package de.fadi.nemotron

import android.app.Application
import de.fadi.nemotron.data.PhotoEncoder
import de.fadi.nemotron.data.Prefs
import okhttp3.OkHttpClient

/** Holds what both modes share: settings, one HTTP client and the photo encoder. */
class FadisApplication : Application() {
    val prefs by lazy { Prefs(this) }
    val http by lazy { OkHttpClient() }
    val photos by lazy { PhotoEncoder(contentResolver) }
}
