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

  static func validateCredentialConfirmationText(
    _ value: String,
    maximumBytes: Int
  ) throws {
    guard
      !value.isEmpty,
      value.utf8.count <= maximumBytes,
      value.unicodeScalars.contains(where: { $0.value != 0x20 }),
      value.unicodeScalars.allSatisfy({ scalar in
        !isCredentialConfirmationSpoofingCodePoint(scalar.value)
      })
    else {
      throw PlatformPolicyError.invalidCredential
    }
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

  private static func isCredentialConfirmationSpoofingCodePoint(
    _ codePoint: UInt32
  ) -> Bool {
    (0x0000...0x001F).contains(codePoint)
      || (0x007F...0x00A0).contains(codePoint)
      || codePoint == 0x00AD
      || codePoint == 0x034F
      || (0x0600...0x0605).contains(codePoint)
      || codePoint == 0x061C
      || codePoint == 0x06DD
      || codePoint == 0x070F
      || (0x0890...0x0891).contains(codePoint)
      || codePoint == 0x08E2
      || (0x115F...0x1160).contains(codePoint)
      || codePoint == 0x1680
      || (0x17B4...0x17B5).contains(codePoint)
      || (0x180B...0x180F).contains(codePoint)
      || (0x2000...0x200F).contains(codePoint)
      || (0x2028...0x202F).contains(codePoint)
      || (0x205F...0x206F).contains(codePoint)
      || codePoint == 0x3000
      || codePoint == 0x3164
      || (0xFE00...0xFE0F).contains(codePoint)
      || codePoint == 0xFEFF
      || codePoint == 0xFFA0
      || (0xFFF0...0xFFFB).contains(codePoint)
      || codePoint == 0x110BD
      || codePoint == 0x110CD
      || (0x13430...0x13455).contains(codePoint)
      || (0x1BCA0...0x1BCA3).contains(codePoint)
      || (0x1D173...0x1D17A).contains(codePoint)
      || (0xE0000...0xE0FFF).contains(codePoint)
  }
}
