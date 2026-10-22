package de.fadi.nemotron.data

import kotlinx.coroutines.runBlocking
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.OkHttpClient
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Before
import org.junit.Test

class PairingTest {
    private val mac = MockWebServer()
    private val http = OkHttpClient()

    @Before
    fun start() = mac.start()

    @After
    fun stop() = mac.close()

    @Test
    fun `reads the QR code of the Mac app`() {
        val code = parsePairingCode(
            """{"fadis": 1, "name": "MacBook", "urls": ["http://100.101.2.3:7878", "http://192.168.1.20:7878"], "token": "abc123"}""",
        )
        assertEquals(PairingCode(1, "MacBook", listOf("http://100.101.2.3:7878", "http://192.168.1.20:7878"), "abc123"), code)
    }

    @Test
    fun `rejects codes without token or addresses`() {
        assertNull(parsePairingCode("""{"fadis": 1, "name": "MacBook", "urls": ["http://192.168.1.20:7878"]}"""))
        assertNull(parsePairingCode("""{"fadis": 1, "name": "MacBook", "urls": ["http://192.168.1.20:7878"], "token": ""}"""))
        assertNull(parsePairingCode("""{"fadis": 1, "urls": [], "token": "abc"}"""))
    }

    @Test
    fun `rejects other QR codes`() {
        assertNull(parsePairingCode("https://example.com"))
        assertNull(parsePairingCode("WIFI:S:Zuhause;T:WPA;P:geheim;;"))
        assertNull(parsePairingCode("{kaputt"))
        assertNull(parsePairingCode("[1, 2, 3]"))
        assertNull(parsePairingCode("""{"urls": ["http://192.168.1.20:7878"], "token": "abc"}"""))
        assertNull(parsePairingCode(""))
    }

    @Test
    fun `normalizes typed addresses`() {
        assertEquals("http://192.168.1.20:7878", normalizeAddress("192.168.1.20"))
        assertEquals("http://192.168.1.20:7878", normalizeAddress(" http://192.168.1.20:7878/ "))
        assertEquals("http://macbook:8080", normalizeAddress("macbook:8080"))
        assertEquals("http://100.64.0.1:7878", normalizeAddress("http://100.64.0.1"))
        assertNull(normalizeAddress(""))
        assertNull(normalizeAddress("http://"))
    }

    @Test
    fun `keeps the first address that answers`() = runBlocking {
        mac.enqueue(MockResponse.Builder().body("""{"ok": true, "name": "Fadis MacBook", "protocol": 2}""").build())
        val unreachable = "http://127.0.0.1:1"
        val reachable = mac.url("/").toString().trimEnd('/')

        val pairing = connect(PairingCode(1, "MacBook", listOf(unreachable, reachable), "abc"), http)

        assertEquals(Pairing(reachable, "abc", "Fadis MacBook"), pairing)
        assertEquals("Bearer abc", mac.takeRequest().headers["Authorization"])
    }

    @Test
    fun `a wrong key stops at the first address`() {
        mac.enqueue(MockResponse.Builder().code(401).body("""{"error": "Falscher Schlüssel"}""").build())
        val first = mac.url("/").toString()
        val error = assertThrows(RemoteException::class.java) {
            runBlocking { connect(PairingCode(1, "MacBook", listOf(first, "http://127.0.0.1:1"), "falsch"), http) }
        }
        assertEquals(401, error.status)
        assertEquals(1, mac.requestCount)
    }

    @Test
    fun `refuses a Mac app that is too old`() {
        mac.enqueue(MockResponse.Builder().body("""{"ok": true, "name": "MacBook", "protocol": 1}""").build())
        val first = mac.url("/").toString()
        val error = assertThrows(RemoteException::class.java) {
            runBlocking { connect(PairingCode(1, "MacBook", listOf(first, "http://127.0.0.1:1"), "abc"), http) }
        }
        assertEquals(RemoteApi.MAC_TOO_OLD, error.message)
        assertEquals(1, mac.requestCount)
    }

    @Test
    fun `reports an unreachable Mac`() {
        val error = assertThrows(RemoteException::class.java) {
            runBlocking { connect(PairingCode(1, "MacBook", listOf("http://127.0.0.1:1"), "abc"), http) }
        }
        assertNull(error.status)
        assertEquals(RemoteApi.UNREACHABLE, error.message)
    }
}
