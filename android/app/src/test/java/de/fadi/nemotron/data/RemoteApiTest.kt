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
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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
        answer(body = """{"ok": true, "app": "Fadis Nemotron", "version": "0.4.0", "protocol": 2, "name": "MacBook"}""")
        answer()

        val ping = api.ping()
        assertEquals("MacBook", ping.name)
        assertEquals(2, ping.protocol)
        assertTrue(ping.supported)
        api.stop()

        val request = mac.takeRequest()
        assertEquals("GET", request.method)
        assertEquals("/api/v1/ping", request.url.encodedPath)
        assertEquals("Bearer geheim123", request.headers["Authorization"])
        val stop = mac.takeRequest()
        assertEquals("POST", stop.method)
        assertEquals("/api/v1/stop", stop.url.encodedPath)
        assertEquals("Bearer geheim123", stop.headers["Authorization"])
    }

    @Test
    fun `a Mac app without protocol 2 is too old`() = runBlocking {
        answer(body = """{"ok": true, "app": "Fadis Nemotron", "version": "0.3.0", "protocol": 1, "name": "MacBook"}""")
        answer(body = """{"ok": true, "name": "MacBook"}""")
        answer(body = """{"ok": true, "protocol": 3}""")

        assertFalse(api.ping().supported)
        assertFalse(api.ping().supported)
        assertTrue(api.ping().supported)
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
    fun `uploads raw bytes with name and type`() = runBlocking {
        answer(body = """{"id": "up-7f3a9c", "kind": "video", "size": 200000}""")
        val bytes = ByteArray(200_000) { (it % 251).toByte() }
        val progress = mutableListOf<Long>()

        val result = api.upload("Urlaub am Meer.mp4", "video/mp4", UploadBody.of(bytes)) { progress += it }

        assertEquals(UploadResult("up-7f3a9c", "video", 200_000), result)
        val request = mac.takeRequest()
        assertEquals("POST", request.method)
        assertEquals("/api/v1/upload", request.url.encodedPath)
        assertEquals("Urlaub am Meer.mp4", request.url.queryParameter("name"))
        assertEquals("video/mp4", request.headers["Content-Type"])
        assertEquals("200000", request.headers["Content-Length"])
        assertEquals("Bearer geheim123", request.headers["Authorization"])
        assertArrayEquals(bytes, request.body!!.toByteArray())
        assertEquals(200_000L, progress.last())
        assertEquals(progress.sorted(), progress)
    }

    @Test
    fun `an upload the Mac refuses shows its reason`() {
        answer(413, """{"error": "Das Video ist größer als 100 MB."}""")
        val error = assertThrows(RemoteException::class.java) {
            runBlocking { api.upload("clip.mp4", "video/mp4", UploadBody.of(ByteArray(10))) }
        }
        assertEquals(413, error.status)
        assertEquals("Das Video ist größer als 100 MB.", error.message)
    }

    @Test
    fun `sends text with the upload ids`() = runBlocking {
        answer()

        api.send("Was passiert in dem Video?", listOf("up-7f3a9c", "up-0b1d2e"))

        val request = mac.takeRequest()
        assertEquals("/api/v1/send", request.url.encodedPath)
        assertTrue(request.headers["Content-Type"]!!.startsWith("application/json"))
        val body = request.json()
        assertEquals(setOf("text", "uploads"), body.keys)
        assertEquals("Was passiert in dem Video?", body["text"]!!.jsonPrimitive.content)
        assertEquals(listOf("up-7f3a9c", "up-0b1d2e"), body["uploads"]!!.jsonArray.map { it.jsonPrimitive.content })
    }

    @Test
    fun `opens, deletes and starts chats`() = runBlocking {
        repeat(3) { answer() }

        api.openChat("c-20261021-180301-b7c1")
        api.deleteChat("c-20261022-104512-3f9a")
        api.newChat()

        val open = mac.takeRequest()
        assertEquals("/api/v1/open_chat", open.url.encodedPath)
        assertEquals(mapOf("id" to "c-20261021-180301-b7c1"), open.json().mapValues { it.value.jsonPrimitive.content })
        val delete = mac.takeRequest()
        assertEquals("/api/v1/delete_chat", delete.url.encodedPath)
        assertEquals(mapOf("id" to "c-20261022-104512-3f9a"), delete.json().mapValues { it.value.jsonPrimitive.content })
        val new = mac.takeRequest()
        assertEquals("POST", new.method)
        assertEquals("/api/v1/new_chat", new.url.encodedPath)
    }

    @Test
    fun `chat actions explain a missing chat and a busy Mac`() {
        answer(404, """{"error": "Not found"}""")
        answer(409, """{"error": "Conflict"}""")
        answer(409, "")

        val missing = assertThrows(RemoteException::class.java) { runBlocking { api.openChat("weg") } }
        assertEquals(404, missing.status)
        assertEquals("Chat nicht gefunden.", missing.message)
        val busy = assertThrows(RemoteException::class.java) { runBlocking { api.deleteChat("c-1") } }
        assertEquals(409, busy.status)
        assertEquals("Nemotron arbeitet noch.", busy.message)
        val busyNew = assertThrows(RemoteException::class.java) { runBlocking { api.newChat() } }
        assertEquals("Nemotron arbeitet noch.", busyNew.message)
    }

    @Test
    fun `an expired upload shows the Mac's reason`() {
        answer(404, """{"error": "Der Upload ist abgelaufen. Bitte noch einmal senden."}""")
        val error = assertThrows(RemoteException::class.java) { runBlocking { api.send("Hallo", listOf("up-alt")) } }
        assertEquals(404, error.status)
        assertEquals("Der Upload ist abgelaufen. Bitte noch einmal senden.", error.message)
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
        val error = assertThrows(RemoteException::class.java) { runBlocking { api.send("Hallo") } }
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
