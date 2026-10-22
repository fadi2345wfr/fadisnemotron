package de.fadi.nemotron.ui.mac

import org.junit.Assert.assertEquals
import org.junit.Test

class DraftTest {
    @Test
    fun `the sent text leaves the field`() {
        assertEquals("", draftAfterSending("Was ist auf dem Video? ", sent = "Was ist auf dem Video? "))
        assertEquals("", draftAfterSending("", sent = ""))
    }

    @Test
    fun `text typed during the upload stays`() {
        assertEquals(
            "und schneide es auf 10 s",
            draftAfterSending("Was ist auf dem Video?\nund schneide es auf 10 s", sent = "Was ist auf dem Video?"),
        )
        assertEquals("Noch eine Frage", draftAfterSending("Noch eine Frage", sent = ""))
    }

    @Test
    fun `an edited draft is kept whole`() {
        assertEquals("Was ist auf den Fotos?", draftAfterSending("Was ist auf den Fotos?", sent = "Was ist auf dem Video?"))
    }
}
