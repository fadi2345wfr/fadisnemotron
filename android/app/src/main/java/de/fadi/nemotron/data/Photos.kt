package de.fadi.nemotron.data

import android.content.ContentResolver
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Matrix
import android.net.Uri
import android.provider.OpenableColumns
import androidx.core.graphics.createBitmap
import androidx.core.graphics.scale
import androidx.exifinterface.media.ExifInterface
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.util.Base64
import kotlin.math.max
import kotlin.math.min
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** A photo ready to send: JPEG, base64, with a small preview. */
class Photo(val name: String, val base64: String, val thumbnail: Bitmap)

/** Turns picked images into [Photo]s: upright, at most [MAX_SIDE] px, JPEG quality [QUALITY]. */
class PhotoEncoder(private val resolver: ContentResolver) {

    suspend fun encode(uri: Uri): Photo = withContext(Dispatchers.IO) {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        open(uri).use { BitmapFactory.decodeStream(it, null, bounds) }
        val longest = max(bounds.outWidth, bounds.outHeight)
        if (longest <= 0) throw IOException("Kein lesbares Bild: $uri")

        // Decode at a power-of-two reduction first so large camera photos do not fill the memory.
        var sample = 1
        while (longest / (sample * 2) >= MAX_SIDE) sample *= 2
        val options = BitmapFactory.Options().apply { inSampleSize = sample }
        val decoded = open(uri).use { BitmapFactory.decodeStream(it, null, options) }
            ?: throw IOException("Kein lesbares Bild: $uri")
        val rotation = open(uri).use(::exifRotation)

        val image = upright(decoded, rotation)
        val jpeg = ByteArrayOutputStream().use {
            image.compress(Bitmap.CompressFormat.JPEG, QUALITY, it)
            it.toByteArray()
        }
        val scale = min(1f, THUMBNAIL_SIDE.toFloat() / max(image.width, image.height))
        val thumbnail = image.scale(max(1, (image.width * scale).toInt()), max(1, (image.height * scale).toInt()))
        Photo(displayName(uri), Base64.getEncoder().encodeToString(jpeg), thumbnail)
    }

    private fun open(uri: Uri): InputStream =
        resolver.openInputStream(uri) ?: throw IOException("Bild lässt sich nicht öffnen: $uri")

    /** Scales to [MAX_SIDE], turns by the EXIF rotation and flattens transparency onto white. */
    private fun upright(source: Bitmap, rotation: Float): Bitmap {
        val scale = min(1f, MAX_SIDE.toFloat() / max(source.width, source.height))
        val matrix = Matrix().apply {
            postScale(scale, scale)
            postRotate(rotation)
        }
        val turned = Bitmap.createBitmap(source, 0, 0, source.width, source.height, matrix, true)
        if (!turned.hasAlpha()) return turned
        val flat = createBitmap(turned.width, turned.height)
        Canvas(flat).apply {
            drawColor(Color.WHITE)
            drawBitmap(turned, 0f, 0f, null)
        }
        return flat
    }

    /** Phone cameras store portrait photos sideways and note the turn in EXIF. */
    private fun exifRotation(stream: InputStream): Float = try {
        ExifInterface(stream).rotationDegrees.toFloat()
    } catch (e: IOException) {
        0f
    }

    private fun displayName(uri: Uri): String {
        val name = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) cursor.getString(0) else null
        }
        return "${name?.substringBeforeLast('.')?.takeIf { it.isNotBlank() } ?: "foto"}.jpg"
    }

    companion object {
        const val MAX_SIDE = 1536
        const val QUALITY = 85
        private const val THUMBNAIL_SIDE = 192
    }
}
