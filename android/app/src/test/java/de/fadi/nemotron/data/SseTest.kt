package de.fadi.nemotron.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class SseTest {

    private fun stream(vararg lines: String) = ChatStream().apply { lines.forEach { feed(it) } }

    @Test
    fun `thinking arrives first, then the answer`() {
        val stream = stream(
            """data: {"choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"Ich überlege"}}]}""",
            "",
            """data: {"choices":[{"delta":{"reasoning_content":" kurz."}}]}""",
            """data: {"choices":[{"delta":{"content":"Hallo"}}]}""",
            """data:{"choices":[{"delta":{"content":" Welt!"},"finish_reason":"stop"}]}""",
        )
        assertEquals(Reply("Ich überlege kurz.", "Hallo Welt!", "stop"), stream.reply)
        assertFalse(stream.finished)
    }

    @Test
    fun `accepts the reasoning field name of other servers`() {
        val stream = stream(
            """data: {"choices":[{"delta":{"reasoning":"Plan"}}]}""",
            """data: {"choices":[{"delta":{"content":"Antwort","reasoning":null}}]}""",
        )
        assertEquals(Reply("Plan", "Antwort"), stream.reply)
    }

    @Test
    fun `stops at DONE and ignores comments`() {
        val stream = ChatStream()
        assertFalse(stream.feed(": keep-alive"))
        assertFalse(stream.feed("event: message"))
        assertTrue(stream.feed("""data: {"choices":[{"delta":{"content":"Fertig"}}]}"""))
        assertFalse(stream.feed("data: [DONE]"))
        assertTrue(stream.finished)
        assertEquals("Fertig", stream.reply.answer)
    }

    @Test
    fun `chunks without text change nothing`() {
        val stream = ChatStream()
        assertFalse(stream.feed("""data: {"choices":[],"usage":{"total_tokens":12}}"""))
        assertFalse(stream.feed("""data: {"choices":[{"delta":{}}]}"""))
        assertFalse(stream.feed("data: kein json"))
        assertEquals(Reply(), stream.reply)
    }

    @Test
    fun `an error chunk becomes an exception`() {
        val stream = stream("""data: {"choices":[{"delta":{"content":"Hal"}}]}""")
        val error = assertThrows(DirectException::class.java) {
            stream.feed("""data: {"error": {"message": "Context length exceeded", "code": 400}}""")
        }
        assertEquals("Context length exceeded", error.message)
        assertThrows(DirectException::class.java) { ChatStream().feed("""data: {"error": "overloaded"}""") }
        assertThrows(DirectException::class.java) { ChatStream().feed("""data: {"object":"error","message":"bad"}""") }
    }

    @Test
    fun `reads a complete answer that was not streamed`() {
        val reply = parseCompletion(
            """{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"Antwort","reasoning_content":"Plan"},"finish_reason":"length"}]}""",
        )
        assertEquals(Reply("Plan", "Antwort", "length"), reply)
        assertThrows(DirectException::class.java) { parseCompletion("""{"status":401,"title":"Unauthorized","detail":"Bad key"}""") }
        assertThrows(DirectException::class.java) { parseCompletion("<html>502</html>") }
    }

    @Test
    fun `splits inline think tags`() {
        val partial = stream(
            """data: {"choices":[{"delta":{"content":"<think>\nErst"}}]}""",
            """data: {"choices":[{"delta":{"content":" nachdenken"}}]}""",
        )
        assertEquals(Reply("Erst nachdenken", ""), partial.reply)
        partial.feed("""data: {"choices":[{"delta":{"content":"</think>\n\nDie Antwort."}}]}""")
        assertEquals(Reply("Erst nachdenken", "Die Antwort."), partial.reply)
        assertEquals(Reply("Plan", "Ja."), parseCompletion("""{"choices":[{"message":{"content":"<think>Plan</think>Ja."}}]}"""))
    }
}
