package dev.lorepia.tauri.platform

import android.app.Activity
import android.app.AlertDialog
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import androidx.activity.result.ActivityResult
import androidx.appcompat.app.AppCompatActivity
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.FileOutputStream
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

@InvokeArg
internal class ReferenceArgs {
    lateinit var reference: String
}

@InvokeArg
internal class CredentialArgs {
    lateinit var reference: String
    lateinit var value: String
}

@InvokeArg
internal class StagedPathArgs {
    lateinit var path: String
}

@InvokeArg
internal class SensitiveCaptureArgs {
    var maximumBytes: Long = 0
}

@InvokeArg
internal class CredentialEffectConfirmationArgs {
    lateinit var effect: String
    lateinit var targetId: String
    lateinit var origin: String
    lateinit var revision: String
}

@InvokeArg
internal class SaveContentSourceArgs {
    lateinit var sourcePath: String
    lateinit var suggestedName: String
    var expectedSizeBytes: Long = 0
    lateinit var expectedSha256: String
}

private enum class ClipboardCleanupStatus(val wireValue: String) {
    CLEARED("cleared"),
    ALREADY_REPLACED("already_replaced"),
    CLEAR_FAILED("clear_failed"),
}

@TauriPlugin
class LorepiaPlatformPlugin(private val activity: Activity) : Plugin(activity) {
    private val workQueues = PlatformWorkQueues()
    private val pickerInFlight = AtomicBoolean(false)
    private val sensitiveCaptureInFlight = AtomicBoolean(false)
    private val credentialConfirmationInFlight = AtomicBoolean(false)
    private val credentials = AndroidCredentialStore(activity.applicationContext)
    private val clipboard = activity.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
    private val dataRoot = activity.filesDir.resolve(DATA_ROOT_DIRECTORY).absoluteFile
    private val sensitiveCaptureRoot = dataRoot
        .resolve(SENSITIVE_CAPTURE_DIRECTORY)
        .absoluteFile
    private val stager = AndroidImportStager(
        activity.contentResolver,
        activity.cacheDir.resolve(IMPORT_STAGING_DIRECTORY).absoluteFile,
    )

    init {
        cleanupAbandonedSensitiveCaptures()
    }

    @Command
    fun dataRoot(invoke: Invoke) {
        try {
            check(dataRoot.mkdirs() || dataRoot.isDirectory) { "storage unavailable" }
            invoke.resolve(JSObject().put("path", dataRoot.absolutePath))
        } catch (_: Exception) {
            invoke.reject("storage unavailable", "storage_unavailable")
        }
    }

    @Command
    fun confirmCredentialEffect(invoke: Invoke) {
        activity.runOnUiThread confirm@{
            if (
                !activity.hasWindowFocus() ||
                activity.isFinishing ||
                activity.isDestroyed ||
                !credentialConfirmationInFlight.compareAndSet(false, true)
            ) {
                invoke.reject("foreground confirmation required", "permission_denied")
                return@confirm
            }
            try {
                val args = invoke.parseArgs(CredentialEffectConfirmationArgs::class.java)
                validateCredentialConfirmation(args)
                AlertDialog.Builder(activity)
                    .setTitle(credentialConfirmationTitle(args.effect))
                    .setMessage(credentialConfirmationMessage(args))
                    .setNegativeButton("Cancel") { _, _ ->
                        finishCredentialConfirmation(invoke, false)
                    }
                    .setPositiveButton("Approve exact action") { dialog, _ ->
                        val dialogFocused =
                            (dialog as? AlertDialog)?.window?.decorView?.hasWindowFocus() == true
                        finishCredentialConfirmation(
                            invoke,
                            dialogFocused && !activity.isFinishing && !activity.isDestroyed,
                        )
                    }
                    .setOnCancelListener {
                        finishCredentialConfirmation(invoke, false)
                    }
                    .show()
            } catch (_: Exception) {
                credentialConfirmationInFlight.set(false)
                invoke.reject("foreground confirmation required", "permission_denied")
            }
        }
    }

    @Command
    fun credentialStatus(invoke: Invoke) {
        workQueues.executeCredential {
            try {
                val args = invoke.parseArgs(ReferenceArgs::class.java)
                val response = JSObject().put(
                    "status",
                    credentials.status(args.reference).wireValue,
                )
                invoke.resolve(response)
            } catch (_: Exception) {
                invoke.resolve(JSObject().put("status", NativeCredentialStatus.UNREADABLE.wireValue))
            }
        }
    }

    @Command
    fun boundCredentialStatus(invoke: Invoke) {
        workQueues.executeCredential {
            try {
                val args = invoke.parseArgs(ReferenceArgs::class.java)
                invoke.resolve(
                    JSObject().put("status", credentials.boundStatus(args.reference).wireValue),
                )
            } catch (_: CredentialRecoveryRequiredException) {
                invoke.reject(
                    "credential recovery requires user attention",
                    "credential_recovery_required",
                )
            } catch (_: Exception) {
                invoke.reject("credential unavailable", "credential_unavailable")
            }
        }
    }

    @Command
    fun readCredential(invoke: Invoke) {
        workQueues.executeCredential {
            try {
                val args = invoke.parseArgs(ReferenceArgs::class.java)
                invoke.resolve(JSObject().put("value", credentials.read(args.reference)))
            } catch (_: Exception) {
                invoke.reject("credential unavailable", "credential_unavailable")
            }
        }
    }

    @Command
    fun readBoundCredential(invoke: Invoke) {
        workQueues.executeCredential {
            try {
                val args = invoke.parseArgs(ReferenceArgs::class.java)
                invoke.resolve(JSObject().put("value", credentials.readBound(args.reference)))
            } catch (_: CredentialRecoveryRequiredException) {
                invoke.reject(
                    "credential recovery requires user attention",
                    "credential_recovery_required",
                )
            } catch (_: Exception) {
                invoke.reject("credential unavailable", "credential_unavailable")
            }
        }
    }

    @Command
    fun storeCredential(invoke: Invoke) {
        workQueues.executeCredential {
            try {
                val args = invoke.parseArgs(CredentialArgs::class.java)
                credentials.storePrevalidated(args.reference, args.value)
                invoke.resolve()
            } catch (_: CredentialRecoveryRequiredException) {
                invoke.reject(
                    "credential recovery requires user attention",
                    "credential_recovery_required",
                )
            } catch (_: Exception) {
                invoke.reject("credential unavailable", "credential_unavailable")
            }
        }
    }

    @Command
    fun storeBoundCredential(invoke: Invoke) {
        workQueues.executeCredential {
            try {
                val args = invoke.parseArgs(CredentialArgs::class.java)
                credentials.storePrevalidatedBound(args.reference, args.value)
                invoke.resolve()
            } catch (_: CredentialRecoveryRequiredException) {
                invoke.reject(
                    "credential recovery requires user attention",
                    "credential_recovery_required",
                )
            } catch (_: Exception) {
                invoke.reject("credential unavailable", "credential_unavailable")
            }
        }
    }

    @Command
    fun deleteBoundCredential(invoke: Invoke) {
        workQueues.executeCredential {
            try {
                val args = invoke.parseArgs(ReferenceArgs::class.java)
                credentials.deleteBound(args.reference)
                invoke.resolve()
            } catch (_: CredentialRecoveryRequiredException) {
                invoke.reject(
                    "credential recovery requires user attention",
                    "credential_recovery_required",
                )
            } catch (_: Exception) {
                invoke.reject("credential unavailable", "credential_unavailable")
            }
        }
    }

    @Command
    fun captureCredential(invoke: Invoke) {
        activity.runOnUiThread capture@{
            if (!beginSensitiveCapture(invoke)) {
                return@capture
            }
            val args: ReferenceArgs
            val captured: String
            try {
                args = invoke.parseArgs(ReferenceArgs::class.java)
                PlatformPolicy.validateReference(args.reference)
                captured = foregroundClipboardText()
                val validated = PlatformPolicy.validateSensitiveCapture(
                    captured,
                    PlatformPolicy.MAXIMUM_CREDENTIAL_WRITE_BYTES.toLong(),
                )
                validated.fill(0)
            } catch (_: SecurityException) {
                finishSensitiveCapture()
                invoke.reject("clipboard permission denied", "permission_denied")
                return@capture
            } catch (_: Exception) {
                finishSensitiveCapture()
                invoke.reject("clipboard is empty or invalid", "invalid_input")
                return@capture
            }

            workQueues.executeCredential {
                try {
                    credentials.store(args.reference, captured)
                    activity.runOnUiThread {
                        val cleanup = clearClipboardIfUnchanged(captured)
                        finishSensitiveCapture()
                        invoke.resolve(
                            JSObject().put("clipboardCleanup", cleanup.wireValue),
                        )
                    }
                } catch (_: CredentialRecoveryRequiredException) {
                    finishSensitiveCapture()
                    invoke.reject(
                        "credential recovery requires user attention",
                        "credential_recovery_required",
                    )
                } catch (_: Exception) {
                    finishSensitiveCapture()
                    invoke.reject("credential unavailable", "credential_unavailable")
                }
            }
        }
    }

    @Command
    fun captureSensitiveText(invoke: Invoke) {
        activity.runOnUiThread capture@{
            if (!beginSensitiveCapture(invoke)) {
                return@capture
            }
            val maximumBytes: Long
            val captured: String
            val encoded: ByteArray
            try {
                maximumBytes = invoke
                    .parseArgs(SensitiveCaptureArgs::class.java)
                    .maximumBytes
                captured = foregroundClipboardText()
                encoded = PlatformPolicy.validateSensitiveCapture(captured, maximumBytes)
            } catch (_: SecurityException) {
                finishSensitiveCapture()
                invoke.reject("clipboard permission denied", "permission_denied")
                return@capture
            } catch (_: Exception) {
                finishSensitiveCapture()
                invoke.reject("clipboard is empty or invalid", "invalid_input")
                return@capture
            }

            workQueues.executeStaging {
                try {
                    val staged = stageSensitiveCapture(encoded)
                    activity.runOnUiThread {
                        val cleanup = clearClipboardIfUnchanged(captured)
                        finishSensitiveCapture()
                        invoke.resolve(
                            JSObject()
                                .put("path", staged.absolutePath)
                                .put("sizeBytes", staged.length())
                                .put("clipboardCleanup", cleanup.wireValue),
                        )
                    }
                } catch (_: Exception) {
                    finishSensitiveCapture()
                    invoke.reject("sensitive capture unavailable", "storage_unavailable")
                } finally {
                    encoded.fill(0)
                }
            }
        }
    }

    @Command
    fun deleteCredential(invoke: Invoke) {
        workQueues.executeCredential {
            try {
                val args = invoke.parseArgs(ReferenceArgs::class.java)
                credentials.delete(args.reference)
                invoke.resolve()
            } catch (_: Exception) {
                invoke.reject("credential unavailable", "credential_unavailable")
            }
        }
    }

    @Command
    fun pickImport(invoke: Invoke) {
        if (!pickerInFlight.compareAndSet(false, true)) {
            invoke.reject("file picker is busy", "busy")
            return
        }
        try {
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = "*/*"
                putExtra(
                    Intent.EXTRA_MIME_TYPES,
                    arrayOf(
                        "application/json",
                        "application/zip",
                        "application/octet-stream",
                        "image/*",
                    ),
                )
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
            startActivityForResult(invoke, intent, "onImportPicked")
        } catch (_: Exception) {
            pickerInFlight.set(false)
            invoke.reject("file selection failed", "selection_failed")
        }
    }

    @Command
    fun saveContentSource(invoke: Invoke) {
        if (!pickerInFlight.compareAndSet(false, true)) {
            invoke.reject("file picker is busy", "busy")
            return
        }
        workQueues.executeStaging {
            val args: SaveContentSourceArgs
            try {
                args = invoke.parseArgs(SaveContentSourceArgs::class.java)
                PlatformPolicy.validateContentSourceExport(
                    dataRoot = dataRoot,
                    sourcePath = args.sourcePath,
                    suggestedName = args.suggestedName,
                    expectedSizeBytes = args.expectedSizeBytes,
                    expectedSha256 = args.expectedSha256,
                )
            } catch (_: Exception) {
                pickerInFlight.set(false)
                invoke.reject("invalid content source export", "invalid_input")
                return@executeStaging
            }

            activity.runOnUiThread {
                try {
                    check(!activity.isFinishing && !activity.isDestroyed) {
                        "activity unavailable"
                    }
                    val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                        addCategory(Intent.CATEGORY_OPENABLE)
                        type = exportMimeType(args.suggestedName)
                        putExtra(Intent.EXTRA_TITLE, args.suggestedName)
                        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                        addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
                    }
                    // ACTION_CREATE_DOCUMENT owns destination selection and any
                    // provider-specific replacement confirmation.
                    startActivityForResult(invoke, intent, "onContentSourceDestinationPicked")
                } catch (_: Exception) {
                    pickerInFlight.set(false)
                    invoke.reject("file selection failed", "selection_failed")
                }
            }
        }
    }

    @ActivityCallback
    private fun onImportPicked(invoke: Invoke, result: ActivityResult) {
        val uri = result.data?.data
        if (result.resultCode != Activity.RESULT_OK || uri == null) {
            pickerInFlight.set(false)
            invoke.resolve(JSObject().put("selected", false))
            return
        }

        workQueues.executeStaging {
            try {
                val staged = stager.stage(uri)
                invoke.resolve(
                    JSObject()
                        .put("selected", true)
                        .put("path", staged.path)
                        .put("displayName", staged.displayName)
                        .put("sizeBytes", staged.sizeBytes),
                )
            } catch (_: SelectedImportTooLarge) {
                invoke.reject("selected file is too large", "selected_file_too_large")
            } catch (_: Exception) {
                invoke.reject("file selection failed", "selection_failed")
            } finally {
                pickerInFlight.set(false)
            }
        }
    }

    @ActivityCallback
    private fun onContentSourceDestinationPicked(invoke: Invoke, result: ActivityResult) {
        val uri = result.data?.data
        if (result.resultCode != Activity.RESULT_OK || uri == null) {
            pickerInFlight.set(false)
            invoke.resolve(JSObject().put("selected", false))
            return
        }

        workQueues.executeStaging {
            try {
                val args = invoke.parseArgs(SaveContentSourceArgs::class.java)
                val source = PlatformPolicy.validateContentSourceExport(
                    dataRoot = dataRoot,
                    sourcePath = args.sourcePath,
                    suggestedName = args.suggestedName,
                    expectedSizeBytes = args.expectedSizeBytes,
                    expectedSha256 = args.expectedSha256,
                )
                val descriptor = activity.contentResolver.openFileDescriptor(uri, "rwt")
                    ?: error("destination unavailable")
                ParcelFileDescriptor.AutoCloseOutputStream(descriptor).use { output ->
                    // `rwt` requested truncation before this descriptor was
                    // returned. Do not assume the provider is seekable.
                    PlatformPolicy.copyVerifiedContentSource(
                        source,
                        output,
                        args.expectedSizeBytes,
                        args.expectedSha256,
                    )
                    output.fd.sync()
                }
                activity.contentResolver.openInputStream(uri).use { saved ->
                    PlatformPolicy.verifyExportedContent(
                        saved ?: error("saved content unavailable"),
                        args.expectedSizeBytes,
                        args.expectedSha256,
                    )
                }
                val displayName = PlatformPolicy.validateExportReceiptDisplayName(
                    savedDisplayName(uri) ?: error("saved display name unavailable"),
                )
                invoke.resolve(
                    JSObject()
                        .put("selected", true)
                        .put("displayName", displayName)
                        .put("sizeBytes", args.expectedSizeBytes)
                        .put("sha256", args.expectedSha256),
                )
            } catch (_: IllegalArgumentException) {
                invoke.reject("invalid content source export", "invalid_input")
            } catch (_: Exception) {
                // Provider implementations vary: the returned URI could alias
                // a pre-existing document even though ACTION_CREATE_DOCUMENT
                // normally creates one. Never delete an uncertain user target
                // after a partial write.
                invoke.reject("content source export failed", "storage_unavailable")
            } finally {
                pickerInFlight.set(false)
            }
        }
    }

    @Command
    fun discardStagedImport(invoke: Invoke) {
        workQueues.executeStaging {
            try {
                val args = invoke.parseArgs(StagedPathArgs::class.java)
                stager.discard(args.path)
                invoke.resolve()
            } catch (_: Exception) {
                invoke.reject("storage unavailable", "storage_unavailable")
            }
        }
    }

    override fun onDestroy(activity: AppCompatActivity) {
        workQueues.shutdownNow()
        pickerInFlight.set(false)
        sensitiveCaptureInFlight.set(false)
        credentialConfirmationInFlight.set(false)
    }

    private fun finishCredentialConfirmation(invoke: Invoke, approved: Boolean) {
        if (credentialConfirmationInFlight.compareAndSet(true, false)) {
            invoke.resolve(JSObject().put("approved", approved))
        }
    }

    private fun validateCredentialConfirmation(args: CredentialEffectConfirmationArgs) {
        require(
            args.effect in setOf(
                "capture_or_replace",
                "delete",
                "archive",
                "discovery_compensation",
            ),
        )
        PlatformPolicy.validateCredentialConfirmationText(args.targetId, 256)
        PlatformPolicy.validateCredentialConfirmationText(args.origin, 2_048)
        PlatformPolicy.validateCredentialConfirmationText(args.revision, 256)
    }

    private fun credentialConfirmationTitle(effect: String): String = when (effect) {
        "capture_or_replace" -> "Allow credential capture?"
        "delete" -> "Delete stored credential?"
        "archive" -> "Archive connection and delete credential?"
        "discovery_compensation" -> "Remove uncommitted credential?"
        else -> error("invalid credential effect")
    }

    private fun credentialConfirmationMessage(args: CredentialEffectConfirmationArgs): String {
        val effect = when (args.effect) {
            "capture_or_replace" ->
                "read one credential from the clipboard and store it. " +
                    "If an older credential exists, it will be deleted only after the replacement is stored"
            "delete" -> "permanently delete the stored credential"
            "archive" ->
                "archive this provider connection and permanently delete its stored credential"
            "discovery_compensation" ->
                "permanently delete the credential created by the cancelled or failed discovery"
            else -> error("invalid credential effect")
        }
        return "LorePia will $effect.\n\n" +
            "Target: ${args.targetId}\n" +
            "Origin: ${args.origin}\n" +
            "Revision: ${args.revision}\n\n" +
            "Approve only if these exact details match your intended action."
    }

    private fun beginSensitiveCapture(invoke: Invoke): Boolean {
        if (!activity.hasWindowFocus() || activity.isFinishing || activity.isDestroyed) {
            invoke.reject("foreground interaction required", "permission_denied")
            return false
        }
        if (!sensitiveCaptureInFlight.compareAndSet(false, true)) {
            invoke.reject("sensitive capture is busy", "busy")
            return false
        }
        return true
    }

    private fun finishSensitiveCapture() {
        sensitiveCaptureInFlight.set(false)
    }

    private fun foregroundClipboardText(): String {
        check(activity.hasWindowFocus() && !activity.isFinishing && !activity.isDestroyed) {
            "foreground interaction required"
        }
        val clip = clipboard.primaryClip ?: error("empty clipboard")
        check(clip.itemCount == 1) { "ambiguous clipboard" }
        return clip.getItemAt(0)
            .coerceToText(activity)
            ?.toString()
            ?.takeIf(String::isNotBlank)
            ?: error("empty clipboard")
    }

    private fun clearClipboardIfUnchanged(expected: String): ClipboardCleanupStatus {
        return try {
            val current = clipboard.primaryClip
                ?.takeIf { it.itemCount == 1 }
                ?.getItemAt(0)
                ?.coerceToText(activity)
                ?.toString()
            if (current != expected) {
                ClipboardCleanupStatus.ALREADY_REPLACED
            } else {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                    clipboard.clearPrimaryClip()
                } else {
                    clipboard.setPrimaryClip(ClipData.newPlainText("", ""))
                }
                val remaining = clipboard.primaryClip
                    ?.takeIf { it.itemCount > 0 }
                    ?.getItemAt(0)
                    ?.coerceToText(activity)
                    ?.toString()
                if (remaining.isNullOrEmpty()) {
                    ClipboardCleanupStatus.CLEARED
                } else {
                    ClipboardCleanupStatus.CLEAR_FAILED
                }
            }
        } catch (_: Exception) {
            ClipboardCleanupStatus.CLEAR_FAILED
        }
    }

    private fun stageSensitiveCapture(bytes: ByteArray): java.io.File {
        check(sensitiveCaptureRoot.mkdirs() || sensitiveCaptureRoot.isDirectory) {
            "storage unavailable"
        }
        val destination = sensitiveCaptureRoot.resolve(
            "$SENSITIVE_CAPTURE_PREFIX${UUID.randomUUID()}",
        )
        try {
            FileOutputStream(destination).use { output ->
                output.write(bytes)
                output.fd.sync()
            }
            check(destination.isFile && destination.length() == bytes.size.toLong()) {
                "storage unavailable"
            }
            return destination
        } catch (error: Exception) {
            destination.delete()
            throw error
        }
    }

    private fun cleanupAbandonedSensitiveCaptures() {
        sensitiveCaptureRoot.listFiles()?.forEach { file ->
            if (file.isFile && file.name.startsWith(SENSITIVE_CAPTURE_PREFIX)) {
                file.delete()
            }
        }
    }

    private fun exportMimeType(suggestedName: String): String =
        when (suggestedName.substringAfterLast('.', "").lowercase()) {
            "json" -> "application/json"
            "charx", "zip" -> "application/zip"
            else -> "application/octet-stream"
        }

    private fun savedDisplayName(uri: android.net.Uri): String? =
        activity.contentResolver.query(
            uri,
            arrayOf(OpenableColumns.DISPLAY_NAME),
            null,
            null,
            null,
        )?.use { cursor ->
            val index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
            if (index >= 0 && cursor.moveToFirst()) cursor.getString(index) else null
        }

    private companion object {
        const val DATA_ROOT_DIRECTORY = "lorepia-data"
        const val IMPORT_STAGING_DIRECTORY = "import-staging"
        const val SENSITIVE_CAPTURE_DIRECTORY = "sensitive-capture"
        const val SENSITIVE_CAPTURE_PREFIX = "lorepia-sensitive-"
    }
}
