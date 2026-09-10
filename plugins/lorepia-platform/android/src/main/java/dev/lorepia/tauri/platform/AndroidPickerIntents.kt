package dev.lorepia.tauri.platform

import android.content.Intent

internal fun createImportPickerIntent(): Intent =
    Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
        addCategory(Intent.CATEGORY_OPENABLE)
        type = "*/*"
        // Providers do not agree on a MIME type for custom `.charx` files.
        // A MIME allowlist can therefore make valid cards unselectable. The
        // staged bytes are bounded and inspected by Core before any import.
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }
