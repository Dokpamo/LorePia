import Foundation
import Tauri
import UIKit
import UniformTypeIdentifiers

private protocol RedactedDescription: CustomStringConvertible,
  CustomDebugStringConvertible
{}

extension RedactedDescription {
  var description: String {
    "[REDACTED]"
  }

  var debugDescription: String {
    description
  }
}

private struct ReferenceArgs: Decodable, RedactedDescription {
  let reference: String
}

private struct CredentialArgs: Decodable, RedactedDescription {
  let reference: String
  let value: String
}

private struct StagedPathArgs: Decodable, RedactedDescription {
  let path: String
}

private struct SensitiveCaptureArgs: Decodable, RedactedDescription {
  let maximumBytes: UInt64
}

private struct SaveContentSourceArgs: Decodable, RedactedDescription {
  let sourcePath: String
  let suggestedName: String
  let expectedSizeBytes: UInt64
  let expectedSha256: String
}

private struct PathResponse: Encodable, RedactedDescription {
  let path: String
}

private struct CredentialResponse: Encodable, RedactedDescription {
  let value: String?
}

private struct CredentialStatusResponse: Encodable {
  let status: String
}

enum ClipboardCleanupStatus: String, Encodable {
  case cleared
  case alreadyReplaced = "already_replaced"
  case clearFailed = "clear_failed"
}

private struct CaptureStatusResponse: Encodable {
  let clipboardCleanup: ClipboardCleanupStatus
}

struct SensitiveCaptureResponse: Encodable, RedactedDescription {
  let value: String
  let clipboardCleanup: ClipboardCleanupStatus
}

private struct PickResponse: Encodable, RedactedDescription {
  let selected: Bool
  let path: String?
  let displayName: String?
  let sizeBytes: UInt64?
}

private struct SaveContentSourceResponse: Encodable {
  let selected: Bool
  let displayName: String?
  let sizeBytes: UInt64?
  let sha256: String?
}

private struct NativeStorage {
  let dataRoot: URL
  let exporter: ContentSourceExporter
  let stager: ImportStager

  init() throws {
    guard
      let applicationSupport = FileManager.default.urls(
        for: .applicationSupportDirectory,
        in: .userDomainMask
      ).first
    else {
      throw ImportStagingError.storageUnavailable
    }
    dataRoot = applicationSupport.appendingPathComponent(
      "LorePia",
      isDirectory: true
    )
    try FileManager.default.createDirectory(
      at: dataRoot,
      withIntermediateDirectories: true,
      attributes: [
        .protectionKey:
          FileProtectionType.completeUntilFirstUserAuthentication
      ]
    )
    exporter = try ContentSourceExporter(dataRoot: dataRoot)
    stager = try ImportStager(dataRoot: dataRoot)
    purgeLegacySensitiveCaptureFiles(dataRoot: dataRoot)
  }
}

/// Removes plaintext staging artifacts created by releases predating the
/// in-memory capture transport. New captures never write this directory.
func purgeLegacySensitiveCaptureFiles(dataRoot: URL) {
  let legacyRoot = dataRoot.appendingPathComponent(
    "sensitive-capture",
    isDirectory: true
  )
  guard
    let entries = try? FileManager.default.contentsOfDirectory(
      at: legacyRoot,
      includingPropertiesForKeys: [.isRegularFileKey],
      options: [.skipsHiddenFiles]
    )
  else {
    return
  }
  for entry in entries
  where entry.lastPathComponent.hasPrefix("lorepia-sensitive-") {
    try? FileManager.default.removeItem(at: entry)
  }
}

private enum PendingPickerOperation {
  case importing(Invoke)
  case preparingExport(Invoke)
  case exporting(Invoke, NativeContentSourceExport)
}

final class PlatformWorkQueues {
  private let credentialQueue = DispatchQueue(
    label: "dev.lorepia.tauri.platform.credential",
    qos: .userInitiated
  )
  private let stagingQueue = DispatchQueue(
    label: "dev.lorepia.tauri.platform.staging",
    qos: .userInitiated
  )

  func scheduleCredential(_ operation: @escaping () -> Void) {
    credentialQueue.async(execute: operation)
  }

  func scheduleStaging(_ operation: @escaping () -> Void) {
    stagingQueue.async(execute: operation)
  }
}

final class LorepiaPlatformPlugin: Plugin, UIDocumentPickerDelegate {
  private let credentialStore = KeychainCredentialStore()
  private let workQueues = PlatformWorkQueues()
  private let storage: Result<NativeStorage, Error>
  private var pendingPickerOperation: PendingPickerOperation?
  private var sensitiveCaptureInFlight = false

  override init() {
    storage = Result {
      try NativeStorage()
    }
    super.init()
  }

  @objc public func dataRoot(_ invoke: Invoke) {
    do {
      let storage = try storage.get()
      invoke.resolve(PathResponse(path: storage.dataRoot.path))
    } catch {
      invoke.reject("storage unavailable", code: "storage_unavailable")
    }
  }

  @objc public func credentialStatus(_ invoke: Invoke) {
    workQueues.scheduleCredential {
      do {
        let args = try invoke.parseArgs(ReferenceArgs.self)
        let status = self.credentialStore.status(
          reference: args.reference
        )
        invoke.resolve(
          CredentialStatusResponse(status: status.rawValue)
        )
      } catch {
        invoke.resolve(
          CredentialStatusResponse(
            status: NativeCredentialStatus.unreadable.rawValue
          )
        )
      }
    }
  }

  @objc public func boundCredentialStatus(_ invoke: Invoke) {
    workQueues.scheduleCredential {
      do {
        let args = try invoke.parseArgs(ReferenceArgs.self)
        let status = try self.credentialStore.boundStatus(
          reference: args.reference
        )
        invoke.resolve(
          CredentialStatusResponse(status: status.rawValue)
        )
      } catch {
        self.rejectCredential(invoke, error: error)
      }
    }
  }

  @objc public func readCredential(_ invoke: Invoke) {
    workQueues.scheduleCredential {
      do {
        let args = try invoke.parseArgs(ReferenceArgs.self)
        let value = try self.credentialStore.read(
          reference: args.reference
        )
        invoke.resolve(CredentialResponse(value: value))
      } catch {
        self.rejectCredential(invoke, error: error)
      }
    }
  }

  @objc public func readBoundCredential(_ invoke: Invoke) {
    workQueues.scheduleCredential {
      do {
        let args = try invoke.parseArgs(ReferenceArgs.self)
        let value = try self.credentialStore.readBound(
          reference: args.reference
        )
        invoke.resolve(CredentialResponse(value: value))
      } catch {
        self.rejectCredential(invoke, error: error)
      }
    }
  }

  @objc public func storeCredential(_ invoke: Invoke) {
    workQueues.scheduleCredential {
      do {
        let args = try invoke.parseArgs(CredentialArgs.self)
        try self.credentialStore.storePrevalidated(
          reference: args.reference,
          value: args.value
        )
        invoke.resolve()
      } catch {
        self.rejectCredential(invoke, error: error)
      }
    }
  }

  @objc public func storeBoundCredential(_ invoke: Invoke) {
    workQueues.scheduleCredential {
      do {
        let args = try invoke.parseArgs(CredentialArgs.self)
        try self.credentialStore.storePrevalidatedBound(
          reference: args.reference,
          value: args.value
        )
        invoke.resolve()
      } catch {
        self.rejectCredential(invoke, error: error)
      }
    }
  }

  @objc public func captureCredential(_ invoke: Invoke) {
    DispatchQueue.main.async {
      guard self.beginSensitiveCapture(invoke) else {
        return
      }
      let args: ReferenceArgs
      let captured: String
      let changeCount: Int
      do {
        args = try invoke.parseArgs(ReferenceArgs.self)
        try PlatformPolicy.validateReference(args.reference)
        (captured, changeCount) = try self.foregroundClipboardText()
        _ = try PlatformPolicy.validateCredentialForWrite(captured)
      } catch {
        self.finishSensitiveCapture()
        invoke.reject(
          "clipboard is empty or invalid",
          code: "invalid_input"
        )
        return
      }

      self.workQueues.scheduleCredential {
        do {
          try self.credentialStore.store(
            reference: args.reference,
            value: captured
          )
          DispatchQueue.main.async {
            let cleanup = self.clearClipboardIfUnchanged(
              captured,
              changeCount: changeCount
            )
            self.finishSensitiveCapture()
            invoke.resolve(
              CaptureStatusResponse(clipboardCleanup: cleanup)
            )
          }
        } catch {
          DispatchQueue.main.async {
            self.finishSensitiveCapture()
            self.rejectCredential(invoke, error: error)
          }
        }
      }
    }
  }

  @objc public func captureSensitiveText(_ invoke: Invoke) {
    DispatchQueue.main.async {
      guard self.beginSensitiveCapture(invoke) else {
        return
      }
      let captured: String
      let changeCount: Int
      var encoded: Data
      do {
        let args = try invoke.parseArgs(SensitiveCaptureArgs.self)
        (captured, changeCount) = try self.foregroundClipboardText()
        encoded = try PlatformPolicy.validateSensitiveCapture(
          captured,
          maximumBytes: args.maximumBytes
        )
      } catch {
        self.finishSensitiveCapture()
        invoke.reject(
          "clipboard is empty or invalid",
          code: "invalid_input"
        )
        return
      }

      defer {
        if !encoded.isEmpty {
          encoded.resetBytes(in: 0..<encoded.count)
        }
      }
      let cleanup = self.clearClipboardIfUnchanged(
        captured,
        changeCount: changeCount
      )
      self.finishSensitiveCapture()
      invoke.resolve(
        SensitiveCaptureResponse(
          value: captured,
          clipboardCleanup: cleanup
        )
      )
    }
  }

  @objc public func deleteCredential(_ invoke: Invoke) {
    workQueues.scheduleCredential {
      do {
        let args = try invoke.parseArgs(ReferenceArgs.self)
        try self.credentialStore.delete(reference: args.reference)
        invoke.resolve()
      } catch {
        invoke.reject(
          "credential unavailable",
          code: "credential_unavailable"
        )
      }
    }
  }

  @objc public func pickImport(_ invoke: Invoke) {
    DispatchQueue.main.async {
      guard self.pendingPickerOperation == nil else {
        invoke.reject("file picker is busy", code: "busy")
        return
      }
      guard let viewController = self.manager.viewController else {
        invoke.reject(
          "file selection failed",
          code: "selection_failed"
        )
        return
      }

      let picker = UIDocumentPickerViewController(
        forOpeningContentTypes: [.data],
        asCopy: false
      )
      picker.allowsMultipleSelection = false
      picker.delegate = self
      self.pendingPickerOperation = .importing(invoke)
      viewController.present(picker, animated: true)
    }
  }

  @objc public func saveContentSource(_ invoke: Invoke) {
    DispatchQueue.main.async {
      guard self.pendingPickerOperation == nil else {
        invoke.reject("file picker is busy", code: "busy")
        return
      }
      self.pendingPickerOperation = .preparingExport(invoke)
      self.workQueues.scheduleStaging {
        do {
          let args = try invoke.parseArgs(
            SaveContentSourceArgs.self
          )
          let storage = try self.storage.get()
          let export = try storage.exporter.prepare(
            sourcePath: args.sourcePath,
            suggestedName: args.suggestedName,
            expectedSizeBytes: args.expectedSizeBytes,
            expectedSha256: args.expectedSha256
          )
          DispatchQueue.main.async {
            guard
              case .some(.preparingExport(_)) =
                self.pendingPickerOperation,
              let viewController = self.manager.viewController
            else {
              self.pendingPickerOperation = nil
              self.workQueues.scheduleStaging {
                do {
                  try storage.exporter.cleanup(export)
                  invoke.reject(
                    "file selection failed",
                    code: "selection_failed"
                  )
                } catch {
                  invoke.reject(
                    "content source export cleanup failed",
                    code: "storage_unavailable"
                  )
                }
              }
              return
            }
            // UIDocumentPicker owns the destination and replacement
            // confirmation. The presentation alias has the safe
            // suggested name and points to exact verified CAS bytes;
            // no path or bytes cross into the webview.
            let picker = UIDocumentPickerViewController(
              forExporting: [export.presentationURL],
              asCopy: true
            )
            picker.delegate = self
            self.pendingPickerOperation = .exporting(
              invoke,
              export
            )
            viewController.present(picker, animated: true)
          }
        } catch {
          DispatchQueue.main.async {
            self.pendingPickerOperation = nil
            invoke.reject(
              "invalid content source export",
              code: "invalid_input"
            )
          }
        }
      }
    }
  }

  @objc public func discardStagedImport(_ invoke: Invoke) {
    workQueues.scheduleStaging {
      do {
        let args = try invoke.parseArgs(StagedPathArgs.self)
        let storage = try self.storage.get()
        try storage.stager.discard(path: args.path)
        invoke.resolve()
      } catch {
        invoke.reject(
          "storage unavailable",
          code: "storage_unavailable"
        )
      }
    }
  }

  func documentPicker(
    _ controller: UIDocumentPickerViewController,
    didPickDocumentsAt urls: [URL]
  ) {
    guard let operation = takePendingPickerOperation() else {
      return
    }
    guard let selectedURL = urls.first else {
      resolvePickerCancellation(operation)
      return
    }

    switch operation {
    case .importing(let invoke):
      finishImport(invoke, selectedURL: selectedURL)
    case .exporting(let invoke, let export):
      finishContentSourceExport(
        invoke,
        export: export,
        selectedURL: selectedURL
      )
    case .preparingExport(let invoke):
      invoke.reject("file selection failed", code: "selection_failed")
    }
  }

  func documentPickerWasCancelled(
    _ controller: UIDocumentPickerViewController
  ) {
    if let operation = takePendingPickerOperation() {
      resolvePickerCancellation(operation)
    }
  }

  private func takePendingPickerOperation() -> PendingPickerOperation? {
    dispatchPrecondition(condition: .onQueue(.main))
    defer {
      pendingPickerOperation = nil
    }
    return pendingPickerOperation
  }

  private func resolvePickerCancellation(
    _ operation: PendingPickerOperation
  ) {
    switch operation {
    case .importing(let invoke):
      invoke.resolve(
        PickResponse(
          selected: false,
          path: nil,
          displayName: nil,
          sizeBytes: nil
        )
      )
    case .preparingExport(let invoke):
      invoke.resolve(
        SaveContentSourceResponse(
          selected: false,
          displayName: nil,
          sizeBytes: nil,
          sha256: nil
        )
      )
    case .exporting(let invoke, let export):
      workQueues.scheduleStaging {
        if let storage = try? self.storage.get() {
          // Cancellation is neutral. Startup cleanup is the durable fallback
          // if this best-effort app-private alias cleanup fails.
          try? storage.exporter.cleanup(export)
        }
        invoke.resolve(
          SaveContentSourceResponse(
            selected: false,
            displayName: nil,
            sizeBytes: nil,
            sha256: nil
          )
        )
      }
    }
  }

  private func finishImport(_ invoke: Invoke, selectedURL: URL) {
    workQueues.scheduleStaging {
      do {
        let storage = try self.storage.get()
        let staged = try storage.stager.stage(
          securityScopedURL: selectedURL
        )
        invoke.resolve(
          PickResponse(
            selected: true,
            path: staged.path,
            displayName: staged.displayName,
            sizeBytes: staged.sizeBytes
          )
        )
      } catch ImportStagingError.selectedFileTooLarge {
        invoke.reject(
          "selected file is too large",
          code: "selected_file_too_large"
        )
      } catch {
        invoke.reject(
          "file selection failed",
          code: "selection_failed"
        )
      }
    }
  }

  private func finishContentSourceExport(
    _ invoke: Invoke,
    export: NativeContentSourceExport,
    selectedURL: URL
  ) {
    workQueues.scheduleStaging {
      do {
        let storage = try self.storage.get()
        // Re-open and re-hash the app-private CAS object after the
        // picker delay before accepting the system-managed copy.
        try storage.exporter.revalidateSource(export)
        let displayName = try storage.exporter.verifySavedCopy(
          url: selectedURL,
          export: export
        )
        try storage.exporter.cleanup(export)
        invoke.resolve(
          SaveContentSourceResponse(
            selected: true,
            displayName: displayName,
            sizeBytes: export.expectedSizeBytes,
            sha256: export.expectedSha256
          )
        )
      } catch {
        if let storage = try? self.storage.get() {
          try? storage.exporter.cleanup(export)
        }
        // The destination is user-owned and may represent a provider
        // replacement. Never delete it on an uncertain callback.
        invoke.reject(
          "content source export failed",
          code: "storage_unavailable"
        )
      }
    }
  }

  private func beginSensitiveCapture(_ invoke: Invoke) -> Bool {
    dispatchPrecondition(condition: .onQueue(.main))
    guard
      UIApplication.shared.applicationState == .active,
      manager.viewController?.viewIfLoaded?.window != nil
    else {
      invoke.reject(
        "foreground interaction required",
        code: "permission_denied"
      )
      return false
    }
    guard !sensitiveCaptureInFlight else {
      invoke.reject("sensitive capture is busy", code: "busy")
      return false
    }
    sensitiveCaptureInFlight = true
    return true
  }

  private func finishSensitiveCapture() {
    dispatchPrecondition(condition: .onQueue(.main))
    sensitiveCaptureInFlight = false
  }

  private func foregroundClipboardText() throws -> (String, Int) {
    dispatchPrecondition(condition: .onQueue(.main))
    guard
      UIApplication.shared.applicationState == .active,
      manager.viewController?.viewIfLoaded?.window != nil
    else {
      throw PlatformPolicyError.invalidCredential
    }
    let pasteboard = UIPasteboard.general
    let changeCount = pasteboard.changeCount
    guard
      pasteboard.numberOfItems == 1,
      let value = pasteboard.string,
      !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    else {
      throw PlatformPolicyError.invalidCredential
    }
    return (value, changeCount)
  }

  private func clearClipboardIfUnchanged(
    _ expected: String,
    changeCount: Int
  ) -> ClipboardCleanupStatus {
    dispatchPrecondition(condition: .onQueue(.main))
    let pasteboard = UIPasteboard.general
    let current = pasteboard.string
    guard
      pasteboard.numberOfItems == 1,
      current == expected,
      pasteboard.changeCount == changeCount
    else {
      return .alreadyReplaced
    }
    pasteboard.items = []
    return pasteboard.numberOfItems == 0 || pasteboard.string == nil
      ? .cleared
      : .clearFailed
  }

  private func rejectCredential(_ invoke: Invoke, error: Error) {
    let code = credentialRejectionCode(for: error)
    invoke.reject("credential unavailable", code: code)
  }
}

func credentialRejectionCode(for error: Error) -> String {
  guard let storeError = error as? KeychainCredentialStoreError else {
    return "credential_unavailable"
  }
  switch storeError {
  case .boundRecoveryRequired, .restoreFailed:
    return "credential_recovery_required"
  case .invalidData, .operationFailed, .verificationFailed:
    return "credential_unavailable"
  }
}

@_cdecl("init_plugin_lorepia_platform")
func initPlugin() -> Plugin {
  LorepiaPlatformPlugin()
}
