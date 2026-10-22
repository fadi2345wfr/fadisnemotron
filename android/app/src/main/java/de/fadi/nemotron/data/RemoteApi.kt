package de.fadi.nemotron.data

import java.io.IOException
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

/** A failed request to the Mac; [message] is ready to show. [status] is null if no HTTP answer came back. */
class RemoteException(val status: Int?, message: String) : Exception(message)

/** Client for the Mac app's remote-access protocol (docs/remote-api.md). */
class RemoteApi(baseUrl: String, private val token: String, http: OkHttpClient) {
    private val base: HttpUrl = baseUrl.toHttpUrl()
    private val client = http.newBuilder()
        .connectTimeout(5, TimeUnit.SECONDS)
        // A long-poll may legitimately stay silent for up to 30 s.
        .readTimeout(40, TimeUnit.SECONDS)
        .build()
    private val pingClient = client.newBuilder().callTimeout(4, TimeUnit.SECONDS).build()

    suspend fun ping(): Ping = decode(execute(pingClient, request("ping")))

    /** The current state; waits up to [wait] seconds for a newer one than [after]. */
    suspend fun state(after: Long, wait: Int = LONG_POLL_SECONDS): RemoteState {
        val request = request("state") {
            addQueryParameter("after", after.toString())
            addQueryParameter("wait", wait.toString())
        }
        return decode(execute(client, request))
    }

    suspend fun diff(path: String): String =
        decode<DiffResponse>(execute(client, request("diff") { addQueryParameter("path", path) })).diff

    suspend fun send(text: String, images: List<ImageUpload>) {
        try {
            post("send", remoteJson.encodeToString(SendRequest(text, images)))
        } catch (e: RemoteException) {
            throw if (e.status == 409) RemoteException(409, BUSY) else e
        }
    }

    suspend fun stop() = post("stop")

    suspend fun decide(callId: String, decision: Decision) =
        post("decide", remoteJson.encodeToString(DecideRequest(callId, decision)))

    suspend fun newChat() = post("new_chat")

    suspend fun retry() = post("retry")

    suspend fun settings(approval: ApprovalMode? = null, thinking: Boolean? = null) =
        post("settings", remoteJson.encodeToString(SettingsRequest(approval, thinking)))

    suspend fun undo(path: String) = post("undo", remoteJson.encodeToString(UndoRequest(path)))

    private fun request(endpoint: String, query: HttpUrl.Builder.() -> Unit = {}): Request.Builder =
        Request.Builder()
            .url(base.newBuilder().addPathSegments("api/v1/$endpoint").apply(query).build())
            .header("Authorization", "Bearer $token")

    private suspend fun post(endpoint: String, json: String = "{}") {
        execute(client, request(endpoint).post(json.toRequestBody(JSON)))
    }

    private suspend fun execute(client: OkHttpClient, request: Request.Builder): String = try {
        client.newCall(request.build()).await { response ->
            val body = response.body.string()
            if (!response.isSuccessful) throw remoteError(response.code, body)
            body
        }
    } catch (e: IOException) {
        // A cancelled poll also ends up here; that is not a connection problem.
        currentCoroutineContext().ensureActive()
        throw RemoteException(null, UNREACHABLE)
    }

    private inline fun <reified T> decode(body: String): T = try {
        remoteJson.decodeFromString<T>(body)
    } catch (e: IllegalArgumentException) {
        // SerializationException is one of these too.
        throw RemoteException(null, UNREADABLE)
    }

    companion object {
        const val LONG_POLL_SECONDS = 25
        const val BUSY = "Nemotron arbeitet noch."
        const val UNREACHABLE =
            "Mac nicht erreichbar. Gleiches WLAN oder Tailscale prüfen – ist der Fernzugriff am Mac an?"
        private const val UNREADABLE = "Die Antwort des Macs ist unverständlich. Sind beide Apps aktuell?"
        private val JSON = "application/json; charset=utf-8".toMediaType()
    }
}

/** Turns an HTTP error into a message; the Mac's own `{"error": …}` text wins where it makes sense. */
private fun remoteError(status: Int, body: String): RemoteException {
    val message = when (status) {
        401 -> "Falscher Schlüssel. Koppel das Handy neu: QR-Code in der Mac-App scannen."
        else -> errorText(body) ?: when (status) {
            409 -> RemoteApi.BUSY
            413 -> "Zu groß für den Mac (höchstens 20 MB). Weniger oder kleinere Fotos anhängen."
            503 -> "Die Mac-App antwortet nicht. Ist sie geöffnet?"
            else -> "Der Mac meldet Fehler $status."
        }
    }
    return RemoteException(status, message)
}

private fun errorText(body: String): String? = try {
    val error = (remoteJson.parseToJsonElement(body) as? JsonObject)?.get("error")
    (error as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }
} catch (e: SerializationException) {
    null
}

@Serializable
data class Ping(val name: String = "")

@Serializable
private class DiffResponse(val diff: String = "")

@Serializable
private class SendRequest(val text: String, val images: List<ImageUpload>)

@Serializable
private class DecideRequest(@SerialName("call_id") val callId: String, val decision: Decision)

@Serializable
private class SettingsRequest(val approval: ApprovalMode?, val thinking: Boolean?)

@Serializable
private class UndoRequest(val path: String)
