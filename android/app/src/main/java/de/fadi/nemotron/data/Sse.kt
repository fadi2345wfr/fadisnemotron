package de.fadi.nemotron.data

import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/** A model answer: the thinking and the visible answer, split apart. */
data class Reply(val thinking: String = "", val answer: String = "", val finishReason: String? = null) {
    companion object {
        fun of(reasoning: String, content: String, finishReason: String?): Reply {
            val (thinking, answer) = thinkingAndAnswer(reasoning, content)
            return Reply(thinking, answer, finishReason)
        }
    }
}

class DirectException(message: String) : Exception(message)

/**
 * Collects an OpenAI-compatible streamed chat completion (server-sent
 * events), one line at a time.
 */
class ChatStream {
    private val reasoning = StringBuilder()
    private val content = StringBuilder()
    private var finishReason: String? = null

    /** True once `data: [DONE]` arrived. */
    var finished = false
        private set

    val reply: Reply get() = Reply.of(reasoning.toString(), content.toString(), finishReason)

    /** Returns true if the reply changed. Throws [DirectException] for an error event. */
    fun feed(line: String): Boolean {
        // Blank separators, comments (keep-alives) and `event:` lines carry nothing for us.
        if (!line.startsWith("data:")) return false
        val data = line.removePrefix("data:").trim()
        if (data == "[DONE]") {
            finished = true
            return false
        }
        val chunk = parseObject(data) ?: return false
        errorMessage(chunk)?.let { throw DirectException(it) }
        val choice = (chunk["choices"] as? JsonArray)?.firstOrNull() as? JsonObject ?: return false
        var changed = false
        (choice["delta"] as? JsonObject)?.let { delta ->
            (delta.text("reasoning_content") ?: delta.text("reasoning"))?.let {
                reasoning.append(it)
                changed = true
            }
            delta.text("content")?.let {
                content.append(it)
                changed = true
            }
        }
        choice.text("finish_reason")?.let {
            finishReason = it
            changed = true
        }
        return changed
    }
}

/** Reads a complete, non-streamed chat completion. */
fun parseCompletion(body: String): Reply {
    val json = parseObject(body) ?: throw DirectException("Unerwartete Antwort vom Server: ${body.take(300)}")
    errorMessage(json)?.let { throw DirectException(it) }
    val choice = (json["choices"] as? JsonArray)?.firstOrNull() as? JsonObject
        ?: throw DirectException("Die Antwort des Servers enthält keinen Text.")
    val message = choice["message"] as? JsonObject
    return Reply.of(
        reasoning = message?.text("reasoning_content") ?: message?.text("reasoning") ?: "",
        content = message?.text("content") ?: "",
        finishReason = choice.text("finish_reason"),
    )
}

/** Pulls a readable message out of the error shapes NVIDIA and OpenAI-compatible servers use. */
internal fun errorMessage(json: JsonObject): String? {
    when (val error = json["error"]) {
        null, JsonNull -> {}
        is JsonPrimitive -> return error.content
        is JsonObject -> return error.text("message") ?: error.toString()
        else -> return error.toString()
    }
    if (json.text("object") == "error") return json.text("message")
    // NVIDIA cloud functions: {"status": 401, "title": "Unauthorized", "detail": "…"}
    if ("choices" !in json) return json.text("detail") ?: json.text("title")
    return null
}

private val lenientJson = Json { isLenient = true }

internal fun parseObject(text: String): JsonObject? = try {
    lenientJson.parseToJsonElement(text) as? JsonObject
} catch (e: SerializationException) {
    null
}

private fun JsonObject.text(key: String): String? =
    (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.content
