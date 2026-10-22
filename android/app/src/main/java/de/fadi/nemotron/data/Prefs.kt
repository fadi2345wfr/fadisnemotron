package de.fadi.nemotron.data

import android.content.Context
import androidx.core.content.edit

/** Small settings that survive restarts. Private to the app and excluded from backups. */
class Prefs(context: Context) {
    private val prefs = context.getSharedPreferences("fadis", Context.MODE_PRIVATE)

    var lastTab: String?
        get() = prefs.getString(TAB, null)
        set(value) = prefs.edit { putString(TAB, value) }

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

    var direct: DirectSettings
        get() {
            val defaults = DirectSettings()
            return DirectSettings(
                apiKey = prefs.getString(API_KEY, null) ?: defaults.apiKey,
                baseUrl = prefs.getString(BASE_URL, null) ?: defaults.baseUrl,
                model = prefs.getString(MODEL, null) ?: defaults.model,
                thinking = prefs.getBoolean(THINKING, defaults.thinking),
                // Stored as text: a Float would turn 0.95 into 0.949999988… in the request.
                temperature = prefs.getString(TEMPERATURE, null)?.toDoubleOrNull() ?: defaults.temperature,
                topP = prefs.getString(TOP_P, null)?.toDoubleOrNull() ?: defaults.topP,
                maxTokens = prefs.getInt(MAX_TOKENS, defaults.maxTokens),
                systemPrompt = prefs.getString(SYSTEM_PROMPT, null) ?: defaults.systemPrompt,
            )
        }
        set(value) = prefs.edit {
            putString(API_KEY, value.apiKey)
            putString(BASE_URL, value.baseUrl)
            putString(MODEL, value.model)
            putBoolean(THINKING, value.thinking)
            putString(TEMPERATURE, value.temperature.toString())
            putString(TOP_P, value.topP.toString())
            putInt(MAX_TOKENS, value.maxTokens)
            putString(SYSTEM_PROMPT, value.systemPrompt)
        }

    private companion object {
        const val TAB = "tab"
        const val MAC_URL = "mac_url"
        const val MAC_TOKEN = "mac_token"
        const val MAC_NAME = "mac_name"
        const val API_KEY = "direct_api_key"
        const val BASE_URL = "direct_base_url"
        const val MODEL = "direct_model"
        const val THINKING = "direct_thinking"
        const val TEMPERATURE = "direct_temperature"
        const val TOP_P = "direct_top_p"
        const val MAX_TOKENS = "direct_max_tokens"
        const val SYSTEM_PROMPT = "direct_system_prompt"
    }
}
