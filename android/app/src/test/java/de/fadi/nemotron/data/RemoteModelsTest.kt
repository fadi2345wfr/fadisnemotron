package de.fadi.nemotron.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteModelsTest {

    private fun parse(json: String): RemoteState = remoteJson.decodeFromString(json)

    @Test
    fun `parses the full example from the protocol description`() {
        val state = parse(DOC_EXAMPLE)

        assertEquals(42L, state.version)
        assertTrue(state.busy)
        assertTrue(state.thinking)
        assertEquals(ApprovalMode.AutoEdits, state.approval)
        assertEquals("proj", state.workspace?.name)
        assertEquals(Notice("README.md wiederhergestellt.", error = false), state.notice)
        assertEquals(listOf(Change("README.md", added = 1, removed = 1, created = false)), state.changes)

        val (user, assistant) = state.messages
        assertTrue(user.isUser)
        assertEquals("Ändere die README", user.text)
        assertEquals(listOf("foto.jpg"), user.attachments)

        assertFalse(assistant.isUser)
        assertEquals(MessageStatus.Streaming, assistant.status)
        assertNull(assistant.error)
        assertNull(assistant.finishReason)
        val step = assistant.steps.single()
        assertEquals("Ich lese zuerst …", step.reasoning)
        assertEquals("Markdown-Text", step.content)
        val tool = step.tools.single()
        assertEquals("chatcmpl-tool-1", tool.id)
        assertEquals("edit_file", tool.name)
        assertEquals("Bearbeitet: README.md", tool.title)
        assertEquals(ToolState.Done, tool.state)
        assertTrue(tool.ok)
        assertEquals("Updated README.md (+1 -1 lines, 2 lines total).", tool.output)
        assertTrue(tool.diff.startsWith("--- a/README.md\n+++ b/README.md"))
        assertEquals(1, tool.added)
        assertEquals(1, tool.removed)
        assertNull(tool.warning)
    }

    @Test
    fun `a chat without project folder has no workspace`() {
        val state = parse("""{"version": 3, "busy": false, "workspace": null, "approval": "AskAll", "messages": []}""")
        assertNull(state.workspace)
        assertEquals(ApprovalMode.AskAll, state.approval)
        assertTrue(state.messages.isEmpty())
    }

    @Test
    fun `skips unknown fields and unknown enum values`() {
        val state = parse(
            """
            {"version": 7, "approval": "SomethingNew", "theme": "dark", "workspace": {"name": "x", "path": "/x", "git": true},
             "messages": [{"role": "assistant", "status": "paused", "mood": 1,
               "steps": [{"content": "Hallo", "tools": [{"id": "t1", "name": "browse", "state": "teleporting", "eta": 3}]}]}]}
            """,
        )
        assertEquals(7L, state.version)
        assertEquals(ApprovalMode.AutoEdits, state.approval)
        assertEquals("x", state.workspace?.name)
        val message = state.messages.single()
        assertEquals(MessageStatus.Done, message.status)
        val tool = message.steps.single().tools.single()
        assertEquals(ToolState.Planned, tool.state)
        assertEquals("browse", tool.name)
    }

    @Test
    fun `missing and null values fall back to defaults`() {
        // The Mac publishes an empty object until its window has drawn once.
        val empty = parse("""{"version": 1}""")
        assertEquals(1L, empty.version)
        assertFalse(empty.busy)
        assertNull(empty.notice)
        assertTrue(empty.changes.isEmpty())

        val state = parse(
            """
            {"messages": [{"role": "assistant", "status": "failed", "error": "HTTP 401", "notices": null,
               "steps": [{"reasoning": null, "content": null, "tools": [{"id": "c", "state": "running", "ok": null,
               "output": null, "diff": null, "added": null, "warning": "löscht Dateien"}]}]}]}
            """,
        )
        val message = state.messages.single()
        assertEquals(MessageStatus.Failed, message.status)
        assertEquals("HTTP 401", message.error)
        assertTrue(message.notices.isEmpty())
        val step = message.steps.single()
        assertEquals("", step.reasoning)
        assertEquals("", step.content)
        val tool = step.tools.single()
        assertEquals(ToolState.Running, tool.state)
        assertTrue(tool.ok)
        assertEquals("", tool.output)
        assertEquals(0, tool.added)
        assertEquals("löscht Dateien", tool.warning)
    }

    @Test
    fun `tool states and decisions use the protocol names`() {
        val names = listOf("planned", "awaiting_approval", "running", "done", "declined", "not_run")
        val states = names.map { remoteJson.decodeFromString<Tool>("""{"state": "$it"}""").state }
        assertEquals(ToolState.entries.toList(), states)
        assertEquals("\"allow_for_run\"", remoteJson.encodeToString(Decision.AllowForRun))
    }

    @Test
    fun `thinking is split from the answer`() {
        assertEquals("Plan" to "Antwort", thinkingAndAnswer("  Plan ", "Antwort"))
        assertEquals("Plan" to "Antwort", thinkingAndAnswer("", "<think>\nPlan\n</think>\n\nAntwort"))
        assertEquals("Plan" to "Antwort", thinkingAndAnswer("", "Plan</think>Antwort"))
        assertEquals("noch am Denken" to "", thinkingAndAnswer("", "<think>noch am Denken"))
        assertEquals("" to "Nur Antwort", thinkingAndAnswer("", "Nur Antwort"))
    }

    private companion object {
        // Verbatim from docs/remote-api.md.
        const val DOC_EXAMPLE = """
{
  "version": 42,
  "busy": true,
  "model": "nvidia/nemotron-3.5-super-vl-120b-a12b",
  "thinking": true,
  "approval": "AutoEdits",
  "workspace": {"name": "proj", "path": "/Users/fadi/proj"},
  "notice": {"text": "README.md wiederhergestellt.", "error": false},
  "changes": [{"path": "README.md", "added": 1, "removed": 1, "created": false}],
  "messages": [
    {"role": "user", "text": "Ändere die README", "attachments": ["foto.jpg"]},
    {
      "role": "assistant",
      "status": "streaming",
      "error": null,
      "finish_reason": null,
      "notices": [],
      "steps": [
        {
          "reasoning": "Ich lese zuerst …",
          "content": "Markdown-Text",
          "tools": [
            {
              "id": "chatcmpl-tool-1",
              "name": "edit_file",
              "title": "Bearbeitet: README.md",
              "state": "done",
              "ok": true,
              "output": "Updated README.md (+1 -1 lines, 2 lines total).",
              "diff": "--- a/README.md\n+++ b/README.md\n…",
              "added": 1,
              "removed": 1,
              "warning": null
            }
          ]
        }
      ]
    }
  ]
}
"""
    }
}
