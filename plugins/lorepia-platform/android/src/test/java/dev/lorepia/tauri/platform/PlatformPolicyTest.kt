package dev.lorepia.tauri.platform

import java.io.ByteArrayOutputStream
import java.security.MessageDigest
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class PlatformPolicyTest {
    @Test
    fun credentialConfirmationTextRejectsPromptSpoofingControls() {
        for (
            invalid in listOf(
                "",
                "   ",
                "connection\nApprove",
                "connection\u0000Approve",
                "connection\u2028Approve",
                "connection\u2029Approve",
                "connection\u202eApprove",
                "connection\u2066Approve",
                "connection\u2069Approve",
                "connection\u200bApprove",
                "connection\u200dApprove",
                "connection\u2060Approve",
                "connection\ufeffApprove",
                "connection\u00adApprove",
                "connection\u034fApprove",
                "connection\udb40\udc01Approve",
            )
        ) {
            assertThrows(IllegalArgumentException::class.java) {
                PlatformPolicy.validateCredentialConfirmationText(invalid, 256)
            }
        }

        PlatformPolicy.validateCredentialConfirmationText("연결 a", 256)
    }

    @Test
    fun credentialNamesAreDeterministicLowercaseHashes() {
        val first = PlatformPolicy.credentialFileName("synthetic-profile")
        val second = PlatformPolicy.credentialFileName("synthetic-profile")

        assertEquals(first, second)
        assertTrue(first.matches(Regex("[0-9a-f]{64}\\.credential")))
        assertFalse(first.contains("synthetic-profile"))
    }

    @Test
    fun sensitiveInputLimitsUseUtf8Bytes() {
        PlatformPolicy.validateReference("가".repeat(85))
        assertThrows(IllegalArgumentException::class.java) {
            PlatformPolicy.validateReference("가".repeat(86))
        }

        val maximum = PlatformPolicy.validateCredentialForWrite(
            "a".repeat(PlatformPolicy.MAXIMUM_CREDENTIAL_WRITE_BYTES),
        )
        maximum.fill(0)
        val opaqueCredential = "  secret\n"
        val opaqueBytes = PlatformPolicy.validateCredentialForWrite(opaqueCredential)
        assertArrayEquals(opaqueCredential.toByteArray(Charsets.UTF_8), opaqueBytes)
        opaqueBytes.fill(0)
        assertThrows(IllegalArgumentException::class.java) {
            PlatformPolicy.validateCredentialForWrite(
                "a".repeat(PlatformPolicy.MAXIMUM_CREDENTIAL_WRITE_BYTES + 1),
            )
        }

        val captured = PlatformPolicy.validateSensitiveCapture(
            "curl https://example.test",
            1_024,
        )
        assertEquals("curl https://example.test".toByteArray().size, captured.size)
        captured.fill(0)
        assertThrows(IllegalArgumentException::class.java) {
            PlatformPolicy.validateSensitiveCapture("", 1_024)
        }
        assertThrows(IllegalArgumentException::class.java) {
            PlatformPolicy.validateSensitiveCapture(
                "x",
                PlatformPolicy.MAXIMUM_SENSITIVE_CAPTURE_BYTES.toLong() + 1,
            )
        }
    }

    @Test
    fun displayNamesAreBoundedAndExtensionsAreAllowlisted() {
        val sanitized = PlatformPolicy.sanitizeDisplayName(
            "card\u0000.${"x".repeat(300)}",
        )
        assertFalse(sanitized.contains('\u0000'))
        assertEquals(PlatformPolicy.MAXIMUM_DISPLAY_NAME_CHARACTERS, sanitized.length)
        assertEquals(".charx", PlatformPolicy.stagingSuffix("card.CHARX"))
        assertEquals(".pending", PlatformPolicy.stagingSuffix("card.html"))
    }

    @Test
    fun contentSourceExportRequiresExactCasIdentityAndPortableName() {
        val root = kotlin.io.path.createTempDirectory("lorepia-export-policy")
            .toFile()
            .canonicalFile
        try {
            val bytes = "synthetic-lossless-source".toByteArray()
            val digest = MessageDigest.getInstance("SHA-256")
                .digest(bytes)
                .joinToString("") { "%02x".format(it.toInt() and 0xff) }
            val source = root
                .resolve("sources")
                .resolve("sha256")
                .resolve(digest.take(2))
                .resolve(digest.drop(2))
            assertTrue(source.parentFile.mkdirs())
            source.writeBytes(bytes)

            val verified = PlatformPolicy.validateContentSourceExport(
                dataRoot = root,
                sourcePath = source.absolutePath,
                suggestedName = "lorepia-character-test.json",
                expectedSizeBytes = bytes.size.toLong(),
                expectedSha256 = digest,
            )
            assertEquals(source, verified)
            val output = ByteArrayOutputStream()
            PlatformPolicy.copyVerifiedContentSource(
                verified,
                output,
                bytes.size.toLong(),
                digest,
            )
            assertTrue(bytes.contentEquals(output.toByteArray()))

            assertThrows(IllegalArgumentException::class.java) {
                PlatformPolicy.validateContentSourceExport(
                    root,
                    root.resolve("outside.json").absolutePath,
                    "card.json",
                    bytes.size.toLong(),
                    digest,
                )
            }
            assertThrows(IllegalArgumentException::class.java) {
                PlatformPolicy.validateExportSuggestedName("../card.json")
            }
            assertThrows(IllegalArgumentException::class.java) {
                PlatformPolicy.validateExportSuggestedName("NUL.json")
            }
            assertThrows(IllegalArgumentException::class.java) {
                PlatformPolicy.validateExportSha256(digest.uppercase())
            }
            assertEquals(
                "캐릭터.json",
                PlatformPolicy.validateExportReceiptDisplayName("캐릭터.json"),
            )
            assertThrows(IllegalArgumentException::class.java) {
                PlatformPolicy.validateExportReceiptDisplayName("folder/card.json")
            }
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun abandonedStagingCleanupRequiresOwnedOldRegularFile() {
        val now = 2L * PlatformPolicy.ABANDONED_STAGING_AGE_MILLIS
        val old = now - PlatformPolicy.ABANDONED_STAGING_AGE_MILLIS
        assertTrue(
            PlatformPolicy.shouldRemoveAbandonedStagingFile(
                name = "${PlatformPolicy.OWNED_STAGING_PREFIX}synthetic.json",
                isRegularFile = true,
                lastModifiedMillis = old,
                nowMillis = now,
            ),
        )
        assertFalse(
            PlatformPolicy.shouldRemoveAbandonedStagingFile(
                name = "unrelated.json",
                isRegularFile = true,
                lastModifiedMillis = old,
                nowMillis = now,
            ),
        )
        assertFalse(
            PlatformPolicy.shouldRemoveAbandonedStagingFile(
                name = "${PlatformPolicy.OWNED_STAGING_PREFIX}fresh.json",
                isRegularFile = true,
                lastModifiedMillis = old + 1L,
                nowMillis = now,
            ),
        )
        assertFalse(
            PlatformPolicy.shouldRemoveAbandonedStagingFile(
                name = "${PlatformPolicy.OWNED_STAGING_PREFIX}directory",
                isRegularFile = false,
                lastModifiedMillis = old,
                nowMillis = now,
            ),
        )
    }

    @Test
    fun stagedImportDescriptionRedactsPathAndDisplayName() {
        val path = "/synthetic/private/card.json"
        val displayName = "private-card.json"
        val rendered = NativeStagedImport(
            path = path,
            displayName = displayName,
            sizeBytes = 42,
        ).toString()

        assertFalse(rendered.contains(path))
        assertFalse(rendered.contains(displayName))
        assertTrue(rendered.contains("[REDACTED]"))
    }
}
