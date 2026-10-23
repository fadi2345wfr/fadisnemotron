package de.fadi.nemotron.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PromptHistoryTest {
    private var stored: String? = null

    private fun history() = PromptHistory(load = { stored }, save = { stored = it })

    @Test
    fun `arrows walk back and forth and return to the draft`() {
        val h = history()
        assertNull(h.older("halb"))
        h.add("eins")
        h.add("zwei")
        h.add("drei")

        assertEquals("drei", h.older("halb getippt"))
        assertTrue(h.isBrowsing("drei"))
        assertFalse(h.isBrowsing("drei!"))
        assertEquals("zwei", h.older("drei"))
        assertEquals("eins", h.older("zwei"))
        assertNull(h.older("eins"))
        assertEquals("zwei", h.newer())
        assertEquals("drei", h.newer())
        assertEquals("halb getippt", h.newer())
        assertNull(h.newer())
    }

    @Test
    fun `repeated and empty messages are handled and sending ends browsing`() {
        val h = history()
        h.add("  Mach das  ")
        h.add("")
        h.add("   ")
        h.add("Anderes")
        h.add("Mach das")

        assertEquals(listOf("Mach das", "Anderes"), h.recent)
        assertEquals("Mach das", h.older(""))
        h.add("neu")
        assertFalse(h.isBrowsing("Mach das"))
        assertNull(h.newer())
    }

    @Test
    fun `survives a restart, stays small and ignores damaged data`() {
        val first = history()
        repeat(60) { first.add("Auftrag $it") }

        val again = history()
        assertEquals(50, again.recent.size)
        assertEquals("Auftrag 59", again.recent.first())
        assertEquals("Auftrag 10", again.recent.last())

        stored = "{kaputt"
        assertTrue(history().isEmpty)
    }
}
