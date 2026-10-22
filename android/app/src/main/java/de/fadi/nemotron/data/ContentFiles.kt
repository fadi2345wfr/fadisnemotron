package de.fadi.nemotron.data

import android.content.ContentResolver
import android.content.res.AssetFileDescriptor
import android.net.Uri
import android.provider.OpenableColumns

/** The file name the picker shows, if the provider has one. */
internal fun ContentResolver.displayName(uri: Uri): String? =
    query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
        if (cursor.moveToFirst()) cursor.getString(0)?.takeIf { it.isNotBlank() } else null
    }

/** The size in bytes, or -1 if the provider does not know it. */
internal fun ContentResolver.fileSize(uri: Uri): Long {
    val listed = query(uri, arrayOf(OpenableColumns.SIZE), null, null, null)?.use { cursor ->
        if (cursor.moveToFirst() && !cursor.isNull(0)) cursor.getLong(0) else null
    }
    return listed ?: openAssetFileDescriptor(uri, "r")?.use { it.length } ?: AssetFileDescriptor.UNKNOWN_LENGTH
}
