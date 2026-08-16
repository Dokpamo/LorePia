import Foundation

enum PlatformPolicyError: Error {
  case invalidCredential
  case invalidExport
  case invalidReference
}

enum PlatformPolicy {
  static let maximumReferenceBytes = 256
  static let maximumCredentialReadBytes = 32 * 1_024
  static let maximumCredentialWriteBytes = 16 * 1_024
  static let maximumSensitiveCaptureBytes: UInt64 = 1_024 * 1_024
  static let maximumImportBytes: UInt64 = 128 * 1_024 * 1_024
  static let copyBufferBytes = 64 * 1_024
  static let maximumDisplayNameCharacters = 255
  static let maximumExportNameBytes = 128
  static let ownedStagingPrefix = "lorepia-tauri-"
  static let abandonedStagingAge: TimeInterval = 24 * 60 * 60

  static func validateReference(_ reference: String) throws {
    guard
      !reference.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
      reference.utf8.count <= maximumReferenceBytes
    else {
      throw PlatformPolicyError.invalidReference
    }
  }

  static func validateCredentialForWrite(_ value: String) throws -> String {
    guard
      !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
      value.utf8.count <= maximumCredentialWriteBytes
    else {
      throw PlatformPolicyError.invalidCredential
    }
    // Credentials are opaque provider bytes. Validation must never normalize
    // leading or trailing whitespace because doing so can change their
    // authentication meaning and would diverge from Rust and Android.
    return value
  }

  static func validateCredentialForRead(_ value: String) throws -> String {
    guard
      !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
      value.utf8.count <= maximumCredentialReadBytes
    else {
      throw PlatformPolicyError.invalidCredential
    }
    return value
  }

  static func validateSensitiveCapture(
    _ value: String,
    maximumBytes: UInt64
  ) throws -> Data {
    guard
      maximumBytes > 0,
      maximumBytes <= maximumSensitiveCaptureBytes,
      !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    else {
      throw PlatformPolicyError.invalidCredential
    }
    let data = Data(value.utf8)
    guard UInt64(data.count) <= maximumBytes else {
      throw PlatformPolicyError.invalidCredential
    }
    return data
  }

  static func validateExportSha256(_ value: String) throws {
    guard
      value.utf8.count == 64,
      value.utf8.allSatisfy({ byte in
        (48...57).contains(byte) || (97...102).contains(byte)
      })
    else {
      throw PlatformPolicyError.invalidExport
    }
  }

  static func validateExportSuggestedName(_ value: String) throws {
    let utf8 = Array(value.utf8)
    let portable =
      !utf8.isEmpty
      && utf8.count <= maximumExportNameBytes
      && utf8.allSatisfy { byte in
        (48...57).contains(byte)
          || (65...90).contains(byte)
          || (97...122).contains(byte)
          || byte == 45
          || byte == 46
          || byte == 95
      }
    let stem =
      value.split(separator: ".", maxSplits: 1)
      .first
      .map(String.init)?
      .uppercased() ?? ""
    guard
      portable,
      !value.hasPrefix("."),
      !value.hasSuffix("."),
      !value.contains(".."),
      !windowsReservedStems.contains(stem)
    else {
      throw PlatformPolicyError.invalidExport
    }
  }

  static func validateExportReceiptDisplayName(
    _ value: String
  ) throws -> String {
    guard
      !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
      value != ".",
      value != "..",
      value.count <= maximumDisplayNameCharacters,
      !value.unicodeScalars.contains(where: { scalar in
        CharacterSet.controlCharacters.contains(scalar)
          || scalar == "/"
          || scalar == "\\"
      })
    else {
      throw PlatformPolicyError.invalidExport
    }
    return value
  }

  static func sanitizeDisplayName(_ value: String?) -> String {
    guard let value else {
      return "selected-file"
    }
    let scalars = value.unicodeScalars.map { scalar -> Character in
      CharacterSet.controlCharacters.contains(scalar)
        ? "\u{FFFD}"
        : Character(String(scalar))
    }
    let sanitized = String(scalars.prefix(maximumDisplayNameCharacters))
    return sanitized.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
      ? "selected-file"
      : sanitized
  }

  static func stagingSuffix(for displayName: String) -> String {
    switch (displayName as NSString).pathExtension.lowercased() {
    case "charx":
      ".charx"
    case "json":
      ".json"
    case "zip":
      ".zip"
    default:
      ".pending"
    }
  }

  static func shouldRemoveAbandonedStagingFile(
    name: String,
    isRegularFile: Bool,
    modifiedAt: Date?,
    now: Date
  ) -> Bool {
    guard
      name.hasPrefix(ownedStagingPrefix),
      isRegularFile,
      let modifiedAt,
      now >= modifiedAt
    else {
      return false
    }
    return now.timeIntervalSince(modifiedAt) >= abandonedStagingAge
  }

  private static let windowsReservedStems: Set<String> = {
    var values: Set<String> = ["CON", "PRN", "AUX", "NUL"]
    for index in 1...9 {
      values.insert("COM\(index)")
      values.insert("LPT\(index)")
    }
    return values
  }()
}
