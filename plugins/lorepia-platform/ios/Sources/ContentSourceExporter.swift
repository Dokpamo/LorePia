import CryptoKit
import Foundation

struct NativeContentSourceExport: CustomStringConvertible,
  CustomDebugStringConvertible
{
  let presentationURL: URL
  let sourceURL: URL
  let stagingDirectory: URL
  let suggestedName: String
  let expectedSizeBytes: UInt64
  let expectedSha256: String

  var description: String {
    "NativeContentSourceExport(presentationURL: [REDACTED], "
      + "sourceURL: [REDACTED], "
      + "stagingDirectory: [REDACTED], "
      + "suggestedName: [REDACTED], "
      + "expectedSizeBytes: \(expectedSizeBytes), "
      + "expectedSha256: \(expectedSha256))"
  }

  var debugDescription: String {
    description
  }
}

enum ContentSourceExportError: Error {
  case invalidInput
  case storageUnavailable
}

final class ContentSourceExporter {
  private let dataRoot: URL
  private let stagingRoot: URL

  init(dataRoot: URL) throws {
    self.dataRoot =
      dataRoot.resolvingSymlinksInPath()
      .standardizedFileURL
    stagingRoot = self.dataRoot.appendingPathComponent(
      "content-export-staging",
      isDirectory: true
    )
    try FileManager.default.createDirectory(
      at: stagingRoot,
      withIntermediateDirectories: true,
      attributes: [
        .protectionKey:
          FileProtectionType.completeUntilFirstUserAuthentication,
        .posixPermissions: 0o700,
      ]
    )
    var mutableStagingRoot = stagingRoot
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try mutableStagingRoot.setResourceValues(values)
    try cleanupAbandonedAliases()
  }

  func prepare(
    sourcePath: String,
    suggestedName: String,
    expectedSizeBytes: UInt64,
    expectedSha256: String
  ) throws -> NativeContentSourceExport {
    let sourceURL = try validateSource(
      sourcePath: sourcePath,
      suggestedName: suggestedName,
      expectedSizeBytes: expectedSizeBytes,
      expectedSha256: expectedSha256
    )
    let (directory, presentationURL) = try createPresentationAlias(
      sourceURL: sourceURL,
      suggestedName: suggestedName,
      expectedSizeBytes: expectedSizeBytes,
      expectedSha256: expectedSha256
    )
    return NativeContentSourceExport(
      presentationURL: presentationURL,
      sourceURL: sourceURL,
      stagingDirectory: directory,
      suggestedName: suggestedName,
      expectedSizeBytes: expectedSizeBytes,
      expectedSha256: expectedSha256
    )
  }

  func revalidateSource(
    _ export: NativeContentSourceExport
  ) throws {
    _ = try validateSource(
      sourcePath: export.sourceURL.path,
      suggestedName: export.suggestedName,
      expectedSizeBytes: export.expectedSizeBytes,
      expectedSha256: export.expectedSha256
    )
  }

  func cleanup(_ export: NativeContentSourceExport) throws {
    let directory = export.stagingDirectory.standardizedFileURL
    guard
      directory.deletingLastPathComponent() == stagingRoot,
      directory.lastPathComponent.hasPrefix(aliasDirectoryPrefix),
      export.presentationURL.deletingLastPathComponent()
        .standardizedFileURL == directory,
      export.presentationURL.lastPathComponent == export.suggestedName
    else {
      throw ContentSourceExportError.invalidInput
    }
    if FileManager.default.fileExists(atPath: directory.path) {
      do {
        try FileManager.default.removeItem(at: directory)
      } catch {
        throw ContentSourceExportError.storageUnavailable
      }
    }
  }

  func verifySavedCopy(
    url: URL,
    export: NativeContentSourceExport
  ) throws -> String {
    let hasSecurityScope = url.startAccessingSecurityScopedResource()
    defer {
      if hasSecurityScope {
        url.stopAccessingSecurityScopedResource()
      }
    }
    try verify(
      url: url,
      expectedSizeBytes: export.expectedSizeBytes,
      expectedSha256: export.expectedSha256
    )
    return try PlatformPolicy.validateExportReceiptDisplayName(
      url.lastPathComponent
    )
  }

  private func validateSource(
    sourcePath: String,
    suggestedName: String,
    expectedSizeBytes: UInt64,
    expectedSha256: String
  ) throws -> URL {
    try PlatformPolicy.validateExportSuggestedName(suggestedName)
    try PlatformPolicy.validateExportSha256(expectedSha256)
    guard expectedSizeBytes > 0 else {
      throw ContentSourceExportError.invalidInput
    }

    let expectedURL =
      dataRoot
      .appendingPathComponent("sources", isDirectory: true)
      .appendingPathComponent("sha256", isDirectory: true)
      .appendingPathComponent(
        String(expectedSha256.prefix(2)),
        isDirectory: true
      )
      .appendingPathComponent(
        String(expectedSha256.dropFirst(2)),
        isDirectory: false
      )
      .standardizedFileURL
    let sourceURL = URL(fileURLWithPath: sourcePath)
      .standardizedFileURL
    guard
      sourceURL == expectedURL,
      expectedURL.resolvingSymlinksInPath().standardizedFileURL
        == expectedURL
    else {
      throw ContentSourceExportError.invalidInput
    }
    let values = try sourceURL.resourceValues(
      forKeys: [.fileSizeKey, .isRegularFileKey, .isSymbolicLinkKey]
    )
    guard
      values.isRegularFile == true,
      values.isSymbolicLink != true,
      values.fileSize.map({ UInt64($0) }) == expectedSizeBytes
    else {
      throw ContentSourceExportError.invalidInput
    }
    try verify(
      url: sourceURL,
      expectedSizeBytes: expectedSizeBytes,
      expectedSha256: expectedSha256
    )
    return sourceURL
  }

  private func createPresentationAlias(
    sourceURL: URL,
    suggestedName: String,
    expectedSizeBytes: UInt64,
    expectedSha256: String
  ) throws -> (URL, URL) {
    let directory = stagingRoot.appendingPathComponent(
      aliasDirectoryPrefix + UUID().uuidString.lowercased(),
      isDirectory: true
    )
    let alias = directory.appendingPathComponent(
      suggestedName,
      isDirectory: false
    )
    do {
      try FileManager.default.createDirectory(
        at: directory,
        withIntermediateDirectories: false,
        attributes: [
          .protectionKey:
            FileProtectionType
            .completeUntilFirstUserAuthentication,
          .posixPermissions: 0o700,
        ]
      )
      do {
        try FileManager.default.linkItem(at: sourceURL, to: alias)
      } catch {
        try? FileManager.default.removeItem(at: alias)
        try copyBounded(
          sourceURL: sourceURL,
          destinationURL: alias,
          expectedSizeBytes: expectedSizeBytes,
          expectedSha256: expectedSha256
        )
      }
      try verify(
        url: alias,
        expectedSizeBytes: expectedSizeBytes,
        expectedSha256: expectedSha256
      )
      return (directory, alias)
    } catch {
      try? FileManager.default.removeItem(at: directory)
      if let exportError = error as? ContentSourceExportError {
        throw exportError
      }
      throw ContentSourceExportError.storageUnavailable
    }
  }

  private func copyBounded(
    sourceURL: URL,
    destinationURL: URL,
    expectedSizeBytes: UInt64,
    expectedSha256: String
  ) throws {
    guard
      FileManager.default.createFile(
        atPath: destinationURL.path,
        contents: nil,
        attributes: [
          .protectionKey:
            FileProtectionType.completeUntilFirstUserAuthentication,
          .posixPermissions: 0o600,
        ]
      )
    else {
      throw ContentSourceExportError.storageUnavailable
    }
    let source = try FileHandle(forReadingFrom: sourceURL)
    let destination = try FileHandle(forWritingTo: destinationURL)
    defer {
      try? source.close()
      try? destination.close()
    }
    var hasher = SHA256()
    var copied = UInt64(0)
    do {
      while var chunk = try source.read(
        upToCount: PlatformPolicy.copyBufferBytes
      ), !chunk.isEmpty {
        defer {
          chunk.resetBytes(in: 0..<chunk.count)
        }
        let (next, overflowed) = copied.addingReportingOverflow(
          UInt64(chunk.count)
        )
        guard !overflowed, next <= expectedSizeBytes else {
          throw ContentSourceExportError.invalidInput
        }
        hasher.update(data: chunk)
        try destination.write(contentsOf: chunk)
        copied = next
      }
      try destination.synchronize()
      try destination.close()
    } catch let error as ContentSourceExportError {
      throw error
    } catch {
      throw ContentSourceExportError.storageUnavailable
    }
    let digest = hasher.finalize().map {
      String(format: "%02x", $0)
    }.joined()
    guard copied == expectedSizeBytes, digest == expectedSha256 else {
      throw ContentSourceExportError.invalidInput
    }
  }

  private func cleanupAbandonedAliases() throws {
    let entries: [URL]
    do {
      entries = try FileManager.default.contentsOfDirectory(
        at: stagingRoot,
        includingPropertiesForKeys: [.isDirectoryKey],
        options: [.skipsHiddenFiles]
      )
    } catch {
      throw ContentSourceExportError.storageUnavailable
    }
    for entry in entries
    where
      entry.deletingLastPathComponent().standardizedFileURL
      == stagingRoot
      && entry.lastPathComponent.hasPrefix(aliasDirectoryPrefix)
    {
      do {
        try FileManager.default.removeItem(at: entry)
      } catch {
        throw ContentSourceExportError.storageUnavailable
      }
    }
  }

  private func verify(
    url: URL,
    expectedSizeBytes: UInt64,
    expectedSha256: String
  ) throws {
    let handle: FileHandle
    do {
      handle = try FileHandle(forReadingFrom: url)
    } catch {
      throw ContentSourceExportError.storageUnavailable
    }
    defer {
      try? handle.close()
    }

    var hasher = SHA256()
    var total = UInt64(0)
    do {
      while var chunk = try handle.read(
        upToCount: PlatformPolicy.copyBufferBytes
      ), !chunk.isEmpty {
        defer {
          chunk.resetBytes(in: 0..<chunk.count)
        }
        let (nextTotal, overflowed) = total.addingReportingOverflow(
          UInt64(chunk.count)
        )
        guard !overflowed, nextTotal <= expectedSizeBytes else {
          throw ContentSourceExportError.invalidInput
        }
        hasher.update(data: chunk)
        total = nextTotal
      }
    } catch let error as ContentSourceExportError {
      throw error
    } catch {
      throw ContentSourceExportError.storageUnavailable
    }
    let actualSha256 = hasher.finalize().map {
      String(format: "%02x", $0)
    }.joined()
    guard
      total == expectedSizeBytes,
      actualSha256 == expectedSha256
    else {
      throw ContentSourceExportError.invalidInput
    }
  }

  private let aliasDirectoryPrefix = "lorepia-export-"
}
