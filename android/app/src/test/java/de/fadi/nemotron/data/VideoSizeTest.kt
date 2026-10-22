package de.fadi.nemotron.data

import org.junit.Assert.assertEquals
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
}
