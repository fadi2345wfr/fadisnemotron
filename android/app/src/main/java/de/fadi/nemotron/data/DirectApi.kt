package de.fadi.nemotron.data

import java.io.IOException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.add
import kotlinx.serialization.json.addJsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response

/** One turn of the conversation as sent to the model. [images] are base64 JPEGs. */
data class ChatMessage(val role: Role, val text: String, val images: List<String> = emptyList()) {
    enum class Role(val apiName: String) { User("user"), Assistant("assistant") }
}

/** Talks to an OpenAI-compatible chat completions endpoint, NVIDIA's by default. */
class DirectApi(http: OkHttpClient) {
    private val client = http.newBuilder()
        .connectTimeout(15, TimeUnit.SECONDS)
        // Thinking can take a while before the first token; the stream is silent until then.
        .readTimeout(3, TimeUnit.MINUTES)
        .build()

    /**
     * Sends the conversation and streams the answer. [onUpdate] receives the
     * reply so far, at most every [UPDATE_INTERVAL_MS] milliseconds, on a
     * background thread. Cancelling the coroutine stops the request.
     */
    suspend fun complete(
        settings: DirectSettings,
        messages: List<ChatMessage>,
        onUpdate: (Reply) -> Unit = {},
    ): Reply {
        val key = settings.apiKey.trim()
        if (!key.isHeaderSafe()) throw DirectException("Der API-Schlüssel enthält ungültige Zeichen. Bitte neu einfügen.")
        val url = settings.baseUrl.trim().trimEnd('/').toHttpUrlOrNull()
            ?.newBuilder()?.addPathSegments("chat/completions")?.build()
            ?: throw DirectException("Die Basis-URL ist ungültig: ${settings.baseUrl}")
        val request = Request.Builder()
            .url(url)
            .header("Authorization", "Bearer $key")
            .header("Accept", "text/event-stream")
            .post(requestBody(settings, messages).toString().toRequestBody(JSON))
            .build()
        return try {
            client.newCall(request).await { read(it, onUpdate) }
        } catch (e: IOException) {
            currentCoroutineContext().ensureActive()
            throw DirectException(networkMessage(e))
        }
    }

    private fun read(response: Response, onUpdate: (Reply) -> Unit): Reply {
        val body = response.body
        if (!response.isSuccessful) throw DirectException(describeHttpError(response.code, body.string()))
        if (body.contentType()?.subtype != "event-stream") {
            // Some servers ignore stream=true and answer with one JSON document.
            val text = body.string()
            if (!text.trimStart().startsWith("data:")) return parseCompletion(text)
            return ChatStream().apply { text.lineSequence().forEach { feed(it) } }.reply
        }
        val stream = ChatStream()
        val source = body.source()
        var lastUpdate = 0L
        while (!stream.finished) {
            val line = source.readUtf8Line() ?: break
            if (stream.feed(line)) {
                val now = System.nanoTime()
                if (now - lastUpdate >= UPDATE_INTERVAL_MS * 1_000_000) {
                    onUpdate(stream.reply)
                    lastUpdate = now
                }
            }
        }
        return stream.reply
    }

    companion object {
        const val UPDATE_INTERVAL_MS = 50L
        private val JSON = "application/json; charset=utf-8".toMediaType()
    }
}

internal fun requestBody(settings: DirectSettings, messages: List<ChatMessage>): JsonObject = buildJsonObject {
    put("model", settings.model.trim())
    putJsonArray("messages") {
        if (settings.systemPrompt.isNotBlank()) {
            addJsonObject {
                put("role", "system")
                put("content", settings.systemPrompt.trim())
            }
        }
        for (message in messages) {
            addJsonObject {
                put("role", message.role.apiName)
                if (message.images.isEmpty()) {
                    put("content", message.text)
                } else {
                    putJsonArray("content") {
                        if (message.text.isNotBlank()) {
                            addJsonObject {
                                put("type", "text")
                                put("text", message.text)
                            }
                        }
                        for (image in message.images) {
                            addJsonObject {
                                put("type", "image_url")
                                putJsonObject("image_url") { put("url", "data:image/jpeg;base64,$image") }
                            }
                        }
                    }
                }
            }
        }
    }
    put("stream", true)
    put("temperature", settings.temperature)
    put("top_p", settings.topP)
    put("max_tokens", settings.maxTokens)
    putJsonObject("chat_template_kwargs") { put("enable_thinking", settings.thinking) }
}

/** Turns an HTTP error status and body into a message for the user. */
internal fun describeHttpError(status: Int, body: String): String {
    val detail = parseObject(body)?.let(::errorMessage) ?: body.trim().take(300)
    val hint = when (status) {
        401, 403 -> "API-Schlüssel fehlt, ist ungültig oder hat keinen Zugriff auf dieses Modell."
        404 -> "Modell oder Adresse nicht gefunden. Prüfe Modell-ID und Basis-URL in den Einstellungen."
        413 -> "Die Anfrage ist zu groß. Weniger oder kleinere Fotos anhängen."
        400, 422 -> "Der Server hat die Anfrage abgelehnt."
        429 -> "Zu viele Anfragen. Kurz warten und erneut versuchen."
        in 500..599 -> "Fehler auf dem Server. Später erneut versuchen."
        else -> "Anfrage fehlgeschlagen."
    }
    return if (detail.isEmpty()) "HTTP $status: $hint" else "HTTP $status: $hint\n$detail"
}

private fun networkMessage(e: IOException): String = when (e) {
    is UnknownHostException -> "Server nicht gefunden. Internetverbindung und Basis-URL prüfen."
    is SocketTimeoutException -> "Zeitüberschreitung: Der Server antwortet nicht."
    else -> "Verbindung fehlgeschlagen: ${e.message ?: e.javaClass.simpleName}"
}
