package de.fadi.nemotron.data

import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.double
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.OkHttpClient
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

class DirectApiTest {
    private val server = MockWebServer()
    private val api = DirectApi(OkHttpClient())
    private lateinit var settings: DirectSettings

    @Before
    fun start() {
        server.start()
        settings = DirectSettings(apiKey = "nvapi-test", baseUrl = server.url("/v1/").toString(), thinking = false)
    }

    @After
    fun stop() = server.close()

    private fun events(vararg data: String) = MockResponse.Builder()
        .setHeader("Content-Type", "text/event-stream")
        .body(data.joinToString("") { "data: $it\n\n" })
        .build()

    @Test
    fun `sends an OpenAI-compatible streaming request`() = runBlocking {
        server.enqueue(events("[DONE]"))
        val messages = listOf(
            ChatMessage(ChatMessage.Role.User, "Was ist das?", listOf("QUJD")),
            ChatMessage(ChatMessage.Role.Assistant, "Ein Hund."),
            ChatMessage(ChatMessage.Role.User, "Und die Rasse?"),
        )

        api.complete(settings, messages)

        val request = server.takeRequest()
        assertEquals("/v1/chat/completions", request.url.encodedPath)
        assertEquals("Bearer nvapi-test", request.headers["Authorization"])
        assertEquals("text/event-stream", request.headers["Accept"])
        val body = Json.parseToJsonElement(request.body!!.utf8()).jsonObject
        assertEquals(DirectSettings.DEFAULT_MODEL, body["model"]!!.jsonPrimitive.content)
        assertTrue(body["stream"]!!.jsonPrimitive.boolean)
        assertEquals(1.0, body["temperature"]!!.jsonPrimitive.double, 0.0)
        assertEquals(0.95, body["top_p"]!!.jsonPrimitive.double, 0.0)
        assertEquals(16384, body["max_tokens"]!!.jsonPrimitive.int)
        assertEquals(false, body["chat_template_kwargs"]!!.jsonObject["enable_thinking"]!!.jsonPrimitive.boolean)

        val sent = body["messages"]!!.jsonArray.map { it.jsonObject }
        assertEquals(listOf("system", "user", "assistant", "user"), sent.map { it["role"]!!.jsonPrimitive.content })
        assertEquals(DirectSettings.DEFAULT_SYSTEM_PROMPT, sent[0]["content"]!!.jsonPrimitive.content)
        // Photos: the text first, then each image as a data URL.
        val parts = sent[1]["content"]!!.jsonArray.map { it.jsonObject }
        assertEquals(listOf("text", "image_url"), parts.map { it["type"]!!.jsonPrimitive.content })
        assertEquals("Was ist das?", parts[0]["text"]!!.jsonPrimitive.content)
        assertEquals("data:image/jpeg;base64,QUJD", parts[1]["image_url"]!!.jsonObject["url"]!!.jsonPrimitive.content)
        assertEquals("Und die Rasse?", sent[3]["content"]!!.jsonPrimitive.content)
    }

    @Test
    fun `streams thinking and answer`() = runBlocking {
        server.enqueue(
            events(
                """{"choices":[{"delta":{"reasoning_content":"Hmm."}}]}""",
                """{"choices":[{"delta":{"content":"Ein "}}]}""",
                """{"choices":[{"delta":{"content":"Labrador."},"finish_reason":"stop"}]}""",
                "[DONE]",
            ),
        )
        val updates = mutableListOf<Reply>()

        val reply = api.complete(settings, listOf(ChatMessage(ChatMessage.Role.User, "Rasse?")), updates::add)

        assertEquals(Reply("Hmm.", "Ein Labrador.", "stop"), reply)
        assertTrue(updates.isNotEmpty())
        assertEquals("Hmm.", updates.first().thinking)
    }

    @Test
    fun `accepts an answer that was not streamed`() = runBlocking {
        server.enqueue(
            MockResponse.Builder()
                .setHeader("Content-Type", "application/json")
                .body("""{"choices":[{"message":{"content":"<think>kurz</think>Ja."},"finish_reason":"stop"}]}""")
                .build(),
        )
        assertEquals(Reply("kurz", "Ja.", "stop"), api.complete(settings, listOf(ChatMessage(ChatMessage.Role.User, "?"))))
    }

    @Test
    fun `explains HTTP errors in German`() {
        server.enqueue(MockResponse.Builder().code(401).body("""{"status":401,"title":"Unauthorized","detail":"Authentication failed"}""").build())
        server.enqueue(MockResponse.Builder().code(429).body("slow down").build())
        server.enqueue(MockResponse.Builder().code(404).body("").build())
        val messages = listOf(ChatMessage(ChatMessage.Role.User, "Hallo"))

        val unauthorized = assertThrows(DirectException::class.java) { runBlocking { api.complete(settings, messages) } }
        assertEquals(
            "HTTP 401: API-Schlüssel fehlt, ist ungültig oder hat keinen Zugriff auf dieses Modell.\nAuthentication failed",
            unauthorized.message,
        )
        val tooMany = assertThrows(DirectException::class.java) { runBlocking { api.complete(settings, messages) } }
        assertEquals("HTTP 429: Zu viele Anfragen. Kurz warten und erneut versuchen.\nslow down", tooMany.message)
        val notFound = assertThrows(DirectException::class.java) { runBlocking { api.complete(settings, messages) } }
        assertTrue(notFound.message!!.startsWith("HTTP 404: Modell oder Adresse nicht gefunden."))
    }

    @Test
    fun `an error inside the stream ends the answer`() {
        server.enqueue(events("""{"choices":[{"delta":{"content":"Hal"}}]}""", """{"error":{"message":"Model overloaded"}}"""))
        val error = assertThrows(DirectException::class.java) {
            runBlocking { api.complete(settings, listOf(ChatMessage(ChatMessage.Role.User, "Hallo"))) }
        }
        assertEquals("Model overloaded", error.message)
    }

    @Test
    fun `a bad base URL is reported before sending`() {
        val error = assertThrows(DirectException::class.java) {
            runBlocking { api.complete(settings.copy(baseUrl = "kein url"), emptyList()) }
        }
        assertTrue(error.message!!.startsWith("Die Basis-URL ist ungültig"))
        assertEquals(0, server.requestCount)
    }
}
