package de.fadi.nemotron.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class VideoSizeTest {
    @Test
    fun `landscape and portrait shrink to 720p`() {
        assertEquals(VideoSize(1280, 720), videoTargetSize(1920, 1080))
        assertEquals(VideoSize(720, 1280), videoTargetSize(1080, 1920))
        assertEquals(VideoSize(1280, 720), videoTargetSize(3840, 2160))
        assertEquals(VideoSize(1280, 960), videoTargetSize(4000, 3000))
    }

    @Test
    fun `small videos keep their size`() {
        assertEquals(VideoSize(640, 480), videoTargetSize(640, 480))
        assertEquals(VideoSize(1280, 720), videoTargetSize(1280, 720))
    }

    @Test
    fun `odd sizes become even`() {
        assertEquals(VideoSize(640, 480), videoTargetSize(641, 481))
        assertEquals(VideoSize(1280, 720), videoTargetSize(1921, 1081))
        assertEquals(VideoSize(720, 1280), videoTargetSize(1081, 1921))
        assertEquals(VideoSize(2, 2), videoTargetSize(1, 1))
    }

    @Test
    fun `bitrate is 2 Mbit per second at 720p and less for fewer pixels`() {
        assertEquals(2_000_000, videoBitrate(VideoSize(1280, 720), sourceBitrate = 0))
        assertEquals(2_000_000, videoBitrate(VideoSize(720, 1280), sourceBitrate = 0))
        assertEquals(2_000_000, videoBitrate(VideoSize(1280, 960), sourceBitrate = 0))
        assertEquals(500_000, videoBitrate(VideoSize(640, 360), sourceBitrate = 0))
        assertEquals(200_000, videoBitrate(VideoSize(160, 90), sourceBitrate = 0))
    }

    @Test
    fun `bitrate never exceeds what the original spent`() {
        assertEquals(1_000_000, videoBitrate(VideoSize(1280, 720), sourceBitrate = 1_000_000))
        assertEquals(2_000_000, videoBitrate(VideoSize(1280, 720), sourceBitrate = 16_000_000))
        assertEquals(200_000, videoBitrate(VideoSize(1280, 720), sourceBitrate = 50_000))
    }

    @Test
    fun `average bitrate from size and duration`() {
        assertEquals(750_000L, averageBitrate(45_000_000, durationMs = 480_000))
        assertEquals(0L, averageBitrate(-1, durationMs = 480_000))
        assertEquals(0L, averageBitrate(45_000_000, durationMs = 0))
    }

    @Test
    fun `a lean clip does not grow`() {
        // An 8-minute 640x352 clip from a messenger, 45 MB.
        val bitrate = videoBitrate(VideoSize(640, 352), averageBitrate(45_000_000, durationMs = 480_000))
        assertTrue(estimatedVideoBytes(480_000, bitrate, hasAudio = true) < 45_000_000)
    }

    @Test
    fun `estimated size of the shrunk video`() {
        assertEquals(15_960_000L, estimatedVideoBytes(60_000, 2_000_000, hasAudio = true))
        assertEquals(15_000_000L, estimatedVideoBytes(60_000, 2_000_000, hasAudio = false))
        // 20 minutes of 720p are far over the 100 MB the Mac takes.
        assertTrue(estimatedVideoBytes(20 * 60_000, 2_000_000, hasAudio = true) > 300_000_000)
    }
}
