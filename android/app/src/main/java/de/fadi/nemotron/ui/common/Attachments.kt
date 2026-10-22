package de.fadi.nemotron.ui.common

import android.net.Uri
import de.fadi.nemotron.data.Photo
import de.fadi.nemotron.data.PhotoEncoder
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** The photos attached to the message being written; owned by a ViewModel. */
class Attachments(
    private val scope: CoroutineScope,
    private val encoder: PhotoEncoder,
    private val onError: (String) -> Unit,
) {
    private val _photos = MutableStateFlow<List<Photo>>(emptyList())
    val photos: StateFlow<List<Photo>> = _photos.asStateFlow()

    fun add(uris: List<Uri>) {
        scope.launch {
            // One after the other, so they keep the order they were picked in.
            for (uri in uris) {
                if (_photos.value.size >= MAX_PHOTOS) break
                try {
                    val photo = encoder.encode(uri)
                    _photos.update { it + photo }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    // Unreadable, unsupported or vanished: say so and keep the other photos.
                    onError("Ein Foto ließ sich nicht laden.")
                }
            }
        }
    }

    fun remove(photo: Photo) = _photos.update { it - photo }

    fun clear() {
        _photos.value = emptyList()
    }

    companion object {
        const val MAX_PHOTOS = 8
    }
}
