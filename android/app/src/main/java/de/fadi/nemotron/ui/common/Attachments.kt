package de.fadi.nemotron.ui.common

import android.graphics.Bitmap
import android.net.Uri
import de.fadi.nemotron.data.Photo
import de.fadi.nemotron.data.PhotoEncoder
import de.fadi.nemotron.data.Upload
import de.fadi.nemotron.data.UploadBody
import de.fadi.nemotron.data.VideoEncoder
import de.fadi.nemotron.data.VideoInfo
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.getAndUpdate
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** A photo or video attached to the message being written. */
sealed interface Attachment {
    val key: Long
    val name: String

    /** What goes to the Mac; null while a video is still being shrunk. */
    val upload: Upload?
}

class PhotoAttachment(override val key: Long, val photo: Photo) : Attachment {
    override val name get() = photo.name
    override val upload = Upload(photo.name, "image/jpeg", UploadBody.of(photo.jpeg))
}

data class VideoAttachment(
    override val key: Long,
    override val name: String,
    val thumbnail: Bitmap?,
    val durationMs: Long,
    /** Percent done while it is being shrunk. */
    val progress: Int = 0,
    override val upload: Upload? = null,
    /** The shrunk copy in the cache; deleted together with the attachment. */
    val file: File? = null,
) : Attachment

/**
 * The photos and videos for the next message; owned by a ViewModel and used
 * on the main thread only. Videos are shrunk one at a time, since phones
 * have few hardware encoders.
 */
class Attachments(
    private val scope: CoroutineScope,
    private val photos: PhotoEncoder,
    private val videos: VideoEncoder,
    private val onError: (String) -> Unit,
) {
    private val _items = MutableStateFlow<List<Attachment>>(emptyList())
    val items: StateFlow<List<Attachment>> = _items.asStateFlow()

    private val shrinking = mutableMapOf<Long, Job>()
    private val encoder = Mutex()
    private var nextKey = 0L

    init {
        scope.launch { encoder.withLock { videos.deleteLeftovers() } }
    }

    fun add(uris: List<Uri>) {
        scope.launch {
            // One after the other, so they keep the order they were picked in.
            for (uri in uris) {
                if (_items.value.size >= MAX_ITEMS) {
                    onError("Höchstens $MAX_ITEMS Fotos oder Videos pro Nachricht.")
                    break
                }
                if (videos.isVideo(uri)) addVideo(uri) else addPhoto(uri)
            }
        }
    }

    private suspend fun addPhoto(uri: Uri) {
        val photo = try {
            photos.encode(uri)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            // Unreadable, unsupported or vanished: say so and keep the others.
            onError("Ein Foto ließ sich nicht laden.")
            return
        }
        append(PhotoAttachment(nextKey++, photo))
    }

    private suspend fun addVideo(uri: Uri) {
        val info = try {
            videos.probe(uri)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            onError("Ein Video ließ sich nicht laden.")
            return
        }
        val video = VideoAttachment(nextKey++, info.name, info.thumbnail, info.durationMs)
        if (!append(video)) return
        val job = scope.launch(start = CoroutineStart.LAZY) { shrink(video.key, uri, info) }
        shrinking[video.key] = job
        job.start()
    }

    /** Checked again here: another pick may have filled the slots while this one was being read. */
    private fun append(item: Attachment): Boolean {
        if (_items.value.size >= MAX_ITEMS) return false
        _items.update { it + item }
        return true
    }

    private suspend fun shrink(key: Long, uri: Uri, info: VideoInfo) {
        try {
            val file = encoder.withLock {
                videos.transcode(uri, info) { percent -> updateVideo(key) { it.copy(progress = percent) } }
            }
            if (file.length() > VideoEncoder.MAX_BYTES) {
                file.delete()
                drop(key, TOO_BIG)
            } else {
                ready(key, Upload(videos.shrunkName(info), "video/mp4", UploadBody.of(file)), file)
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            // Usually a codec this phone cannot decode; the Mac may still take the original.
            val original = videos.original(uri, info)
            when {
                original == null -> drop(key, "Dieses Video lässt sich nicht umwandeln. Bitte als MP4 aufnehmen.")
                info.size > VideoEncoder.MAX_BYTES -> drop(key, TOO_BIG)
                else -> ready(key, original, file = null)
            }
        } finally {
            shrinking.remove(key)
        }
    }

    private fun ready(key: Long, upload: Upload, file: File?) {
        if (_items.value.none { it.key == key }) {
            file?.delete()
            return
        }
        updateVideo(key) { it.copy(upload = upload, file = file) }
    }

    private fun updateVideo(key: Long, change: (VideoAttachment) -> VideoAttachment) =
        _items.update { items -> items.map { if (it.key == key && it is VideoAttachment) change(it) else it } }

    private fun drop(key: Long, reason: String) {
        _items.update { items -> items.filterNot { it.key == key } }
        onError(reason)
    }

    fun remove(key: Long) {
        shrinking.remove(key)?.cancel()
        _items.getAndUpdate { items -> items.filterNot { it.key == key } }.find { it.key == key }?.deleteFile()
    }

    /** After sending, and when the screen goes away for good: also deletes the shrunk copies. */
    fun clear() {
        // Copied first: a cancelled job may run its cleanup, which edits the map, right inside cancel().
        val jobs = shrinking.values.toList()
        shrinking.clear()
        jobs.forEach { it.cancel() }
        _items.getAndUpdate { emptyList() }.forEach { it.deleteFile() }
    }

    private fun Attachment.deleteFile() {
        (this as? VideoAttachment)?.file?.delete()
    }

    companion object {
        const val MAX_ITEMS = 6
        private const val TOO_BIG = "Video zu groß (über 100 MB) – bitte kürzer aufnehmen."
    }
}

/** Bytes sent of all attachments of a message, while they are being uploaded. */
data class UploadProgress(val sent: Long, val total: Long)
