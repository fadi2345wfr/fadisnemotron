package de.fadi.nemotron

import android.app.Application
import de.fadi.nemotron.data.PhotoEncoder
import de.fadi.nemotron.data.Prefs
import de.fadi.nemotron.data.VideoEncoder
import okhttp3.OkHttpClient

/** Holds what lives as long as the app: settings, one HTTP client and the photo and video encoders. */
class FadisApplication : Application() {
    val prefs by lazy { Prefs(this) }
    val http by lazy { OkHttpClient() }
    val photos by lazy { PhotoEncoder(contentResolver) }
    val videos by lazy { VideoEncoder(this) }
}
