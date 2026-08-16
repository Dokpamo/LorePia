import Foundation
@preconcurrency import Security

enum NativeCredentialStatus: String {
    case missing
    case available
    case unreadable
}

enum KeychainCredentialStoreError: Error {
    case boundRecoveryRequired
    case invalidData
    case operationFailed
    case verificationFailed
    case restoreFailed
}

private final class KeychainRecord {
    var data: Data
    let accessibility: String

    init(data: Data, accessibility: String) {
        self.data = data
        self.accessibility = accessibility
    }
}

protocol KeychainCredentialBackend: AnyObject {
    func copyMatching(
        _ query: [String: Any]
    ) -> (status: OSStatus, result: Any?)

    func update(
        _ query: [String: Any],
        attributes: [String: Any]
    ) -> OSStatus

    func add(_ attributes: [String: Any]) -> OSStatus

    func delete(_ query: [String: Any]) -> OSStatus
}

private final class SystemKeychainCredentialBackend:
    KeychainCredentialBackend
{
    func copyMatching(
        _ query: [String: Any]
    ) -> (status: OSStatus, result: Any?) {
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        return (status, result)
    }

    func update(
        _ query: [String: Any],
        attributes: [String: Any]
    ) -> OSStatus {
        SecItemUpdate(
            query as CFDictionary,
            attributes as CFDictionary
        )
    }

    func add(_ attributes: [String: Any]) -> OSStatus {
        SecItemAdd(attributes as CFDictionary, nil)
    }

    func delete(_ query: [String: Any]) -> OSStatus {
        SecItemDelete(query as CFDictionary)
    }
}

final class KeychainCredentialStore {
    private static let currentBoundReferencePrefix = "lpc2-"
    private static let currentBoundReferenceDigestBytes = 64
    private static let currentBoundEnvelopePrefix =
        "lorepia-provider-credential\nv1\n"
    private static let maximumBoundSecretBytes =
        PlatformPolicy.maximumCredentialWriteBytes
            - (currentBoundEnvelopePrefix.utf8.count + 256 + 1 + 64 + 1)
    private let service = "dev.lorepia.provider-credentials"
    private let requiredAccessibility =
        kSecAttrAccessibleWhenUnlockedThisDeviceOnly as String
    private let backend: any KeychainCredentialBackend

    init() {
        backend = SystemKeychainCredentialBackend()
    }

    init(backend: any KeychainCredentialBackend) {
        self.backend = backend
    }

    func status(reference: String) -> NativeCredentialStatus {
        do {
            try PlatformPolicy.validateReference(reference)
            let query = baseQuery(reference: reference)
            guard let record = try copyCredentialRecord(query: query) else {
                return .missing
            }
            defer {
                wipe(&record.data)
            }
            guard let decoded = String(data: record.data, encoding: .utf8) else {
                return .unreadable
            }
            _ = try PlatformPolicy.validateCredentialForRead(decoded)
            return .available
        } catch {
            return .unreadable
        }
    }

    func read(reference: String) throws -> String? {
        try PlatformPolicy.validateReference(reference)
        let query = baseQuery(reference: reference)
        guard let originalRecord = try copyCredentialRecord(query: query) else {
            return nil
        }
        defer {
            wipe(&originalRecord.data)
        }

        guard
            let decoded = String(data: originalRecord.data, encoding: .utf8)
        else {
            throw KeychainCredentialStoreError.invalidData
        }
        let credential = try PlatformPolicy.validateCredentialForRead(decoded)

        if originalRecord.accessibility != requiredAccessibility {
            do {
                try upsert(originalRecord.data, query: query)
                try verify(originalRecord.data, reference: reference)
            } catch {
                do {
                    try restore(originalRecord, query: query)
                } catch {
                    throw KeychainCredentialStoreError.restoreFailed
                }
                throw error
            }
        }
        return credential
    }

    func boundStatus(reference: String) throws -> NativeCredentialStatus {
        try readBound(reference: reference) == nil
            ? .missing
            : .available
    }

    /// Reads only the exact data-protection item used for authority-bound
    /// credentials. Observation must never rewrite accessibility metadata or
    /// consult a legacy keychain because it can run before the durable
    /// credential operation has reached its Started cutpoint.
    func readBound(reference: String) throws -> String? {
        try PlatformPolicy.validateReference(reference)
        let query = baseQuery(reference: reference)
        let record: KeychainRecord
        do {
            guard let stored = try copyCredentialRecord(query: query) else {
                return nil
            }
            record = stored
        } catch KeychainCredentialStoreError.invalidData {
            throw KeychainCredentialStoreError.boundRecoveryRequired
        }
        defer {
            wipe(&record.data)
        }
        guard record.accessibility == requiredAccessibility else {
            throw KeychainCredentialStoreError.boundRecoveryRequired
        }
        guard let decoded = String(data: record.data, encoding: .utf8) else {
            throw KeychainCredentialStoreError.boundRecoveryRequired
        }
        do {
            return try PlatformPolicy.validateCredentialForRead(decoded)
        } catch {
            throw KeychainCredentialStoreError.boundRecoveryRequired
        }
    }

    func store(reference: String, value: String) throws {
        try PlatformPolicy.validateReference(reference)
        let credential = try PlatformPolicy.validateCredentialForWrite(value)
        try storePrevalidated(reference: reference, value: credential)
    }

    static func isCurrentBoundPhysicalReference(_ reference: String) -> Bool {
        guard
            reference.utf8.count
                == currentBoundReferencePrefix.utf8.count
                    + currentBoundReferenceDigestBytes,
            reference.hasPrefix(currentBoundReferencePrefix)
        else {
            return false
        }
        return reference.utf8
            .dropFirst(currentBoundReferencePrefix.utf8.count)
            .allSatisfy { byte in
                (48...57).contains(byte) || (97...102).contains(byte)
            }
    }

    private static func isCurrentRustPreparedBoundEnvelope(
        _ value: String
    ) -> Bool {
        guard value.hasPrefix(currentBoundEnvelopePrefix) else {
            return false
        }
        let payload = value.dropFirst(currentBoundEnvelopePrefix.count)
        guard
            let authorityEnd = payload.firstIndex(of: "\n"),
            authorityEnd != payload.startIndex
        else {
            return false
        }
        let authority = payload[..<authorityEnd]
        let afterAuthority = payload[payload.index(after: authorityEnd)...]
        guard
            let bindingEnd = afterAuthority.firstIndex(of: "\n"),
            bindingEnd != afterAuthority.startIndex
        else {
            return false
        }
        let binding = afterAuthority[..<bindingEnd]
        let secret = afterAuthority[afterAuthority.index(after: bindingEnd)...]
        let validAuthority =
            !authority.trimmingCharacters(
                in: .whitespacesAndNewlines
            ).isEmpty
            && authority.utf8.count <= 256
            && !authority.unicodeScalars.contains { scalar in
                CharacterSet.controlCharacters.contains(scalar)
            }
        let validBinding =
            binding.utf8.count == 64
            && binding.utf8.allSatisfy { byte in
                (48...57).contains(byte) || (97...102).contains(byte)
            }
        let validSecret =
            !secret.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && secret.utf8.count <= maximumBoundSecretBytes
        return validAuthority && validBinding && validSecret
    }

    func storePrevalidatedBound(reference: String, value: String) throws {
        guard
            Self.isCurrentBoundPhysicalReference(reference),
            Self.isCurrentRustPreparedBoundEnvelope(value)
        else {
            throw KeychainCredentialStoreError.boundRecoveryRequired
        }
        var data = Data(value.utf8)
        defer {
            wipe(&data)
        }

        var item = baseQuery(reference: reference)
        item[kSecValueData as String] = data
        item[kSecAttrAccessible as String] = requiredAccessibility
        let addStatus = backend.add(item)
        if addStatus == errSecDuplicateItem {
            // A slot which was Missing when Core prepared the durable
            // operation became occupied before this atomic add. Never update
            // or delete that unsnapshotted writer's item.
            throw KeychainCredentialStoreError.boundRecoveryRequired
        }
        guard addStatus == errSecSuccess else {
            throw KeychainCredentialStoreError.operationFailed
        }

        do {
            try verify(data, reference: reference)
        } catch {
            // Verification observes only. Once add succeeds, a later writer
            // may already have changed the slot, so no CAS-safe rollback is
            // available and every mutation here would risk their credential.
            throw KeychainCredentialStoreError.boundRecoveryRequired
        }
    }

    /// Stores a Rust-prepared reference and envelope without repeating
    /// deterministic policy checks after the durable Started cutpoint.
    func storePrevalidated(reference: String, value: String) throws {
        var data = Data(value.utf8)
        defer {
            wipe(&data)
        }

        let query = baseQuery(reference: reference)
        let previousRecord = try copyCredentialRecord(query: query)
        defer {
            if let previousRecord {
                wipe(&previousRecord.data)
            }
        }

        do {
            try upsert(data, query: query)
            try verify(data, reference: reference)
        } catch {
            guard let previousRecord else {
                // A failed add can mean another writer won the not-found/add
                // race. Keychain has no compare-and-delete operation, so
                // deleting here could erase that writer's credential. Leave
                // the slot untouched and require explicit recovery instead.
                throw KeychainCredentialStoreError.restoreFailed
            }
            do {
                try restore(previousRecord, query: query)
            } catch {
                throw KeychainCredentialStoreError.restoreFailed
            }
            throw error
        }
    }

    func delete(reference: String) throws {
        try PlatformPolicy.validateReference(reference)
        let status = backend.delete(baseQuery(reference: reference))
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw KeychainCredentialStoreError.operationFailed
        }
    }

    private func baseQuery(reference: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: reference,
            kSecUseDataProtectionKeychain as String: true,
        ]
    }

    private func copyCredentialRecord(
        query: [String: Any]
    ) throws -> KeychainRecord? {
        var dataQuery = query
        dataQuery[kSecReturnData as String] = true
        dataQuery[kSecReturnAttributes as String] = true
        dataQuery[kSecMatchLimit as String] = kSecMatchLimitOne

        let response = backend.copyMatching(dataQuery)
        let status = response.status
        if status == errSecItemNotFound {
            return nil
        }
        guard status == errSecSuccess else {
            throw KeychainCredentialStoreError.operationFailed
        }
        guard
            let attributes = response.result as? [String: Any],
            let data = attributes[kSecValueData as String] as? Data,
            let accessibility =
                attributes[kSecAttrAccessible as String] as? String
        else {
            throw KeychainCredentialStoreError.invalidData
        }
        guard
            !data.isEmpty,
            data.count <= PlatformPolicy.maximumCredentialReadBytes
        else {
            throw KeychainCredentialStoreError.invalidData
        }
        return KeychainRecord(
            data: data,
            accessibility: accessibility
        )
    }

    private func upsert(
        _ data: Data,
        query: [String: Any],
        accessibility: String? = nil
    ) throws {
        let attributes: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String:
                accessibility ?? requiredAccessibility,
        ]
        let updateStatus = backend.update(query, attributes: attributes)
        if updateStatus == errSecSuccess {
            return
        }
        guard updateStatus == errSecItemNotFound else {
            throw KeychainCredentialStoreError.operationFailed
        }

        var item = query
        attributes.forEach { key, value in
            item[key] = value
        }
        let addStatus = backend.add(item)
        // A duplicate means another writer won the add race after our
        // not-found result. Do not overwrite that unsnapshotted item: doing so
        // would make a later verification rollback capable of deleting or
        // replacing another process's credential.
        guard addStatus == errSecSuccess else {
            throw KeychainCredentialStoreError.operationFailed
        }
    }

    private func verify(_ expected: Data, reference: String) throws {
        guard let record = try copyCredentialRecord(
            query: baseQuery(reference: reference)
        ) else {
            throw KeychainCredentialStoreError.verificationFailed
        }
        defer {
            wipe(&record.data)
        }
        guard
            record.data == expected,
            record.accessibility == requiredAccessibility
        else {
            throw KeychainCredentialStoreError.verificationFailed
        }
    }

    private func restore(
        _ record: KeychainRecord,
        query: [String: Any]
    ) throws {
        try upsert(
            record.data,
            query: query,
            accessibility: record.accessibility
        )
        guard let restored = try copyCredentialRecord(query: query) else {
            throw KeychainCredentialStoreError.restoreFailed
        }
        defer {
            wipe(&restored.data)
        }
        guard
            restored.data == record.data,
            restored.accessibility == record.accessibility
        else {
            throw KeychainCredentialStoreError.restoreFailed
        }
    }

    private func wipe(_ data: inout Data) {
        guard !data.isEmpty else {
            return
        }
        data.resetBytes(in: 0 ..< data.count)
    }
}
