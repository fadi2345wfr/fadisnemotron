package de.fadi.nemotron.data

import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.RecordedRequest
import okhttp3.OkHttpClient
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

class RemoteApiTest {
    private val mac = MockWebServer()
    private lateinit var api: RemoteApi

    @Before
    fun start() {
        mac.start()
        api = RemoteApi(mac.url("/").toString(), "geheim123", OkHttpClient())
    }

    @After
    fun stop() = mac.close()

    private fun answer(code: Int = 200, body: String = """{"ok": true}""") =
        mac.enqueue(MockResponse.Builder().code(code).setHeader("Content-Type", "application/json").body(body).build())

    private fun RecordedRequest.json(): JsonObject = Json.parseToJsonElement(body!!.utf8()).jsonObject

    @Test
    fun `every request carries the key`() = runBlocking {
        answer(body = """{"ok": true, "app": "Fadis Nemotron", "version": "0.3.0", "protocol": 1, "name": "MacBook"}""")
        answer()

        assertEquals("MacBook", api.ping().name)
        api.stop()

        val ping = mac.takeRequest()
        assertEquals("GET", ping.method)
        assertEquals("/api/v1/ping", ping.url.encodedPath)
        assertEquals("Bearer geheim123", ping.headers["Authorization"])
        val stop = mac.takeRequest()
        assertEquals("POST", stop.method)
        assertEquals("/api/v1/stop", stop.url.encodedPath)
        assertEquals("Bearer geheim123", stop.headers["Authorization"])
    }

    @Test
    fun `long-polls with the last known version`() = runBlocking {
        answer(body = """{"version": 43, "busy": false, "messages": []}""")

        val state = api.state(after = 42)

        assertEquals(43L, state.version)
        val request = mac.takeRequest()
        assertEquals("/api/v1/state", request.url.encodedPath)
        assertEquals("42", request.url.queryParameter("after"))
        assertEquals("25", request.url.queryParameter("wait"))
    }

    @Test
    fun `sends text and photos`() = runBlocking {
        answer()

        api.send("Bau das Projekt", listOf(ImageUpload("foto.jpg", "AAEC"), ImageUpload("bild.jpg", "/9j/")))

        val request = mac.takeRequest()
        assertEquals("/api/v1/send", request.url.encodedPath)
        assertTrue(request.headers["Content-Type"]!!.startsWith("application/json"))
        val body = request.json()
        assertEquals("Bau das Projekt", body["text"]!!.jsonPrimitive.content)
        val images = body["images"]!!.jsonArray.map { it.jsonObject }
        assertEquals(listOf("foto.jpg", "bild.jpg"), images.map { it["name"]!!.jsonPrimitive.content })
        assertEquals(listOf("AAEC", "/9j/"), images.map { it["data"]!!.jsonPrimitive.content })
    }

    @Test
    fun `decisions, settings and undo use the protocol fields`() = runBlocking {
        repeat(3) { answer() }

        api.decide("chatcmpl-tool-1", Decision.AllowForRun)
        api.settings(approval = ApprovalMode.AskAll)
        api.undo("src/main.rs")

        val decide = mac.takeRequest().json()
        assertEquals("chatcmpl-tool-1", decide["call_id"]!!.jsonPrimitive.content)
        assertEquals("allow_for_run", decide["decision"]!!.jsonPrimitive.content)
        // Fields left out stay as they are on the Mac, so they must not be sent as null.
        assertEquals(setOf("approval"), mac.takeRequest().json().keys)
        assertEquals("src/main.rs", mac.takeRequest().json()["path"]!!.jsonPrimitive.content)
    }

    @Test
    fun `loads a diff`() = runBlocking {
        answer(body = """{"path": "src/a b.rs", "diff": "--- a/src/a b.rs\n+++ b/src/a b.rs\n+neu"}""")

        assertEquals("--- a/src/a b.rs\n+++ b/src/a b.rs\n+neu", api.diff("src/a b.rs"))
        assertEquals("src/a b.rs", mac.takeRequest().url.queryParameter("path"))
    }

    @Test
    fun `a wrong key asks to pair again`() {
        answer(401, """{"error": "Falscher oder fehlender Schlüssel."}""")
        val error = assertThrows(RemoteException::class.java) { runBlocking { api.state(after = 0) } }
        assertEquals(401, error.status)
        assertTrue(error.message!!.startsWith("Falscher Schlüssel"))
    }

    @Test
    fun `sending while Nemotron works says so`() {
        answer(409, """{"error": "Conflict"}""")
        val error = assertThrows(RemoteException::class.java) { runBlocking { api.send("Hallo", emptyList()) } }
        assertEquals(409, error.status)
        assertEquals("Nemotron arbeitet noch.", error.message)
    }

    @Test
    fun `other errors show the Mac's own text or a fallback`() {
        answer(409, """{"error": "README.md wurde inzwischen geändert. Bitte am Mac bestätigen."}""")
        answer(503, "")
        val undo = assertThrows(RemoteException::class.java) { runBlocking { api.undo("README.md") } }
        assertEquals("README.md wurde inzwischen geändert. Bitte am Mac bestätigen.", undo.message)
        val stop = assertThrows(RemoteException::class.java) { runBlocking { api.stop() } }
        assertEquals(503, stop.status)
        assertEquals("Die Mac-App antwortet nicht. Ist sie geöffnet?", stop.message)
    }

    @Test
    fun `an unreadable answer is reported, not thrown raw`() {
        answer(body = "<html>Router-Login</html>")
        val error = assertThrows(RemoteException::class.java) { runBlocking { api.state(after = 0) } }
        assertNull(error.status)
    }
}
