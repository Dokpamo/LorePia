import Foundation

struct NativeStagedImport: CustomStringConvertible,
    CustomDebugStringConvertible
{
    let path: String
    let displayName: String
    let sizeBytes: UInt64

    var description: String {
        "NativeStagedImport(path: [REDACTED], "
            + "displayName: [REDACTED], sizeBytes: \(sizeBytes))"
    }

    var debugDescription: String {
        description
    }
}

enum ImportStagingError: Error {
    case invalidSelection
    case selectedFileTooLarge
    case storageUnavailable
}

final class ImportStager {
    let directory: URL

    init(dataRoot: URL) throws {
        directory = dataRoot.appendingPathComponent(
            "native-staging",
            isDirectory: true
        )
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [
                .protectionKey:
                    FileProtectionType.completeUntilFirstUserAuthentication,
            ]
        )
        var mutableDirectory = directory
        var resourceValues = URLResourceValues()
        resourceValues.isExcludedFromBackup = true
        try mutableDirectory.setResourceValues(resourceValues)
        try removeAbandonedFiles()
    }

    func stage(securityScopedURL sourceURL: URL) throws -> NativeStagedImport {
        guard sourceURL.isFileURL else {
            throw ImportStagingError.invalidSelection
        }
        let hasSecurityScope = sourceURL.startAccessingSecurityScopedResource()
        defer {
            if hasSecurityScope {
                sourceURL.stopAccessingSecurityScopedResource()
            }
        }

        var coordinatedResult: Result<NativeStagedImport, Error>?
        var coordinationError: NSError?
        NSFileCoordinator().coordinate(
            readingItemAt: sourceURL,
            options: [.withoutChanges],
            error: &coordinationError
        ) { coordinatedURL in
            coordinatedResult = Result {
                try self.copyBounded(from: coordinatedURL)
            }
        }
        if let coordinatedResult {
            return try coordinatedResult.get()
        }
        if coordinationError != nil {
            throw ImportStagingError.invalidSelection
        }
        throw ImportStagingError.invalidSelection
    }

    func discard(path: String) throws {
        let root = directory.resolvingSymlinksInPath().standardizedFileURL
        let candidate = URL(fileURLWithPath: path)
            .resolvingSymlinksInPath()
            .standardizedFileURL
        guard candidate.deletingLastPathComponent() == root else {
            throw ImportStagingError.invalidSelection
        }
        if FileManager.default.fileExists(atPath: candidate.path) {
            do {
                try FileManager.default.removeItem(at: candidate)
            } catch {
                throw ImportStagingError.storageUnavailable
            }
        }
    }

    private func copyBounded(from sourceURL: URL) throws -> NativeStagedImport {
        let values = try sourceURL.resourceValues(
            forKeys: [.fileSizeKey, .isRegularFileKey]
        )
        guard values.isRegularFile == true else {
            throw ImportStagingError.invalidSelection
        }
        if let fileSize = values.fileSize,
           UInt64(fileSize) > PlatformPolicy.maximumImportBytes
        {
            throw ImportStagingError.selectedFileTooLarge
        }

        let displayName = PlatformPolicy.sanitizeDisplayName(
            sourceURL.lastPathComponent
        )
        let basename =
            PlatformPolicy.ownedStagingPrefix + UUID().uuidString.lowercased()
        let finalURL = directory.appendingPathComponent(
            basename + PlatformPolicy.stagingSuffix(for: displayName),
            isDirectory: false
        )
        let partialURL = directory.appendingPathComponent(
            basename + ".partial",
            isDirectory: false
        )
        guard FileManager.default.createFile(
            atPath: partialURL.path,
            contents: nil,
            attributes: [
                .protectionKey:
                    FileProtectionType.completeUntilFirstUserAuthentication,
            ]
        ) else {
            throw ImportStagingError.storageUnavailable
        }

        do {
            let source = try FileHandle(forReadingFrom: sourceURL)
            let destination = try FileHandle(forWritingTo: partialURL)
            defer {
                try? source.close()
                try? destination.close()
            }

            var copied: UInt64 = 0
            while var chunk = try source.read(
                upToCount: PlatformPolicy.copyBufferBytes
            ), !chunk.isEmpty {
                defer {
                    chunk.resetBytes(in: 0 ..< chunk.count)
                }
                let (nextTotal, overflowed) = copied.addingReportingOverflow(
                    UInt64(chunk.count)
                )
                guard
                    !overflowed,
                    nextTotal <= PlatformPolicy.maximumImportBytes
                else {
                    throw ImportStagingError.selectedFileTooLarge
                }
                try destination.write(contentsOf: chunk)
                copied = nextTotal
            }
            try destination.synchronize()
            try destination.close()
            try FileManager.default.moveItem(at: partialURL, to: finalURL)
            return NativeStagedImport(
                path: finalURL.path,
                displayName: displayName,
                sizeBytes: copied
            )
        } catch {
            try? FileManager.default.removeItem(at: partialURL)
            try? FileManager.default.removeItem(at: finalURL)
            if let error = error as? ImportStagingError {
                throw error
            }
            throw ImportStagingError.storageUnavailable
        }
    }

    private func removeAbandonedFiles() throws {
        let resourceKeys: Set<URLResourceKey> = [
            .contentModificationDateKey,
            .isRegularFileKey,
        ]
        let entries = try FileManager.default.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: Array(resourceKeys),
            options: [.skipsHiddenFiles]
        )
        let now = Date()
        for entry in entries {
            guard
                entry.deletingLastPathComponent().standardizedFileURL
                    == directory.standardizedFileURL,
                let values = try? entry.resourceValues(forKeys: resourceKeys),
                PlatformPolicy.shouldRemoveAbandonedStagingFile(
                    name: entry.lastPathComponent,
                    isRegularFile: values.isRegularFile == true,
                    modifiedAt: values.contentModificationDate,
                    now: now
                )
            else {
                continue
            }
            try? FileManager.default.removeItem(at: entry)
        }
    }
}
