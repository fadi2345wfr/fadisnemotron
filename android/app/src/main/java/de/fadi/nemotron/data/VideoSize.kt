package de.fadi.nemotron.data

import kotlin.math.max
import kotlin.math.roundToInt

data class VideoSize(val width: Int, val height: Int)

/**
 * The size a video is shrunk to: the longest side at most [maxSide] (720p),
 * same aspect ratio, never larger than the original. Video encoders want
 * even sizes, so odd ones are rounded down.
 */
fun videoTargetSize(width: Int, height: Int, maxSide: Int = MAX_VIDEO_SIDE): VideoSize {
    val scale = minOf(1.0, maxSide.toDouble() / max(width, height))
    fun even(side: Int) = ((side * scale).roundToInt() / 2 * 2).coerceAtLeast(2)
    return VideoSize(even(width), even(height))
}

const val MAX_VIDEO_SIDE = 1280
