package de.fadi.nemotron.data

/** Connection to NVIDIA (or any OpenAI-compatible server) for the direct mode. */
data class DirectSettings(
    val apiKey: String = "",
    val baseUrl: String = DEFAULT_BASE_URL,
    val model: String = DEFAULT_MODEL,
    val thinking: Boolean = true,
    val temperature: Double = 1.0,
    val topP: Double = 0.95,
    val maxTokens: Int = 16384,
    val systemPrompt: String = DEFAULT_SYSTEM_PROMPT,
) {
    companion object {
        const val DEFAULT_BASE_URL = "https://integrate.api.nvidia.com/v1"
        const val DEFAULT_MODEL = "nvidia/nemotron-3.5-super-vl-120b-a12b"
        const val DEFAULT_SYSTEM_PROMPT =
            "Du bist ein hilfreicher Assistent. Antworte in der Sprache des Nutzers."
    }
}
