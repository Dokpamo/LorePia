import CryptoKit
@preconcurrency import Security
import XCTest

@testable import LorepiaPlatformPlugin

private final class RecordingKeychainCredentialBackend:
  KeychainCredentialBackend
{
  struct Record {
    var data: Data
    var accessibility: String
  }

  var protectedRecord: Record?
  var legacyRecord: Record?
  var failedAddStatus: OSStatus?
  var competingRecordOnFailedAdd: Record?
  var competingRecordAfterSuccessfulAdd: Record?
  private(set) var operations: [String] = []
  private(set) var protectedQueries = 0
  private(set) var legacyQueries = 0

  init(protectedRecord: Record? = nil, legacyRecord: Record? = nil) {
    self.protectedRecord = protectedRecord
    self.legacyRecord = legacyRecord
  }

  func copyMatching(
    _ query: [String: Any]
  ) -> (status: OSStatus, result: Any?) {
    operations.append("copy")
    let usesDataProtection =
      query[kSecUseDataProtectionKeychain as String] as? Bool == true
    if usesDataProtection {
      protectedQueries += 1
    } else {
      legacyQueries += 1
    }
    guard let record = usesDataProtection ? protectedRecord : legacyRecord
    else {
      return (errSecItemNotFound, nil)
    }
    return (
      errSecSuccess,
      [
        kSecValueData as String: record.data,
        kSecAttrAccessible as String: record.accessibility,
      ]
    )
  }

  func update(
    _ query: [String: Any],
    attributes: [String: Any]
  ) -> OSStatus {
    operations.append("update")
    guard protectedRecord != nil else {
      return errSecItemNotFound
    }
    protectedRecord = record(from: attributes)
    if let competingRecordAfterSuccessfulAdd {
      protectedRecord = competingRecordAfterSuccessfulAdd
    }
    return errSecSuccess
  }

  func add(_ attributes: [String: Any]) -> OSStatus {
    operations.append("add")
    if let failedAddStatus {
      if let competingRecordOnFailedAdd {
        protectedRecord = competingRecordOnFailedAdd
      }
      return failedAddStatus
    }
    guard protectedRecord == nil else {
      return errSecDuplicateItem
    }
    protectedRecord = record(from: attributes)
    if let competingRecordAfterSuccessfulAdd {
      protectedRecord = competingRecordAfterSuccessfulAdd
    }
    return errSecSuccess
  }

  func delete(_ query: [String: Any]) -> OSStatus {
    operations.append("delete")
    let usesDataProtection =
      query[kSecUseDataProtectionKeychain as String] as? Bool == true
    if usesDataProtection {
      protectedRecord = nil
    } else {
      legacyRecord = nil
    }
    return errSecSuccess
  }

  private func record(from attributes: [String: Any]) -> Record {
    Record(
      data: attributes[kSecValueData as String] as? Data ?? Data(),
      accessibility:
        attributes[kSecAttrAccessible as String] as? String ?? ""
    )
  }
}

final class PlatformPolicyTests: XCTestCase {
  private let requiredAccessibility =
    kSecAttrAccessibleWhenUnlockedThisDeviceOnly as String
  private let boundPhysicalReference =
    "lpc2-5c3607fcc99a0026c030c0ed2507c5535f509f5e16fa8db97cf02b08aca5447b"
  private let rustPreparedBoundEnvelope =
    "lorepia-provider-credential\nv1\ninstall-a\n"
      + String(repeating: "ab", count: 32)
      + "\nsynthetic-secret"

  func testCredentialConfirmationTextRejectsPromptSpoofingControls() {
    let invalid = [
      "",
      "   ",
      "connection\nApprove",
      "connection\u{0000}Approve",
      "connection\u{2028}Approve",
      "connection\u{2029}Approve",
      "connection\u{202E}Approve",
      "connection\u{2066}Approve",
      "connection\u{2069}Approve",
      "connection\u{200B}Approve",
      "connection\u{200D}Approve",
      "connection\u{2060}Approve",
      "connection\u{FEFF}Approve",
      "connection\u{00AD}Approve",
      "connection\u{034F}Approve",
      "connection\u{E0001}Approve",
    ]

    for value in invalid {
      XCTAssertThrowsError(
        try PlatformPolicy.validateCredentialConfirmationText(
          value,
          maximumBytes: 256
        )
      )
    }
    XCTAssertNoThrow(
      try PlatformPolicy.validateCredentialConfirmationText(
        "연결 a",
        maximumBytes: 256
      )
    )
  }

  func testBoundReadAcceptsExactProtectedRecordWithoutMutation() throws {
    let backend = RecordingKeychainCredentialBackend(
      protectedRecord: .init(
        data: Data("synthetic-bound-envelope".utf8),
        accessibility: requiredAccessibility
      )
    )
    let store = KeychainCredentialStore(backend: backend)

    XCTAssertEqual(
      try store.readBound(reference: "bound-reference"),
      "synthetic-bound-envelope"
    )
    XCTAssertEqual(backend.operations, ["copy"])
    XCTAssertEqual(backend.protectedQueries, 1)
    XCTAssertEqual(backend.legacyQueries, 0)
  }

  func testGenericStatusObservesAccessibilityDriftWithoutMutation() {
    let driftedAccessibility =
      kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly as String
    let backend = RecordingKeychainCredentialBackend(
      protectedRecord: .init(
        data: Data("legacy-raw-secret".utf8),
        accessibility: driftedAccessibility
      )
    )
    let store = KeychainCredentialStore(backend: backend)

    XCTAssertEqual(
      store.status(reference: "legacy-reference"),
      .available
    )
    XCTAssertEqual(backend.operations, ["copy"])
    XCTAssertEqual(
      backend.protectedRecord?.accessibility,
      driftedAccessibility
    )
  }

  func testBoundReadRejectsAccessibilityDriftWithoutMutation() {
    let driftedAccessibility =
      kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly as String
    let backend = RecordingKeychainCredentialBackend(
      protectedRecord: .init(
        data: Data("synthetic-bound-envelope".utf8),
        accessibility: driftedAccessibility
      )
    )
    let store = KeychainCredentialStore(backend: backend)

    XCTAssertThrowsError(
      try store.boundStatus(reference: "bound-reference")
    )
    XCTAssertEqual(backend.operations, ["copy"])
    XCTAssertEqual(
      backend.protectedRecord?.accessibility,
      driftedAccessibility
    )
    XCTAssertEqual(backend.legacyQueries, 0)
  }

  func testBoundAccessibilityDriftIsStableRecoveryRequired() {
    let backend = RecordingKeychainCredentialBackend(
      protectedRecord: .init(
        data: Data("synthetic-bound-envelope".utf8),
        accessibility:
          kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly as String
      )
    )
    let store = KeychainCredentialStore(backend: backend)

    XCTAssertThrowsError(
      try store.readBound(reference: "bound-reference")
    ) { error in
      guard
        let storeError = error as? KeychainCredentialStoreError,
        case .boundRecoveryRequired = storeError
      else {
        return XCTFail("bound accessibility drift must require recovery")
      }
      XCTAssertEqual(
        credentialRejectionCode(for: error),
        "credential_recovery_required"
      )
    }
    XCTAssertEqual(backend.operations, ["copy"])
  }

  func testBoundInvalidUtf8IsStableRecoveryRequired() {
    let backend = RecordingKeychainCredentialBackend(
      protectedRecord: .init(
        data: Data([0xff, 0xfe, 0xfd]),
        accessibility: requiredAccessibility
      )
    )
    let store = KeychainCredentialStore(backend: backend)

    XCTAssertThrowsError(
      try store.readBound(reference: "bound-reference")
    ) { error in
      guard
        let storeError = error as? KeychainCredentialStoreError,
        case .boundRecoveryRequired = storeError
      else {
        return XCTFail("bound UTF-8 damage must require recovery")
      }
      XCTAssertEqual(
        credentialRejectionCode(for: error),
        "credential_recovery_required"
      )
    }
    XCTAssertEqual(backend.operations, ["copy"])
  }

  func testBoundReadNeverFallsBackToLegacyKeychain() throws {
    let backend = RecordingKeychainCredentialBackend(
      legacyRecord: .init(
        data: Data("legacy-raw-secret".utf8),
        accessibility: requiredAccessibility
      )
    )
    let store = KeychainCredentialStore(backend: backend)

    XCTAssertNil(try store.readBound(reference: "bound-reference"))
    XCTAssertEqual(backend.operations, ["copy"])
    XCTAssertEqual(backend.protectedQueries, 1)
    XCTAssertEqual(backend.legacyQueries, 0)
  }

  func testGenericReadStillHardensAccessibility() throws {
    let backend = RecordingKeychainCredentialBackend(
      protectedRecord: .init(
        data: Data("legacy-raw-secret".utf8),
        accessibility:
          kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly as String
      )
    )
    let store = KeychainCredentialStore(backend: backend)

    XCTAssertEqual(
      try store.read(reference: "legacy-reference"),
      "legacy-raw-secret"
    )
    XCTAssertEqual(backend.operations, ["copy", "update", "copy"])
    XCTAssertEqual(
      backend.protectedRecord?.accessibility,
      requiredAccessibility
    )
  }

  func testFailedAddRaceNeverDeletesCompetingCredential() {
    let competingRecord = RecordingKeychainCredentialBackend.Record(
      data: Data("competing-writer-secret".utf8),
      accessibility: requiredAccessibility
    )
    let backend = RecordingKeychainCredentialBackend()
    backend.failedAddStatus = errSecDuplicateItem
    backend.competingRecordOnFailedAdd = competingRecord
    let store = KeychainCredentialStore(backend: backend)

    XCTAssertThrowsError(
      try store.storePrevalidated(
        reference: "bound-reference",
        value: "our-prepared-envelope"
      )
    )
    XCTAssertEqual(backend.operations, ["copy", "update", "add"])
    XCTAssertEqual(
      backend.protectedRecord?.data,
      competingRecord.data
    )
    XCTAssertEqual(
      backend.protectedRecord?.accessibility,
      competingRecord.accessibility
    )
  }

  func testBoundInstallAfterMissingObservationNeverUpdatesRaceWinner()
    throws
  {
    let competingRecord = RecordingKeychainCredentialBackend.Record(
      data: Data("competing-writer-envelope".utf8),
      accessibility: requiredAccessibility
    )
    let backend = RecordingKeychainCredentialBackend()
    let store = KeychainCredentialStore(backend: backend)

    XCTAssertEqual(
      try store.boundStatus(reference: boundPhysicalReference),
      .missing
    )
    backend.protectedRecord = competingRecord

    XCTAssertThrowsError(
      try store.storePrevalidatedBound(
        reference: boundPhysicalReference,
        value: rustPreparedBoundEnvelope
      )
    ) { error in
      XCTAssertEqual(
        credentialRejectionCode(for: error),
        "credential_recovery_required"
      )
    }
    XCTAssertEqual(backend.operations, ["copy", "add"])
    XCTAssertEqual(backend.protectedRecord?.data, competingRecord.data)
    XCTAssertEqual(
      backend.protectedRecord?.accessibility,
      competingRecord.accessibility
    )
  }

  func testBoundInstallDuplicateAddNeverMutatesRaceWinner() {
    let competingRecord = RecordingKeychainCredentialBackend.Record(
      data: Data("competing-writer-envelope".utf8),
      accessibility: requiredAccessibility
    )
    let backend = RecordingKeychainCredentialBackend()
    backend.failedAddStatus = errSecDuplicateItem
    backend.competingRecordOnFailedAdd = competingRecord
    let store = KeychainCredentialStore(backend: backend)

    XCTAssertThrowsError(
      try store.storePrevalidatedBound(
        reference: boundPhysicalReference,
        value: rustPreparedBoundEnvelope
      )
    ) { error in
      XCTAssertEqual(
        credentialRejectionCode(for: error),
        "credential_recovery_required"
      )
    }
    XCTAssertEqual(backend.operations, ["add"])
    XCTAssertEqual(backend.protectedRecord?.data, competingRecord.data)
  }

  func testBoundInstallPreservesExactRustPreparedEnvelope() throws {
    let backend = RecordingKeychainCredentialBackend()
    let store = KeychainCredentialStore(backend: backend)

    try store.storePrevalidatedBound(
      reference: boundPhysicalReference,
      value: rustPreparedBoundEnvelope
    )

    XCTAssertEqual(backend.operations, ["add", "copy"])
    XCTAssertEqual(
      backend.protectedRecord?.data,
      Data(rustPreparedBoundEnvelope.utf8)
    )
    XCTAssertEqual(
      backend.protectedRecord?.accessibility,
      requiredAccessibility
    )
  }

  func testBoundInstallPostAddMismatchNeverRollsBackRaceWinner() {
    let competingRecord = RecordingKeychainCredentialBackend.Record(
      data: Data("post-add-winner-envelope".utf8),
      accessibility: requiredAccessibility
    )
    let backend = RecordingKeychainCredentialBackend()
    backend.competingRecordAfterSuccessfulAdd = competingRecord
    let store = KeychainCredentialStore(backend: backend)

    XCTAssertThrowsError(
      try store.storePrevalidatedBound(
        reference: boundPhysicalReference,
        value: rustPreparedBoundEnvelope
      )
    ) { error in
      XCTAssertEqual(
        credentialRejectionCode(for: error),
        "credential_recovery_required"
      )
    }
    XCTAssertEqual(backend.operations, ["add", "copy"])
    XCTAssertEqual(backend.protectedRecord?.data, competingRecord.data)
    XCTAssertEqual(
      backend.protectedRecord?.accessibility,
      competingRecord.accessibility
    )
  }

  func testReservedBoundReferenceRequiresExactRustPreparedEnvelope() {
    let malformedValues = [
      "raw-legacy-secret",
      "lorepia-provider-credential\nv2\ninstall-a\n"
        + String(repeating: "b", count: 64)
        + "\nsynthetic-secret",
      "lorepia-provider-credential\nv1\ninstall-a\n"
        + String(repeating: "B", count: 64)
        + "\nsynthetic-secret",
      "lorepia-provider-credential\nv1\ninstall-a\n"
        + String(repeating: "b", count: 64)
        + "\n   ",
    ]

    for value in malformedValues {
      let backend = RecordingKeychainCredentialBackend()
      let store = KeychainCredentialStore(backend: backend)

      XCTAssertThrowsError(
        try store.storePrevalidatedBound(
          reference: boundPhysicalReference,
          value: value
        )
      ) { error in
        XCTAssertEqual(
          credentialRejectionCode(for: error),
          "credential_recovery_required"
        )
      }
      XCTAssertEqual(backend.operations, [], value)
      XCTAssertNil(backend.protectedRecord, value)
    }
  }

  func testCurrentBoundReferenceGrammarCannotCaptureRawLegacyReferences() {
    XCTAssertTrue(
      KeychainCredentialStore.isCurrentBoundPhysicalReference(
        boundPhysicalReference
      )
    )
    for rawReference in [
      "connection-a",
      "lpc2-" + String(repeating: "a", count: 63),
      "lpc2-" + String(repeating: "a", count: 65),
      "lpc2-" + String(repeating: "A", count: 64),
      "lpc1-" + String(repeating: "a", count: 64),
      "lpc2-" + String(repeating: "g", count: 64),
    ] {
      XCTAssertFalse(
        KeychainCredentialStore.isCurrentBoundPhysicalReference(rawReference),
        rawReference
      )
    }
  }

  func testRawLegacyStoreRetainsExistingUpsertContract() throws {
    let backend = RecordingKeychainCredentialBackend(
      protectedRecord: .init(
        data: Data("old-raw-secret".utf8),
        accessibility: requiredAccessibility
      )
    )
    let store = KeychainCredentialStore(backend: backend)

    try store.storePrevalidated(
      reference: "connection-a",
      value: "replacement-raw-secret"
    )

    XCTAssertEqual(backend.operations, ["copy", "update", "copy"])
    XCTAssertEqual(
      backend.protectedRecord?.data,
      Data("replacement-raw-secret".utf8)
    )
  }

  func testRawStoreDoesNotInferBoundSemanticsFromReferenceShape() throws {
    let backend = RecordingKeychainCredentialBackend(
      protectedRecord: .init(
        data: Data("old-raw-secret".utf8),
        accessibility: requiredAccessibility
      )
    )
    let store = KeychainCredentialStore(backend: backend)

    try store.storePrevalidated(
      reference: boundPhysicalReference,
      value: "replacement-raw-secret"
    )

    XCTAssertEqual(backend.operations, ["copy", "update", "copy"])
    XCTAssertEqual(
      backend.protectedRecord?.data,
      Data("replacement-raw-secret".utf8)
    )
  }

  func testStagingWorkCannotBlockCredentialWork() {
    let queues = PlatformWorkQueues()
    let stagingStarted = expectation(description: "staging started")
    let stagingFinished = expectation(description: "staging finished")
    let credentialFinished = expectation(description: "credential finished")
    let releaseStaging = DispatchSemaphore(value: 0)

    queues.scheduleStaging {
      stagingStarted.fulfill()
      XCTAssertEqual(
        releaseStaging.wait(timeout: .now() + 2),
        .success
      )
      stagingFinished.fulfill()
    }
    wait(for: [stagingStarted], timeout: 1)

    queues.scheduleCredential {
      credentialFinished.fulfill()
    }
    wait(for: [credentialFinished], timeout: 1)

    DispatchQueue.global(qos: .userInitiated).async {
      releaseStaging.signal()
    }
    wait(for: [stagingFinished], timeout: 1)
  }

  func testWorkQueuesPreserveOrderingWithinEachDomain() {
    let queues = PlatformWorkQueues()
    let credentialFinished = expectation(
      description: "credential work finished"
    )
    credentialFinished.expectedFulfillmentCount = 2
    let stagingFinished = expectation(description: "staging work finished")
    stagingFinished.expectedFulfillmentCount = 2
    let lock = NSLock()
    var credentialOrder: [Int] = []
    var stagingOrder: [Int] = []

    for value in 1...2 {
      queues.scheduleCredential {
        lock.lock()
        credentialOrder.append(value)
        lock.unlock()
        credentialFinished.fulfill()
      }
      queues.scheduleStaging {
        lock.lock()
        stagingOrder.append(value)
        lock.unlock()
        stagingFinished.fulfill()
      }
    }

    wait(
      for: [credentialFinished, stagingFinished],
      timeout: 1
    )
    XCTAssertEqual(credentialOrder, [1, 2])
    XCTAssertEqual(stagingOrder, [1, 2])
  }

  func testReferenceLimitUsesUTF8Bytes() throws {
    try PlatformPolicy.validateReference(
      String(repeating: "a", count: 256)
    )
    XCTAssertThrowsError(
      try PlatformPolicy.validateReference(
        String(repeating: "가", count: 86)
      )
    )
  }

  func testCredentialIsOpaquePreservedAndBounded() throws {
    XCTAssertEqual(
      try PlatformPolicy.validateCredentialForWrite("  secret\n"),
      "  secret\n"
    )
    XCTAssertThrowsError(
      try PlatformPolicy.validateCredentialForWrite(" \n")
    )
    XCTAssertThrowsError(
      try PlatformPolicy.validateCredentialForWrite(
        String(repeating: "a", count: 16 * 1_024 + 1)
      )
    )
    XCTAssertEqual(
      try PlatformPolicy.validateCredentialForRead(
        String(repeating: "r", count: 32 * 1_024)
      ).utf8.count,
      32 * 1_024
    )
  }

  func testSensitiveCaptureIsNonemptyAndBoundedByUTF8Bytes() throws {
    var captured = try PlatformPolicy.validateSensitiveCapture(
      "curl https://example.test",
      maximumBytes: 1_024
    )
    XCTAssertEqual(captured.count, 25)
    captured.resetBytes(in: 0..<captured.count)
    XCTAssertThrowsError(
      try PlatformPolicy.validateSensitiveCapture(
        "",
        maximumBytes: 1_024
      )
    )
    XCTAssertThrowsError(
      try PlatformPolicy.validateSensitiveCapture(
        "x",
        maximumBytes: PlatformPolicy.maximumSensitiveCaptureBytes + 1
      )
    )
  }

  func testSensitiveCaptureResponseIsTransientAndCarriesNoPath() throws {
    let response = SensitiveCaptureResponse(
      value: "synthetic-one-shot-secret",
      clipboardCleanup: .cleared
    )
    let encoded = try JSONEncoder().encode(response)
    let object = try XCTUnwrap(
      JSONSerialization.jsonObject(with: encoded) as? [String: Any]
    )

    XCTAssertEqual(
      object["value"] as? String,
      "synthetic-one-shot-secret"
    )
    XCTAssertNil(object["path"])
    XCTAssertNil(object["sizeBytes"])
  }

  func testLegacySensitiveCaptureCleanupRemovesOnlyOwnedArtifacts()
    throws
  {
    let root = FileManager.default.temporaryDirectory
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let legacyRoot = root.appendingPathComponent(
      "sensitive-capture",
      isDirectory: true
    )
    try FileManager.default.createDirectory(
      at: legacyRoot,
      withIntermediateDirectories: true
    )
    defer {
      try? FileManager.default.removeItem(at: root)
    }
    let owned = legacyRoot.appendingPathComponent(
      "lorepia-sensitive-abandoned"
    )
    let unrelated = legacyRoot.appendingPathComponent("keep-me")
    try Data("legacy-plaintext".utf8).write(to: owned)
    try Data("unrelated".utf8).write(to: unrelated)

    purgeLegacySensitiveCaptureFiles(dataRoot: root)

    XCTAssertFalse(FileManager.default.fileExists(atPath: owned.path))
    XCTAssertTrue(FileManager.default.fileExists(atPath: unrelated.path))
  }

  func testStagingSuffixIsAllowlisted() {
    XCTAssertEqual(
      PlatformPolicy.stagingSuffix(for: "character.CHARX"),
      ".charx"
    )
    XCTAssertEqual(
      PlatformPolicy.stagingSuffix(for: "archive.tar.gz"),
      ".pending"
    )
  }

  func testDisplayNameReplacesControls() {
    XCTAssertEqual(
      PlatformPolicy.sanitizeDisplayName("bad\u{0000}name.json"),
      "bad\u{FFFD}name.json"
    )
  }

  func testContentSourceExportRequiresExactCasIdentityAndPortableName()
    throws
  {
    let root = FileManager.default.temporaryDirectory
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(
      at: root,
      withIntermediateDirectories: true
    )
    defer {
      try? FileManager.default.removeItem(at: root)
    }
    let bytes = Data("synthetic-lossless-source".utf8)
    let digest = SHA256.hash(data: bytes).map {
      String(format: "%02x", $0)
    }.joined()
    let source =
      root
      .appendingPathComponent("sources", isDirectory: true)
      .appendingPathComponent("sha256", isDirectory: true)
      .appendingPathComponent(
        String(digest.prefix(2)),
        isDirectory: true
      )
      .appendingPathComponent(
        String(digest.dropFirst(2)),
        isDirectory: false
      )
    try FileManager.default.createDirectory(
      at: source.deletingLastPathComponent(),
      withIntermediateDirectories: true
    )
    try bytes.write(to: source)

    let abandoned =
      root
      .appendingPathComponent(
        "content-export-staging",
        isDirectory: true
      )
      .appendingPathComponent(
        "lorepia-export-abandoned",
        isDirectory: true
      )
    try FileManager.default.createDirectory(
      at: abandoned,
      withIntermediateDirectories: true
    )
    try Data("abandoned".utf8).write(
      to: abandoned.appendingPathComponent("stale")
    )

    let exporter = try ContentSourceExporter(dataRoot: root)
    XCTAssertFalse(FileManager.default.fileExists(atPath: abandoned.path))
    let prepared = try exporter.prepare(
      sourcePath: source.path,
      suggestedName: "lorepia-character-test.json",
      expectedSizeBytes: UInt64(bytes.count),
      expectedSha256: digest
    )
    XCTAssertEqual(prepared.sourceURL, source)
    XCTAssertEqual(
      prepared.presentationURL.lastPathComponent,
      "lorepia-character-test.json"
    )
    XCTAssertTrue(
      FileManager.default.fileExists(
        atPath: prepared.presentationURL.path
      )
    )
    let saved = root.appendingPathComponent("캐릭터.json")
    try bytes.write(to: saved)
    XCTAssertEqual(
      try exporter.verifySavedCopy(url: saved, export: prepared),
      "캐릭터.json"
    )

    XCTAssertThrowsError(
      try exporter.prepare(
        sourcePath: root.appendingPathComponent("outside").path,
        suggestedName: "character.json",
        expectedSizeBytes: UInt64(bytes.count),
        expectedSha256: digest
      )
    )
    XCTAssertThrowsError(
      try PlatformPolicy.validateExportSuggestedName("../card.json")
    )
    XCTAssertThrowsError(
      try PlatformPolicy.validateExportSuggestedName("NUL.json")
    )
    XCTAssertThrowsError(
      try PlatformPolicy.validateExportSha256(digest.uppercased())
    )
    XCTAssertThrowsError(
      try PlatformPolicy.validateExportReceiptDisplayName(
        "folder/card.json"
      )
    )

    let description = String(describing: prepared)
    XCTAssertFalse(description.contains(source.path))
    XCTAssertFalse(description.contains(prepared.presentationURL.path))
    XCTAssertFalse(description.contains(prepared.stagingDirectory.path))
    XCTAssertFalse(description.contains(prepared.suggestedName))
    XCTAssertTrue(description.contains("[REDACTED]"))

    try exporter.cleanup(prepared)
    XCTAssertFalse(
      FileManager.default.fileExists(
        atPath: prepared.stagingDirectory.path
      )
    )
  }

  func testAbandonedStagingCleanupRequiresOwnedOldRegularFile() {
    let now = Date(timeIntervalSince1970: 200_000)
    let old = now.addingTimeInterval(-PlatformPolicy.abandonedStagingAge)
    XCTAssertTrue(
      PlatformPolicy.shouldRemoveAbandonedStagingFile(
        name: PlatformPolicy.ownedStagingPrefix + "synthetic.json",
        isRegularFile: true,
        modifiedAt: old,
        now: now
      )
    )
    XCTAssertFalse(
      PlatformPolicy.shouldRemoveAbandonedStagingFile(
        name: "unrelated.json",
        isRegularFile: true,
        modifiedAt: old,
        now: now
      )
    )
    XCTAssertFalse(
      PlatformPolicy.shouldRemoveAbandonedStagingFile(
        name: PlatformPolicy.ownedStagingPrefix + "fresh.json",
        isRegularFile: true,
        modifiedAt: old.addingTimeInterval(1),
        now: now
      )
    )
    XCTAssertFalse(
      PlatformPolicy.shouldRemoveAbandonedStagingFile(
        name: PlatformPolicy.ownedStagingPrefix + "directory",
        isRegularFile: false,
        modifiedAt: old,
        now: now
      )
    )
  }

  func testStagedImportDescriptionRedactsPathAndDisplayName() {
    let path = "/synthetic/private/card.json"
    let displayName = "private-card.json"
    let staged = NativeStagedImport(
      path: path,
      displayName: displayName,
      sizeBytes: 42
    )

    XCTAssertFalse(String(describing: staged).contains(path))
    XCTAssertFalse(String(describing: staged).contains(displayName))
    XCTAssertFalse(String(reflecting: staged).contains(path))
    XCTAssertFalse(String(reflecting: staged).contains(displayName))
    XCTAssertTrue(String(describing: staged).contains("[REDACTED]"))
  }
}
