package dev.lorepia.tauri.platform

import java.io.File
import java.io.FileInputStream
import java.io.InputStream
import java.io.OutputStream
import java.nio.file.Files
import java.nio.file.LinkOption
import java.security.MessageDigest

internal object PlatformPolicy {
    const val MAXIMUM_REFERENCE_BYTES = 256
    const val MAXIMUM_CREDENTIAL_READ_BYTES = 32 * 1024
    const val MAXIMUM_CREDENTIAL_WRITE_BYTES = 16 * 1024
    const val MAXIMUM_SENSITIVE_CAPTURE_BYTES = 1024 * 1024
    const val MAXIMUM_IMPORT_BYTES = 50L * 1024L * 1024L
    const val COPY_BUFFER_BYTES = 64 * 1024
    const val MAXIMUM_DISPLAY_NAME_CHARACTERS = 255
    const val MAXIMUM_EXPORT_NAME_BYTES = 128
    const val OWNED_STAGING_PREFIX = "lorepia-tauri-"
    const val ABANDONED_STAGING_AGE_MILLIS = 24L * 60L * 60L * 1_000L

    fun validateReference(reference: String) {
        require(reference.isNotBlank()) { "invalid reference" }
        val encoded = reference.toByteArray(Charsets.UTF_8)
        try {
            require(encoded.size <= MAXIMUM_REFERENCE_BYTES) { "invalid reference" }
        } finally {
            encoded.fill(0)
        }
    }

    fun validateCredentialForWrite(value: String): ByteArray {
        require(value.isNotBlank()) { "invalid credential" }
        return value.toByteArray(Charsets.UTF_8).also {
            require(it.size <= MAXIMUM_CREDENTIAL_WRITE_BYTES) {
                it.fill(0)
                "invalid credential"
            }
        }
    }

    fun validateCredentialForRead(value: ByteArray) {
        require(value.isNotEmpty() && value.size <= MAXIMUM_CREDENTIAL_READ_BYTES) {
            "invalid credential"
        }
    }

    fun validateSensitiveCapture(value: String, maximumBytes: Long): ByteArray {
        require(maximumBytes in 1..MAXIMUM_SENSITIVE_CAPTURE_BYTES.toLong()) {
            "invalid sensitive capture limit"
        }
        require(value.isNotBlank()) { "empty clipboard" }
        return value.toByteArray(Charsets.UTF_8).also {
            require(it.size.toLong() <= maximumBytes) {
                it.fill(0)
                "sensitive capture too large"
            }
        }
    }

    fun validateContentSourceExport(
        dataRoot: File,
        sourcePath: String,
        suggestedName: String,
        expectedSizeBytes: Long,
        expectedSha256: String,
    ): File {
        validateExportSuggestedName(suggestedName)
        validateExportSha256(expectedSha256)
        require(expectedSizeBytes > 0) { "invalid export size" }

        val canonicalRoot = dataRoot.canonicalFile
        val expected = canonicalRoot
            .resolve("sources")
            .resolve("sha256")
            .resolve(expectedSha256.take(2))
            .resolve(expectedSha256.drop(2))
            .absoluteFile
        val source = File(sourcePath).absoluteFile
        require(source == expected) { "source is outside content CAS" }
        require(expected.canonicalFile == expected) { "content CAS contains a link" }
        require(
            Files.isRegularFile(source.toPath(), LinkOption.NOFOLLOW_LINKS) &&
                !Files.isSymbolicLink(source.toPath()) &&
                source.length() == expectedSizeBytes,
        ) { "invalid content source" }
        val (actualSha256, actualSizeBytes) = hashFile(source)
        require(actualSha256 == expectedSha256 && actualSizeBytes == expectedSizeBytes) {
            "content source identity changed"
        }
        return source
    }

    fun copyVerifiedContentSource(
        source: File,
        destination: OutputStream,
        expectedSizeBytes: Long,
        expectedSha256: String,
    ) {
        val digest = MessageDigest.getInstance("SHA-256")
        var copied = 0L
        FileInputStream(source).use { input ->
            val buffer = ByteArray(COPY_BUFFER_BYTES)
            try {
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    if (read == 0) continue
                    copied = Math.addExact(copied, read.toLong())
                    require(copied <= expectedSizeBytes) { "content source grew" }
                    digest.update(buffer, 0, read)
                    destination.write(buffer, 0, read)
                }
            } finally {
                buffer.fill(0)
            }
        }
        destination.flush()
        val actualSha256 = digestToHex(digest.digest())
        require(copied == expectedSizeBytes && actualSha256 == expectedSha256) {
            "content source identity changed"
        }
    }

    fun verifyExportedContent(
        source: InputStream,
        expectedSizeBytes: Long,
        expectedSha256: String,
    ) {
        val digest = MessageDigest.getInstance("SHA-256")
        var total = 0L
        val buffer = ByteArray(COPY_BUFFER_BYTES)
        try {
            while (true) {
                val read = source.read(buffer)
                if (read < 0) break
                if (read == 0) continue
                total = Math.addExact(total, read.toLong())
                require(total <= expectedSizeBytes) { "saved content grew" }
                digest.update(buffer, 0, read)
            }
        } finally {
            buffer.fill(0)
        }
        require(total == expectedSizeBytes && digestToHex(digest.digest()) == expectedSha256) {
            "saved content identity changed"
        }
    }

    fun validateExportReceiptDisplayName(value: String): String {
        require(
            value.isNotBlank() &&
                value != "." &&
                value != ".." &&
                value.length <= MAXIMUM_DISPLAY_NAME_CHARACTERS &&
                value.none { it.isISOControl() || it == '/' || it == '\\' },
        ) { "invalid saved display name" }
        return value
    }

    fun validateExportSha256(value: String) {
        require(value.matches(Regex("[0-9a-f]{64}"))) { "invalid export digest" }
    }

    fun validateExportSuggestedName(value: String) {
        val encoded = value.toByteArray(Charsets.UTF_8)
        try {
            require(encoded.isNotEmpty() && encoded.size <= MAXIMUM_EXPORT_NAME_BYTES) {
                "invalid export name"
            }
            require(
                value.all { it.isAsciiLetterOrDigit() || it == '.' || it == '_' || it == '-' } &&
                    !value.startsWith('.') &&
                    !value.endsWith('.') &&
                    !value.contains(".."),
            ) { "invalid export name" }
            val stem = value.substringBefore('.').uppercase()
            require(stem !in WINDOWS_RESERVED_STEMS) { "invalid export name" }
        } finally {
            encoded.fill(0)
        }
    }

    fun credentialFileName(reference: String): String {
        val referenceBytes = reference.toByteArray(Charsets.UTF_8)
        val digest = try {
            MessageDigest.getInstance("SHA-256").digest(referenceBytes)
        } finally {
            referenceBytes.fill(0)
        }
        return try {
            buildString(digest.size * 2 + CREDENTIAL_SUFFIX.length) {
                for (byte in digest) {
                    append(HEX[(byte.toInt() ushr 4) and 0x0f])
                    append(HEX[byte.toInt() and 0x0f])
                }
                append(CREDENTIAL_SUFFIX)
            }
        } finally {
            digest.fill(0)
        }
    }

    fun sanitizeDisplayName(value: String?): String {
        val sanitized = value
            ?.take(MAXIMUM_DISPLAY_NAME_CHARACTERS)
            ?.map { character -> if (character.isISOControl()) '\uFFFD' else character }
            ?.joinToString(separator = "")
            ?.takeIf(String::isNotBlank)
        return sanitized ?: "selected-file"
    }

    fun stagingSuffix(displayName: String): String =
        when (displayName.substringAfterLast('.', "").lowercase()) {
            "charx" -> ".charx"
            "json" -> ".json"
            "zip" -> ".zip"
            else -> ".pending"
        }

    fun shouldRemoveAbandonedStagingFile(
        name: String,
        isRegularFile: Boolean,
        lastModifiedMillis: Long,
        nowMillis: Long,
    ): Boolean =
        name.startsWith(OWNED_STAGING_PREFIX) &&
            isRegularFile &&
            lastModifiedMillis > 0L &&
            nowMillis >= lastModifiedMillis &&
            nowMillis - lastModifiedMillis >= ABANDONED_STAGING_AGE_MILLIS

    private const val CREDENTIAL_SUFFIX = ".credential"
    private const val HEX = "0123456789abcdef"
    private val WINDOWS_RESERVED_STEMS = buildSet {
        addAll(listOf("CON", "PRN", "AUX", "NUL"))
        for (index in 1..9) {
            add("COM$index")
            add("LPT$index")
        }
    }

    private fun Char.isAsciiLetterOrDigit(): Boolean =
        this in 'a'..'z' || this in 'A'..'Z' || this in '0'..'9'

    private fun hashFile(file: File): Pair<String, Long> {
        val digest = MessageDigest.getInstance("SHA-256")
        var total = 0L
        FileInputStream(file).use { input ->
            val buffer = ByteArray(COPY_BUFFER_BYTES)
            try {
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    if (read == 0) continue
                    total = Math.addExact(total, read.toLong())
                    digest.update(buffer, 0, read)
                }
            } finally {
                buffer.fill(0)
            }
        }
        return digestToHex(digest.digest()) to total
    }

    private fun digestToHex(digest: ByteArray): String =
        try {
            buildString(digest.size * 2) {
                for (byte in digest) {
                    append(HEX[(byte.toInt() ushr 4) and 0x0f])
                    append(HEX[byte.toInt() and 0x0f])
                }
            }
        } finally {
            digest.fill(0)
        }
}
