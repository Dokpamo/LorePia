package dev.lorepia.tauri.platform

import android.content.ContentResolver
import android.net.Uri
import android.provider.OpenableColumns
import java.io.BufferedOutputStream
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.util.UUID

internal class NativeStagedImport(
    val path: String,
    val displayName: String,
    val sizeBytes: Long,
) {
    override fun toString(): String =
        "NativeStagedImport(path=[REDACTED], displayName=[REDACTED], sizeBytes=$sizeBytes)"
}

internal class SelectedImportTooLarge : IOException("selected import is too large")

internal class AndroidImportStager(
    private val contentResolver: ContentResolver,
    private val stagingDirectory: File,
) {
    init {
        if (stagingDirectory.isDirectory) {
            val nowMillis = System.currentTimeMillis()
            stagingDirectory.listFiles()?.forEach { candidate ->
                if (
                    candidate.parentFile == stagingDirectory &&
                    PlatformPolicy.shouldRemoveAbandonedStagingFile(
                        name = candidate.name,
                        isRegularFile = candidate.isFile,
                        lastModifiedMillis = candidate.lastModified(),
                        nowMillis = nowMillis,
                    )
                ) {
                    candidate.delete()
                }
            }
        }
    }

    fun stage(uri: Uri): NativeStagedImport {
        check(stagingDirectory.mkdirs() || stagingDirectory.isDirectory) {
            "staging unavailable"
        }
        val metadata = queryMetadata(uri)
        if (metadata.sizeBytes != null &&
            metadata.sizeBytes > PlatformPolicy.MAXIMUM_IMPORT_BYTES
        ) {
            throw SelectedImportTooLarge()
        }

        val basename = "${PlatformPolicy.OWNED_STAGING_PREFIX}${UUID.randomUUID()}"
        val finalFile = stagingDirectory.resolve(
            "$basename${PlatformPolicy.stagingSuffix(metadata.displayName)}",
        )
        val partialFile = stagingDirectory.resolve("${finalFile.name}.partial")
        check(partialFile.createNewFile()) { "staging unavailable" }

        try {
            val input = contentResolver.openInputStream(uri)
                ?: throw IOException("selection unavailable")
            var copied = 0L
            input.use { source ->
                FileOutputStream(partialFile).use { destination ->
                    val output = BufferedOutputStream(destination)
                    val buffer = ByteArray(PlatformPolicy.COPY_BUFFER_BYTES)
                    try {
                        while (true) {
                            val count = source.read(buffer)
                            if (count < 0) {
                                break
                            }
                            copied = Math.addExact(copied, count.toLong())
                            if (copied > PlatformPolicy.MAXIMUM_IMPORT_BYTES) {
                                throw SelectedImportTooLarge()
                            }
                            output.write(buffer, 0, count)
                        }
                        output.flush()
                        destination.fd.sync()
                    } finally {
                        buffer.fill(0)
                    }
                }
            }
            check(partialFile.renameTo(finalFile)) { "staging unavailable" }
            return NativeStagedImport(
                path = finalFile.absolutePath,
                displayName = metadata.displayName,
                sizeBytes = copied,
            )
        } catch (error: Exception) {
            partialFile.delete()
            finalFile.delete()
            throw error
        }
    }

    fun discard(path: String) {
        val root = stagingDirectory.canonicalFile
        val candidate = File(path).canonicalFile
        require(candidate.parentFile == root) { "invalid staged path" }
        if (candidate.exists() && !candidate.delete()) {
            throw IOException("staging unavailable")
        }
    }

    private fun queryMetadata(uri: Uri): ImportMetadata {
        var displayName: String? = null
        var sizeBytes: Long? = null
        contentResolver.query(
            uri,
            arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE),
            null,
            null,
            null,
        )?.use { cursor ->
            if (cursor.moveToFirst()) {
                val nameIndex = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                val sizeIndex = cursor.getColumnIndex(OpenableColumns.SIZE)
                if (nameIndex >= 0 && !cursor.isNull(nameIndex)) {
                    displayName = cursor.getString(nameIndex)
                }
                if (sizeIndex >= 0 && !cursor.isNull(sizeIndex)) {
                    cursor.getLong(sizeIndex).takeIf { it >= 0 }?.let {
                        sizeBytes = it
                    }
                }
            }
        }
        return ImportMetadata(
            displayName = PlatformPolicy.sanitizeDisplayName(displayName),
            sizeBytes = sizeBytes,
        )
    }

    private data class ImportMetadata(
        val displayName: String,
        val sizeBytes: Long?,
    )
}
