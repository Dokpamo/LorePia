package dev.lorepia.tauri.platform

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.system.Os
import android.system.OsConstants
import android.util.AtomicFile
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.File
import java.io.FileNotFoundException
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.charset.CodingErrorAction
import java.nio.channels.FileChannel
import java.nio.file.FileAlreadyExistsException
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.NoSuchFileException
import java.nio.file.StandardOpenOption
import java.nio.file.attribute.BasicFileAttributes
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

internal enum class NativeCredentialStatus(val wireValue: String) {
    MISSING("missing"),
    AVAILABLE("available"),
    UNREADABLE("unreadable"),
}

internal class CredentialRecoveryRequiredException(cause: Throwable) :
    Exception("credential recovery required", cause)

private fun forceCredentialDirectory(directory: File) {
    val descriptor = Os.open(
        directory.absolutePath,
        OsConstants.O_RDONLY or OsConstants.O_NOFOLLOW,
        0,
    )
    try {
        Os.fsync(descriptor)
    } finally {
        Os.close(descriptor)
    }
}

/** Creates and durably publishes the bound credential directory itself. */
internal object BoundCredentialDirectory {
    fun ensureDurable(
        directory: File,
        createDirectory: (File) -> Unit = { Files.createDirectory(it.toPath()) },
        syncDirectory: (File) -> Unit = ::forceCredentialDirectory,
    ) {
        try {
            val parent = directory.parentFile
                ?: throw IOException("credential directory parent is unavailable")
            requireDirectory(parent)
            val exists = Files.exists(directory.toPath(), LinkOption.NOFOLLOW_LINKS)
            if (!exists) {
                try {
                    createDirectory(directory)
                } catch (_: FileAlreadyExistsException) {
                    // A racing creator must still pass the exact validation and
                    // durability sequence below.
                }
            }
            requireDirectory(directory)
            syncDirectory(directory)
            syncDirectory(parent)
        } catch (error: CredentialRecoveryRequiredException) {
            throw error
        } catch (error: Exception) {
            throw CredentialRecoveryRequiredException(error)
        }
    }

    private fun requireDirectory(directory: File) {
        val attributes = Files.readAttributes(
            directory.toPath(),
            BasicFileAttributes::class.java,
            LinkOption.NOFOLLOW_LINKS,
        )
        if (!attributes.isDirectory) {
            throw IOException("credential directory is not a directory")
        }
    }
}

/**
 * Publishes one authority-bound encrypted record with CREATE_NEW semantics.
 *
 * Once the caller's durable operation is Started, any existing file or
 * uncertain publication is recovery evidence. This helper therefore never
 * replaces or removes the target, including after verification failure.
 */
internal object BoundCredentialFilePublisher {
    fun publishAddOnly(
        target: File,
        encoded: ByteArray,
        syncDirectory: (File) -> Unit = ::forceCredentialDirectory,
        verify: () -> Boolean,
    ) {
        try {
            FileChannel.open(
                target.toPath(),
                StandardOpenOption.CREATE_NEW,
                StandardOpenOption.WRITE,
            ).use { channel ->
                val source = ByteBuffer.wrap(encoded)
                while (source.hasRemaining()) {
                    if (channel.write(source) <= 0) {
                        throw IOException("credential publication made no progress")
                    }
                }
                channel.force(true)
            }
            syncDirectory(
                target.parentFile
                    ?: throw IOException("credential directory is unavailable"),
            )
            if (!verify()) {
                throw IOException("credential publication could not be verified")
            }
        } catch (error: CredentialRecoveryRequiredException) {
            throw error
        } catch (error: Exception) {
            throw CredentialRecoveryRequiredException(error)
        }
    }

}

/**
 * Reads only the final authority-bound credential file.
 *
 * `AtomicFile.openRead()` is deliberately forbidden here because it can rename
 * a stale `.bak` over the final file or delete `.new` while merely observing a
 * durable operation. A missing final file is Missing even when abandoned
 * sidecars exist. If the final file exists, any sidecar makes the state
 * ambiguous and therefore recovery-required without mutating any path.
 */
internal object BoundCredentialFileReader {
    fun readBaseOnly(target: File, maximumBytes: Long): ByteArray? {
        require(maximumBytes in 1..Int.MAX_VALUE.toLong()) {
            "invalid credential read limit"
        }
        try {
            if (!requireRegularBase(target, missingIsAllowed = true)) {
                return null
            }
            requireNoAtomicFileSidecars(target)

            val result = FileChannel.open(
                target.toPath(),
                StandardOpenOption.READ,
                LinkOption.NOFOLLOW_LINKS,
            ).use { channel ->
                ByteArrayOutputStream().use { output ->
                    val buffer = ByteBuffer.allocate(BOUND_READ_BUFFER_BYTES)
                    try {
                        var total = 0L
                        while (true) {
                            buffer.clear()
                            val count = channel.read(buffer)
                            if (count < 0) {
                                break
                            }
                            if (count == 0) {
                                throw IOException("credential read made no progress")
                            }
                            total = Math.addExact(total, count.toLong())
                            if (total > maximumBytes) {
                                throw IOException("credential record is too large")
                            }
                            output.write(buffer.array(), 0, count)
                        }
                        output.toByteArray()
                    } finally {
                        buffer.array().fill(0)
                    }
                }
            }

            try {
                check(requireRegularBase(target, missingIsAllowed = false))
                requireNoAtomicFileSidecars(target)
            } catch (error: Exception) {
                result.fill(0)
                throw error
            }
            return result
        } catch (error: CredentialRecoveryRequiredException) {
            throw error
        } catch (error: Exception) {
            throw CredentialRecoveryRequiredException(error)
        }
    }

    private fun requireRegularBase(target: File, missingIsAllowed: Boolean): Boolean {
        val attributes = try {
            Files.readAttributes(
                target.toPath(),
                BasicFileAttributes::class.java,
                LinkOption.NOFOLLOW_LINKS,
            )
        } catch (_: NoSuchFileException) {
            if (missingIsAllowed) {
                return false
            }
            throw IOException("credential disappeared during observation")
        }
        if (!attributes.isRegularFile) {
            throw IOException("credential target is not a regular file")
        }
        return true
    }

    private fun requireNoAtomicFileSidecars(target: File) {
        for (suffix in ATOMIC_FILE_SIDECAR_SUFFIXES) {
            val sidecar = File("${target.path}$suffix")
            try {
                Files.readAttributes(
                    sidecar.toPath(),
                    BasicFileAttributes::class.java,
                    LinkOption.NOFOLLOW_LINKS,
                )
                throw IOException("credential sidecar state is ambiguous")
            } catch (_: NoSuchFileException) {
                // Absence is the only safe sidecar state for a bound record.
            }
        }
    }

    private const val BOUND_READ_BUFFER_BYTES = 8 * 1024
    private val ATOMIC_FILE_SIDECAR_SUFFIXES = arrayOf(".bak", ".new")
}

/** Keeps the legacy/raw path on Android's state-recovering AtomicFile read. */
internal object AtomicCredentialFileReader {
    fun readEncoded(file: AtomicFile, maximumBytes: Long): ByteArray {
        require(maximumBytes in 1..Int.MAX_VALUE.toLong()) {
            "invalid credential read limit"
        }
        return file.openRead().use { input ->
            ByteArrayOutputStream().use { output ->
                val buffer = ByteArray(RECORD_READ_BUFFER_BYTES)
                var total = 0L
                try {
                    while (true) {
                        val count = input.read(buffer)
                        if (count < 0) {
                            break
                        }
                        total = Math.addExact(total, count.toLong())
                        check(total <= maximumBytes) {
                            "credential unavailable"
                        }
                        output.write(buffer, 0, count)
                    }
                    output.toByteArray()
                } finally {
                    buffer.fill(0)
                }
            }
        }
    }

    private const val RECORD_READ_BUFFER_BYTES = 8 * 1024
}

/** Deletes only the final bound record and durably publishes its absence. */
internal object BoundCredentialFileDeleter {
    fun deleteBaseOnly(
        target: File,
        unlink: (File) -> Boolean = { Files.deleteIfExists(it.toPath()) },
        syncDirectory: (File) -> Unit = ::forceCredentialDirectory,
    ) {
        try {
            val parent = target.parentFile
                ?: throw IOException("credential directory is unavailable")
            val parentAttributes = readAttributesNoFollowOrNull(parent)
            if (parentAttributes == null) {
                val grandparent = parent.parentFile
                    ?: throw IOException("credential directory parent is unavailable")
                requireDirectory(grandparent)

                // A crash may occur after Core persists Started but before the
                // first bound install creates its directory. Publishing the
                // already-absent result still requires syncing the existing
                // no-backup directory. Never create the missing directory or
                // invoke unlink while repairing that cutpoint.
                syncDirectory(grandparent)
                val parentAppeared = pathExistsNoFollow(parent)
                val targetAppeared = pathExistsNoFollow(target)
                val sidecarAppeared = hasAtomicFileSidecarNoFollow(target)
                if (parentAppeared || targetAppeared || sidecarAppeared) {
                    throw IOException("credential state appeared during absent-directory delete")
                }
                return
            }
            if (!parentAttributes.isDirectory) {
                throw IOException("credential parent is not a directory")
            }
            requireNoAtomicFileSidecars(target)

            val targetAttributes = try {
                Files.readAttributes(
                    target.toPath(),
                    BasicFileAttributes::class.java,
                    LinkOption.NOFOLLOW_LINKS,
                )
            } catch (_: NoSuchFileException) {
                null
            }
            if (targetAttributes != null) {
                if (!targetAttributes.isRegularFile) {
                    throw IOException("credential target is not a regular file")
                }
                if (!unlink(target)) {
                    throw IOException("credential unlink did not remove the target")
                }
            }

            syncDirectory(parent)
            if (pathExistsNoFollow(target)) {
                throw IOException("credential target reappeared after unlink")
            }
            requireNoAtomicFileSidecars(target)
        } catch (error: CredentialRecoveryRequiredException) {
            throw error
        } catch (error: Exception) {
            throw CredentialRecoveryRequiredException(error)
        }
    }

    private fun requireNoAtomicFileSidecars(target: File) {
        if (hasAtomicFileSidecarNoFollow(target)) {
            throw IOException("credential sidecar state is ambiguous")
        }
    }

    private fun hasAtomicFileSidecarNoFollow(target: File): Boolean =
        ATOMIC_FILE_SIDECAR_SUFFIXES.any { suffix ->
            pathExistsNoFollow(File("${target.path}$suffix"))
        }

    private fun requireDirectory(directory: File) {
        val attributes = readAttributesNoFollowOrNull(directory)
            ?: throw IOException("credential directory is unavailable")
        if (!attributes.isDirectory) {
            throw IOException("credential path is not a directory")
        }
    }

    private fun readAttributesNoFollowOrNull(path: File): BasicFileAttributes? = try {
        Files.readAttributes(
            path.toPath(),
            BasicFileAttributes::class.java,
            LinkOption.NOFOLLOW_LINKS,
        )
    } catch (_: NoSuchFileException) {
        null
    }

    private fun pathExistsNoFollow(path: File): Boolean =
        readAttributesNoFollowOrNull(path) != null

    private val ATOMIC_FILE_SIDECAR_SUFFIXES = arrayOf(".bak", ".new")
}

internal class AndroidCredentialStore(context: Context) {
    private val directory = context.noBackupFilesDir
        .resolve(CREDENTIAL_DIRECTORY)
        .absoluteFile

    fun status(reference: String): NativeCredentialStatus {
        PlatformPolicy.validateReference(reference)
        synchronized(PROCESS_LOCK) {
            val file = credentialFile(reference)
            return try {
                val plaintext = readPlaintext(reference, file)
                plaintext.fill(0)
                NativeCredentialStatus.AVAILABLE
            } catch (_: FileNotFoundException) {
                if (file.baseFile.exists()) {
                    NativeCredentialStatus.UNREADABLE
                } else {
                    NativeCredentialStatus.MISSING
                }
            } catch (_: Exception) {
                NativeCredentialStatus.UNREADABLE
            }
        }
    }

    fun read(reference: String): String? {
        PlatformPolicy.validateReference(reference)
        synchronized(PROCESS_LOCK) {
            val file = credentialFile(reference)
            val plaintext = try {
                readPlaintext(reference, file)
            } catch (error: FileNotFoundException) {
                if (file.baseFile.exists()) {
                    throw error
                }
                return null
            }
            return try {
                decodeUtf8(plaintext).also {
                    require(it.isNotBlank()) { "credential unavailable" }
                }
            } finally {
                plaintext.fill(0)
            }
        }
    }

    fun boundStatus(reference: String): NativeCredentialStatus {
        PlatformPolicy.validateReference(reference)
        synchronized(PROCESS_LOCK) {
            val plaintext = readBoundPlaintext(reference) ?: return NativeCredentialStatus.MISSING
            plaintext.fill(0)
            return NativeCredentialStatus.AVAILABLE
        }
    }

    fun readBound(reference: String): String? {
        PlatformPolicy.validateReference(reference)
        synchronized(PROCESS_LOCK) {
            val plaintext = readBoundPlaintext(reference) ?: return null
            return try {
                try {
                    decodeUtf8(plaintext).also {
                        require(it.isNotBlank()) { "credential unavailable" }
                    }
                } catch (error: Exception) {
                    throw CredentialRecoveryRequiredException(error)
                }
            } finally {
                plaintext.fill(0)
            }
        }
    }

    fun store(reference: String, value: String) {
        PlatformPolicy.validateReference(reference)
        val plaintext = PlatformPolicy.validateCredentialForWrite(value)
        try {
            storePrevalidatedBytes(reference, plaintext)
        } finally {
            plaintext.fill(0)
        }
    }

    /**
     * Stores a Rust-prepared reference and envelope without repeating
     * deterministic policy checks after the durable Started cutpoint.
     */
    fun storePrevalidated(reference: String, value: String) {
        val plaintext = value.toByteArray(Charsets.UTF_8)
        try {
            storePrevalidatedBytes(reference, plaintext)
        } finally {
            plaintext.fill(0)
        }
    }

    /**
     * Installs one Rust-prepared authority envelope without replacing a file
     * that appeared after the preceding Missing observation.
     */
    fun storePrevalidatedBound(reference: String, value: String) {
        val plaintext = value.toByteArray(Charsets.UTF_8)
        try {
            storePrevalidatedBoundBytes(reference, plaintext)
        } finally {
            plaintext.fill(0)
        }
    }

    private fun storePrevalidatedBytes(reference: String, plaintext: ByteArray) {
        synchronized(PROCESS_LOCK) {
            check(directory.mkdirs() || directory.isDirectory) {
                "credential unavailable"
            }
            val encoded = encode(reference, plaintext)
            var previousRecord: ByteArray? = null
            try {
                val file = credentialFile(reference)
                previousRecord = try {
                    readEncoded(file)
                } catch (_: FileNotFoundException) {
                    null
                }
                val stream = file.startWrite()
                try {
                    stream.write(encoded)
                    stream.fd.sync()
                    file.finishWrite(stream)
                } catch (error: Exception) {
                    file.failWrite(stream)
                    throw error
                }

                try {
                    val verified = readPlaintext(reference, file)
                    try {
                        check(MessageDigest.isEqual(plaintext, verified)) {
                            "credential unavailable"
                        }
                    } finally {
                        verified.fill(0)
                    }
                } catch (error: Exception) {
                    try {
                        restoreRecord(file, previousRecord)
                    } catch (restoreError: Exception) {
                        val recoveryError =
                            CredentialRecoveryRequiredException(error)
                        recoveryError.addSuppressed(restoreError)
                        throw recoveryError
                    }
                    throw error
                }
            } finally {
                encoded.fill(0)
                previousRecord?.fill(0)
            }
        }
    }

    private fun storePrevalidatedBoundBytes(reference: String, plaintext: ByteArray) {
        synchronized(PROCESS_LOCK) {
            BoundCredentialDirectory.ensureDurable(directory)
            val target = credentialBaseFile(reference)
            val encoded = encode(reference, plaintext)
            try {
                BoundCredentialFilePublisher.publishAddOnly(
                    target = target,
                    encoded = encoded,
                    verify = {
                        val verified = readBoundPlaintext(reference)
                        if (verified == null) {
                            false
                        } else {
                            try {
                                MessageDigest.isEqual(plaintext, verified)
                            } finally {
                                verified.fill(0)
                            }
                        }
                    },
                )
            } finally {
                encoded.fill(0)
            }
        }
    }

    fun delete(reference: String) {
        PlatformPolicy.validateReference(reference)
        synchronized(PROCESS_LOCK) {
            credentialFile(reference).delete()
        }
    }

    fun deleteBound(reference: String) {
        PlatformPolicy.validateReference(reference)
        synchronized(PROCESS_LOCK) {
            BoundCredentialFileDeleter.deleteBaseOnly(credentialBaseFile(reference))
        }
    }

    private fun readPlaintext(reference: String, file: AtomicFile): ByteArray {
        val encoded = readEncoded(file)
        return try {
            val plaintext = decode(reference, encoded)
            try {
                PlatformPolicy.validateCredentialForRead(plaintext)
                plaintext
            } catch (error: Exception) {
                plaintext.fill(0)
                throw error
            }
        } finally {
            encoded.fill(0)
        }
    }

    private fun readBoundPlaintext(reference: String): ByteArray? {
        val encoded = BoundCredentialFileReader.readBaseOnly(
            credentialBaseFile(reference),
            MAXIMUM_RECORD_BYTES,
        ) ?: return null
        return try {
            val plaintext = try {
                decode(reference, encoded)
            } catch (error: Exception) {
                throw CredentialRecoveryRequiredException(error)
            }
            try {
                PlatformPolicy.validateCredentialForRead(plaintext)
                plaintext
            } catch (error: Exception) {
                plaintext.fill(0)
                throw CredentialRecoveryRequiredException(error)
            }
        } finally {
            encoded.fill(0)
        }
    }

    private fun readEncoded(file: AtomicFile): ByteArray =
        AtomicCredentialFileReader.readEncoded(file, MAXIMUM_RECORD_BYTES)

    private fun restoreRecord(file: AtomicFile, record: ByteArray?) {
        if (record == null) {
            file.delete()
            check(!file.baseFile.exists()) {
                "credential unavailable"
            }
            return
        }

        val stream = file.startWrite()
        try {
            stream.write(record)
            stream.fd.sync()
            file.finishWrite(stream)
        } catch (error: Exception) {
            file.failWrite(stream)
            throw error
        }

        val restored = readEncoded(file)
        try {
            check(MessageDigest.isEqual(record, restored)) {
                "credential unavailable"
            }
        } finally {
            restored.fill(0)
        }
    }

    private fun encode(reference: String, plaintext: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, getOrCreateKey())
        updateAssociatedData(cipher, reference)
        val encrypted = cipher.doFinal(plaintext)
        return try {
            ByteArrayOutputStream().use { bytes ->
                DataOutputStream(bytes).use { output ->
                    output.writeInt(FILE_VERSION)
                    output.writeInt(cipher.iv.size)
                    output.write(cipher.iv)
                    output.writeInt(encrypted.size)
                    output.write(encrypted)
                }
                bytes.toByteArray()
            }
        } finally {
            encrypted.fill(0)
        }
    }

    private fun decode(reference: String, encoded: ByteArray): ByteArray =
        DataInputStream(ByteArrayInputStream(encoded)).use { input ->
            check(input.readInt() == FILE_VERSION) { "credential unavailable" }
            val ivLength = input.readInt()
            check(ivLength in MINIMUM_IV_BYTES..MAXIMUM_IV_BYTES) {
                "credential unavailable"
            }
            val iv = ByteArray(ivLength)
            val ciphertext: ByteArray
            try {
                input.readFully(iv)
                val ciphertextLength = input.readInt()
                check(ciphertextLength in 1..MAXIMUM_LEGACY_CIPHERTEXT_BYTES) {
                    "credential unavailable"
                }
                ciphertext = ByteArray(ciphertextLength)
                input.readFully(ciphertext)
                check(input.read() == -1) { "credential unavailable" }
            } catch (error: Exception) {
                iv.fill(0)
                throw error
            }

            try {
                val key = getExistingKey() ?: error("credential unavailable")
                val cipher = Cipher.getInstance(TRANSFORMATION)
                cipher.init(
                    Cipher.DECRYPT_MODE,
                    key,
                    GCMParameterSpec(GCM_TAG_BITS, iv),
                )
                updateAssociatedData(cipher, reference)
                cipher.doFinal(ciphertext)
            } finally {
                iv.fill(0)
                ciphertext.fill(0)
            }
        }

    private fun updateAssociatedData(cipher: Cipher, reference: String) {
        val associatedData = reference.toByteArray(Charsets.UTF_8)
        try {
            cipher.updateAAD(associatedData)
        } finally {
            associatedData.fill(0)
        }
    }

    private fun getExistingKey(): SecretKey? {
        val keyStore = KeyStore.getInstance(KEYSTORE_PROVIDER).apply { load(null) }
        return keyStore.getKey(KEY_ALIAS, null) as? SecretKey
    }

    private fun getOrCreateKey(): SecretKey {
        getExistingKey()?.let { return it }
        val generator = KeyGenerator.getInstance(
            KeyProperties.KEY_ALGORITHM_AES,
            KEYSTORE_PROVIDER,
        )
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true)
                .build(),
        )
        return generator.generateKey()
    }

    private fun credentialBaseFile(reference: String): File =
        directory.resolve(PlatformPolicy.credentialFileName(reference))

    private fun credentialFile(reference: String): AtomicFile = AtomicFile(credentialBaseFile(reference))

    private fun decodeUtf8(value: ByteArray): String =
        Charsets.UTF_8
            .newDecoder()
            .onMalformedInput(CodingErrorAction.REPORT)
            .onUnmappableCharacter(CodingErrorAction.REPORT)
            .decode(ByteBuffer.wrap(value))
            .toString()

    private companion object {
        const val CREDENTIAL_DIRECTORY = "provider-credentials"
        const val KEYSTORE_PROVIDER = "AndroidKeyStore"
        const val KEY_ALIAS = "dev.lorepia.provider-credentials.v1"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val GCM_TAG_BITS = 128
        const val FILE_VERSION = 1
        const val MINIMUM_IV_BYTES = 12
        const val MAXIMUM_IV_BYTES = 32
        const val MAXIMUM_LEGACY_CIPHERTEXT_BYTES = 64 * 1024
        const val MAXIMUM_RECORD_BYTES = MAXIMUM_LEGACY_CIPHERTEXT_BYTES + 128L
        val PROCESS_LOCK = Any()
    }
}
