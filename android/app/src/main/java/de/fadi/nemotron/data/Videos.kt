package de.fadi.nemotron.data

import android.content.Context
import android.graphics.Bitmap
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.os.Build
import androidx.annotation.OptIn
import androidx.core.graphics.scale
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.util.UnstableApi
import androidx.media3.effect.Presentation
import androidx.media3.transformer.Composition
import androidx.media3.transformer.DefaultEncoderFactory
import androidx.media3.transformer.EditedMediaItem
import androidx.media3.transformer.EditedMediaItemSequence
import androidx.media3.transformer.Effects
import androidx.media3.transformer.ExportException
import androidx.media3.transformer.ExportResult
import androidx.media3.transformer.ProgressHolder
import androidx.media3.transformer.Transformer
import androidx.media3.transformer.VideoEncoderSettings
import java.io.File
import java.io.FileNotFoundException
import java.util.UUID
import kotlin.math.max
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import okio.source

/** The shrunk video would be larger than the Mac takes. */
class VideoTooBigException : Exception()

/** A picked video, read from its metadata without decoding it. */
class VideoInfo(
    val name: String,
    val mimeType: String,
    /** Bytes, or -1 if the provider does not say. */
    val size: Long,
    /** As shown, i.e. with the rotation already applied. */
    val width: Int,
    val height: Int,
    val durationMs: Long,
    val hasAudio: Boolean,
    val thumbnail: Bitmap?,
)

/**
 * Shrinks picked videos for the Mac: MP4 with H.264 and AAC, at most 720p
 * and 2 Mbit/s. The work runs on Transformer's own threads; it is only
 * driven from the main thread, as Transformer requires.
 */
@OptIn(UnstableApi::class)
class VideoEncoder(private val context: Context) {
    private val resolver = context.contentResolver
    private val cache = File(context.cacheDir, "videos")

    fun isVideo(uri: Uri): Boolean = resolver.getType(uri)?.startsWith("video/") == true

    suspend fun probe(uri: Uri): VideoInfo = withContext(Dispatchers.IO) {
        val retriever = MediaMetadataRetriever()
        try {
            retriever.setDataSource(context, uri)
            fun number(key: Int) = retriever.extractMetadata(key)?.toLongOrNull() ?: 0
            val width = number(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH).toInt()
            val height = number(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT).toInt()
            val sideways = number(MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION) % 180 != 0L
            VideoInfo(
                name = resolver.displayName(uri) ?: "video",
                mimeType = resolver.getType(uri).orEmpty(),
                size = resolver.fileSize(uri),
                width = if (sideways) height else width,
                height = if (sideways) width else height,
                durationMs = number(MediaMetadataRetriever.METADATA_KEY_DURATION),
                hasAudio = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_HAS_AUDIO) == "yes",
                thumbnail = thumbnail(retriever),
            )
        } finally {
            retriever.release()
        }
    }

    private fun thumbnail(retriever: MediaMetadataRetriever): Bitmap? {
        val option = MediaMetadataRetriever.OPTION_CLOSEST_SYNC
        if (Build.VERSION.SDK_INT >= 27) {
            return retriever.getScaledFrameAtTime(0, option, THUMBNAIL_SIDE, THUMBNAIL_SIDE)
        }
        val frame = retriever.getFrameAtTime(0, option) ?: return null
        val scale = minOf(1f, THUMBNAIL_SIDE.toFloat() / max(frame.width, frame.height))
        return frame.scale(max(1, (frame.width * scale).toInt()), max(1, (frame.height * scale).toInt()))
    }

    /**
     * Writes the shrunk video to the app's cache and returns it; the caller
     * deletes it. [onProgress] gets percent. Cancelling stops the encoder.
     * Throws [VideoTooBigException] as soon as the result would clearly be
     * over [MAX_BYTES], before the whole clip is encoded for nothing.
     */
    suspend fun transcode(uri: Uri, info: VideoInfo, onProgress: (Int) -> Unit): File {
        val target = targetSize(info)
        val bitrate = videoBitrate(target ?: VideoSize(MAX_VIDEO_SIDE, 720), averageBitrate(info.size, info.durationMs))
        // Encoders miss their target in both directions, so only refuse up front with a wide margin.
        if (estimatedVideoBytes(info.durationMs, bitrate, info.hasAudio) > MAX_BYTES * 3 / 2) throw VideoTooBigException()
        val output = withContext(Dispatchers.IO) {
            cache.mkdirs()
            File(cache, "${UUID.randomUUID()}.mp4")
        }
        return withContext(Dispatchers.Main) {
            val result = CompletableDeferred<Unit>()
            val transformer = Transformer.Builder(context)
                .setVideoMimeType(MimeTypes.VIDEO_H264)
                .setAudioMimeType(MimeTypes.AUDIO_AAC)
                .setEncoderFactory(
                    DefaultEncoderFactory.Builder(context)
                        .setRequestedVideoEncoderSettings(VideoEncoderSettings.Builder().setBitrate(bitrate).build())
                        .build(),
                )
                .addListener(object : Transformer.Listener {
                    override fun onCompleted(composition: Composition, exportResult: ExportResult) {
                        result.complete(Unit)
                    }

                    override fun onError(composition: Composition, exportResult: ExportResult, exportException: ExportException) {
                        result.completeExceptionally(exportException)
                    }
                })
                .build()
            try {
                transformer.start(composition(uri, info, target), output.path)
                val progress = ProgressHolder()
                while (withTimeoutOrNull(PROGRESS_INTERVAL_MS) { result.await() } == null) {
                    if (transformer.getProgress(progress) == Transformer.PROGRESS_STATE_AVAILABLE) {
                        onProgress(progress.progress)
                    }
                    if (withContext(Dispatchers.IO) { output.length() } > MAX_BYTES) throw VideoTooBigException()
                }
                output
            } catch (e: Throwable) {
                // Failed, or removed by the user: stop the encoder and drop the half-written file.
                transformer.cancel()
                output.delete()
                throw e
            }
        }
    }

    /** Null if the metadata does not tell the size. */
    private fun targetSize(info: VideoInfo): VideoSize? =
        if (info.width > 0 && info.height > 0) videoTargetSize(info.width, info.height) else null

    private fun composition(uri: Uri, info: VideoInfo, target: VideoSize?): Composition {
        val effects = if (target != null) {
            Effects(emptyList(), listOf(Presentation.createForWidthAndHeight(target.width, target.height, Presentation.LAYOUT_SCALE_TO_FIT)))
        } else {
            Effects.EMPTY
        }
        val item = listOf(EditedMediaItem.Builder(MediaItem.fromUri(uri)).setEffects(effects).build())
        val sequence = if (info.hasAudio) {
            EditedMediaItemSequence.withAudioAndVideoFrom(item)
        } else {
            EditedMediaItemSequence.withVideoFrom(item)
        }
        // HDR clips from phone cameras become SDR, so the result stays plain H.264 that every Mac plays.
        return Composition.Builder(sequence).setHdrMode(Composition.HDR_MODE_TONE_MAP_HDR_TO_SDR_USING_OPEN_GL).build()
    }

    /** The name for the shrunk copy. */
    fun shrunkName(info: VideoInfo): String = "${info.name.substringBeforeLast('.').ifBlank { "video" }}.mp4"

    /**
     * The picked file as it is, for when shrinking fails or does not help:
     * only in a format the Mac takes, and only if its size is known.
     */
    fun original(uri: Uri, info: VideoInfo): Upload? {
        val extension = ORIGINAL_FORMATS[info.mimeType] ?: return null
        if (info.size < 0) return null
        val name = if (info.name.substringAfterLast('.', "").equals(extension, ignoreCase = true)) {
            info.name
        } else {
            "${info.name.substringBeforeLast('.')}.$extension"
        }
        val body = UploadBody(info.size) {
            (resolver.openInputStream(uri) ?: throw FileNotFoundException(uri.toString())).source()
        }
        return Upload(name, info.mimeType, body)
    }

    /** Removes shrunk copies a previous run left behind, e.g. when the app was closed mid-way. */
    suspend fun deleteLeftovers() {
        withContext(Dispatchers.IO) { cache.deleteRecursively() }
    }

    companion object {
        /** The Mac refuses larger videos. */
        const val MAX_BYTES = 100L * 1024 * 1024
        private const val PROGRESS_INTERVAL_MS = 300L
        private val ORIGINAL_FORMATS = mapOf("video/mp4" to "mp4", "video/quicktime" to "mov", "video/webm" to "webm")
    }
}
