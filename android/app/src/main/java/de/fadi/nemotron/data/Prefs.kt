package de.fadi.nemotron.data

import android.content.Context
import androidx.core.content.edit

/** Small settings that survive restarts. Private to the app and excluded from backups. */
class Prefs(context: Context) {
    private val prefs = context.getSharedPreferences("fadis", Context.MODE_PRIVATE)

    init {
        // Drops what older versions kept, such as the NVIDIA key of the former direct mode.
        val obsolete = prefs.all.keys - setOf(MAC_URL, MAC_TOKEN, MAC_NAME, HISTORY)
        if (obsolete.isNotEmpty()) prefs.edit { obsolete.forEach(::remove) }
    }

    var pairing: Pairing?
        get() {
            val url = prefs.getString(MAC_URL, null) ?: return null
            val token = prefs.getString(MAC_TOKEN, null) ?: return null
            return Pairing(url, token, prefs.getString(MAC_NAME, null) ?: "Mac")
        }
        set(value) = prefs.edit {
            if (value == null) {
                remove(MAC_URL)
                remove(MAC_TOKEN)
                remove(MAC_NAME)
            } else {
                putString(MAC_URL, value.url)
                putString(MAC_TOKEN, value.token)
                putString(MAC_NAME, value.name)
            }
        }

    /** What was sent from this phone, oldest first, as a JSON array. */
    var history: String?
        get() = prefs.getString(HISTORY, null)
        set(value) = prefs.edit { if (value == null) remove(HISTORY) else putString(HISTORY, value) }

    private companion object {
        const val HISTORY = "history"
        const val MAC_URL = "mac_url"
        const val MAC_TOKEN = "mac_token"
        const val MAC_NAME = "mac_name"
    }
}
