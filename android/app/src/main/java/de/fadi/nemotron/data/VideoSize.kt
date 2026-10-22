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

/**
 * Video bits per second for a shrunk video of [size]: 2 Mbit/s at 720p, less
 * for fewer pixels, and never more than the original spent on average
 * ([sourceBitrate], 0 if unknown), so re-encoding a lean clip does not make
 * it bigger.
 */
fun videoBitrate(size: VideoSize, sourceBitrate: Long): Int {
    val forSize = MAX_VIDEO_BITRATE * size.width.toLong() * size.height / PIXELS_720P
    val bitrate = if (sourceBitrate > 0) minOf(forSize, sourceBitrate) else forSize
    return bitrate.coerceIn(MIN_VIDEO_BITRATE, MAX_VIDEO_BITRATE).toInt()
}

/** The average bits per second of a file; 0 if its size or duration is unknown. */
fun averageBitrate(bytes: Long, durationMs: Long): Long =
    if (bytes > 0 && durationMs > 0) bytes * 8_000 / durationMs else 0

/** About how many bytes the shrunk video takes, with AAC audio if [hasAudio]. */
fun estimatedVideoBytes(durationMs: Long, videoBitrate: Int, hasAudio: Boolean): Long =
    durationMs * (videoBitrate + if (hasAudio) AUDIO_BITRATE else 0) / 8_000

const val MAX_VIDEO_SIDE = 1280
private const val PIXELS_720P = 1280L * 720
private const val MAX_VIDEO_BITRATE = 2_000_000L
private const val MIN_VIDEO_BITRATE = 200_000L
private const val AUDIO_BITRATE = 128_000
