package de.fadi.nemotron.ui.common

import java.time.ZoneId
import java.time.ZonedDateTime
import org.junit.Assert.assertEquals
import org.junit.Test

class FormatsTest {
    private val berlin = ZoneId.of("Europe/Berlin")
    private val now = ZonedDateTime.of(2026, 10, 22, 14, 30, 0, 0, berlin)

    private fun ago(minutes: Long) = now.minusMinutes(minutes).toEpochSecond()

    @Test
    fun `recent chats say how long ago`() {
        assertEquals("gerade eben", relativeTime(now.minusSeconds(20).toEpochSecond(), now))
        assertEquals("vor 1 Min.", relativeTime(ago(1), now))
        assertEquals("vor 5 Min.", relativeTime(ago(5), now))
        assertEquals("vor 59 Min.", relativeTime(ago(59), now))
        assertEquals("vor 1 Std.", relativeTime(ago(60), now))
        assertEquals("vor 14 Std.", relativeTime(now.withHour(0).withMinute(5).toEpochSecond(), now))
    }

    @Test
    fun `older chats show the day`() {
        assertEquals("gestern", relativeTime(now.minusDays(1).withHour(23).toEpochSecond(), now))
        assertEquals("gestern", relativeTime(now.minusDays(1).withHour(0).toEpochSecond(), now))
        assertEquals("20.10.", relativeTime(now.minusDays(2).toEpochSecond(), now))
        assertEquals("3.1.", relativeTime(ZonedDateTime.of(2026, 1, 3, 9, 0, 0, 0, berlin).toEpochSecond(), now))
        assertEquals("31.12.2025", relativeTime(ZonedDateTime.of(2025, 12, 31, 9, 0, 0, 0, berlin).toEpochSecond(), now))
    }

    @Test
    fun `a Mac clock running ahead counts as just now`() {
        assertEquals("gerade eben", relativeTime(now.plusMinutes(3).toEpochSecond(), now))
    }

    @Test
    fun `video durations`() {
        assertEquals("0:00", formatDuration(0))
        assertEquals("0:42", formatDuration(42_300))
        assertEquals("1:00", formatDuration(59_600))
        assertEquals("12:05", formatDuration(725_000))
        assertEquals("1:02:03", formatDuration(3_723_000))
    }

    @Test
    fun `upload progress in megabytes`() {
        val mb = 1024L * 1024
        assertEquals("3,2 / 14 MB", formatMegabytes(3 * mb + mb / 5, 14 * mb))
        assertEquals("0,0 / 0,5 MB", formatMegabytes(0, mb / 2))
        assertEquals("98 / 100 MB", formatMegabytes(98 * mb, 100 * mb))
    }
}
