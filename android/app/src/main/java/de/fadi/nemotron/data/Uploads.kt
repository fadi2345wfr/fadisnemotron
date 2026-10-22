package de.fadi.nemotron.data

import java.io.File
import okhttp3.MediaType
import okhttp3.RequestBody
import okio.Buffer
import okio.BufferedSink
import okio.Source
import okio.source

/** A photo or video for the Mac, with what [RemoteApi.upload] needs. */
class Upload(val name: String, val contentType: String, val body: UploadBody)

/**
 * The raw bytes of a photo or video for [RemoteApi.upload]. [open] starts
 * from the beginning each time, so OkHttp can resend after a dropped
 * connection. Videos are streamed from their file, never held in memory.
 */
class UploadBody(val length: Long, val open: () -> Source) {
    companion object {
        fun of(bytes: ByteArray) = UploadBody(bytes.size.toLong()) { Buffer().write(bytes) }

        fun of(file: File) = UploadBody(file.length()) { file.source() }
    }
}

/** Streams an [UploadBody] and reports the bytes written so far. */
internal class ProgressRequestBody(
    private val type: MediaType?,
    private val body: UploadBody,
    private val onProgress: (sent: Long) -> Unit,
) : RequestBody() {
    override fun contentType() = type

    override fun contentLength() = body.length

    override fun writeTo(sink: BufferedSink) {
        body.open().use { source ->
            var sent = 0L
            while (true) {
                val read = source.read(sink.buffer, CHUNK)
                if (read == -1L) break
                sink.emit()
                sent += read
                onProgress(sent)
            }
        }
    }

    private companion object {
        const val CHUNK = 64 * 1024L
    }
}
