package de.fadi.nemotron.data

import java.util.Base64
import kotlinx.coroutines.runBlocking
import okhttp3.OkHttpClient
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test

/**
 * Runs against a real Mac app instead of a mock, to catch protocol drift.
 * Skipped unless FADIS_LIVE_URL and FADIS_LIVE_TOKEN are set; the Mac app
 * needs a project with README.md containing "Hallo Welt" and a model server
 * that edits it (the desktop repo's test mock does).
 */
class RemoteApiLiveTest {
    private val url = System.getenv("FADIS_LIVE_URL").orEmpty()
    private val token = System.getenv("FADIS_LIVE_TOKEN").orEmpty()

    @Test
    fun `phone drives an agent run on the real Mac app`() = runBlocking {
        assumeTrue(url.isNotEmpty() && token.isNotEmpty())
        val api = RemoteApi(url, token, OkHttpClient())

        val ping = api.ping()
        assertTrue(ping.name.isNotEmpty())
        assertTrue("protocol ${ping.protocol}", ping.supported)
        val wrongKey = RemoteApi(url, "falsch", OkHttpClient())
        assertEquals(401, assertThrows(RemoteException::class.java) { runBlocking { wrongKey.ping() } }.status)

        api.newChat()
        api.settings(approval = ApprovalMode.AutoEdits)
        api.settings(thinking = true)
        var state = api.state(after = 0, wait = 0)
        assertEquals(ApprovalMode.AutoEdits, state.approval)
        assertTrue(state.messages.isEmpty())
        val editChat = state.chat!!.id
        assertTrue(state.chats.any { it.id == editChat })

        api.send("Ändere die Begrüßung in der README")
        assertEquals(409, assertThrows(RemoteException::class.java) {
            runBlocking { api.send("Noch was") }
        }.status)
        assertEquals(409, assertThrows(RemoteException::class.java) { runBlocking { api.newChat() } }.status)

        val seen = mutableSetOf<ToolState>()
        for (round in 0 until 60) {
            state = api.state(after = state.version, wait = 5)
            val tools = state.messages.lastOrNull()?.steps.orEmpty().flatMap { it.tools }
            tools.forEach { seen += it.state }
            tools.firstOrNull { it.state == ToolState.AwaitingApproval }?.let { api.decide(it.id, Decision.Allow) }
            if (!state.busy && state.messages.lastOrNull()?.status == MessageStatus.Done) break
        }
        val answer = state.messages.last()
        assertEquals(MessageStatus.Done, answer.status)
        assertTrue(answer.steps.any { it.reasoning.isNotEmpty() })
        assertTrue(ToolState.AwaitingApproval in seen)
        val edit = answer.steps.flatMap { it.tools }.first { it.name == "edit_file" }
        assertEquals(ToolState.Done, edit.state)
        assertTrue(edit.diff, edit.diff.contains("+Hallo Nemotron"))

        val change = state.changes.single()
        assertEquals("README.md", change.path)
        assertTrue(api.diff("README.md").contains("-Hallo Welt"))
        api.undo("README.md")
        assertTrue(api.state(after = 0, wait = 0).changes.isEmpty())

        // A photo and a video go up as raw bytes first. Asking first keeps the project untouched.
        val jpeg = Base64.getDecoder().decode(JPEG)
        api.settings(approval = ApprovalMode.AskAll)
        api.newChat()
        val photoChat = api.state(after = 0, wait = 0).chat!!.id
        assertNotEquals(editChat, photoChat)
        val upload = api.upload("foto.jpg", "image/jpeg", UploadBody.of(jpeg))
        assertEquals("image", upload.kind)
        val clip = api.upload("clip.mp4", "video/mp4", UploadBody.of(byteArrayOf(0, 0, 0, 24, 102, 116, 121, 112)))
        assertEquals("video", clip.kind)
        api.send("Was siehst du?", listOf(upload.id, clip.id))
        state = api.state(after = 0, wait = 0)
        assertEquals(listOf("foto.jpg", "clip.mp4"), state.messages.first().attachments)
        api.stop()
        for (round in 0 until 12) {
            if (!state.busy) break
            state = api.state(after = state.version, wait = 5)
        }
        assertFalse(state.busy)

        // The chat list is the Mac's: switch back, then delete the photo chat.
        api.openChat(editChat)
        state = api.state(after = 0, wait = 0)
        assertEquals(editChat, state.chat?.id)
        assertTrue(state.messages.isNotEmpty())
        api.deleteChat(photoChat)
        state = api.state(after = 0, wait = 0)
        assertFalse(state.chats.any { it.id == photoChat })
        assertEquals(editChat, state.chat?.id)
        assertEquals(404, assertThrows(RemoteException::class.java) { runBlocking { api.openChat(photoChat) } }.status)
        api.settings(approval = ApprovalMode.AutoEdits)
    }

    private companion object {
        const val JPEG = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/2wBDAQkJCQwLDBgNDRgyIRwhMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjL/wAARCAAIAAgDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwDQooor83PPP//Z"
    }
}
