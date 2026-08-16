package dev.lorepia.tauri.platform

import android.util.AtomicFile
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class AtomicCredentialFileReaderTest {
    @Test
    fun boundDeleteBeforeFirstDirectoryCreationSyncsExistingGrandparent() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "bound-delete-before-directory-${System.nanoTime()}")
        check(root.mkdir())
        try {
            val directory = root.resolve("provider-credentials")
            val target = directory.resolve("synthetic-bound.credential")

            BoundCredentialFileDeleter.deleteBaseOnly(target)

            assertFalse(directory.exists())
            assertFalse(target.exists())
            assertFalse(File("${target.path}.bak").exists())
            assertFalse(File("${target.path}.new").exists())
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun boundPublicationAndDeleteUseRealAndroidDurabilityPrimitives() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "bound-durability-${System.nanoTime()}")
        check(root.mkdir())
        try {
            val directory = root.resolve("provider-credentials")
            val target = directory.resolve("synthetic-bound.credential")
            val expected = "synthetic-encrypted-envelope".toByteArray()

            BoundCredentialDirectory.ensureDurable(directory)
            BoundCredentialFilePublisher.publishAddOnly(
                target = target,
                encoded = expected,
                verify = {
                    BoundCredentialFileReader.readBaseOnly(
                        target = target,
                        maximumBytes = 1024,
                    )?.contentEquals(expected) == true
                },
            )

            assertArrayEquals(expected, target.readBytes())
            BoundCredentialFileDeleter.deleteBaseOnly(target)
            assertFalse(target.exists())
            assertTrue(directory.isDirectory)
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun genericRawReadRetainsAtomicFileBackupRecovery() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "atomic-credential-reader-${System.nanoTime()}")
        check(root.mkdir())
        try {
            val base = root.resolve("legacy-reference.credential")
            val backup = File("${base.path}.bak")
            val pending = File("${base.path}.new")
            val currentBytes = "current-incomplete-record".toByteArray()
            val backupBytes = "last-committed-legacy-record".toByteArray()
            base.writeBytes(currentBytes)
            backup.writeBytes(backupBytes)
            pending.writeBytes("pending-legacy-record".toByteArray())

            val observed = AtomicCredentialFileReader.readEncoded(
                AtomicFile(base),
                maximumBytes = 1024,
            )

            assertArrayEquals(backupBytes, observed)
            assertArrayEquals(backupBytes, base.readBytes())
            assertFalse(backup.exists())
            assertFalse(pending.exists())
        } finally {
            root.deleteRecursively()
        }
    }
}
