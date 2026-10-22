package de.fadi.nemotron.ui.common

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import de.fadi.nemotron.R
import de.fadi.nemotron.data.Photo

/**
 * The input row: text, photos, and Send, which turns into Stop while
 * Nemotron is busy. [sending] shows a request on its way.
 */
@Composable
fun Composer(
    text: String,
    onTextChange: (String) -> Unit,
    photos: List<Photo>,
    onAddPhotos: (List<Uri>) -> Unit,
    onRemovePhoto: (Photo) -> Unit,
    busy: Boolean,
    sending: Boolean,
    onSend: () -> Unit,
    onStop: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val picker = rememberLauncherForActivityResult(
        ActivityResultContracts.PickMultipleVisualMedia(Attachments.MAX_PHOTOS),
    ) { uris -> if (uris.isNotEmpty()) onAddPhotos(uris) }
    val canSend = !busy && !sending && (text.isNotBlank() || photos.isNotEmpty())

    Surface(color = MaterialTheme.colorScheme.surfaceContainerLow, modifier = modifier.fillMaxWidth()) {
        Column(Modifier.padding(horizontal = 8.dp, vertical = 8.dp)) {
            if (photos.isNotEmpty()) {
                Row(
                    Modifier.horizontalScroll(rememberScrollState()).padding(start = 8.dp, bottom = 8.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    for (photo in photos) {
                        Box {
                            Thumbnail(photo, 64)
                            Surface(
                                onClick = { onRemovePhoto(photo) },
                                shape = CircleShape,
                                color = MaterialTheme.colorScheme.inverseSurface,
                                modifier = Modifier.align(Alignment.TopEnd).padding(2.dp).size(22.dp),
                            ) {
                                Icon(
                                    painterResource(R.drawable.ic_close),
                                    contentDescription = stringResource(R.string.remove_photo, photo.name),
                                    tint = MaterialTheme.colorScheme.inverseOnSurface,
                                    modifier = Modifier.padding(3.dp),
                                )
                            }
                        }
                    }
                }
            }
            Row(verticalAlignment = Alignment.Bottom) {
                IconButton(
                    onClick = {
                        picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                    },
                    enabled = photos.size < Attachments.MAX_PHOTOS,
                    modifier = Modifier.padding(bottom = 4.dp),
                ) {
                    Icon(painterResource(R.drawable.ic_image), contentDescription = stringResource(R.string.add_photos))
                }
                OutlinedTextField(
                    value = text,
                    onValueChange = onTextChange,
                    placeholder = { Text(stringResource(R.string.composer_hint)) },
                    minLines = 1,
                    maxLines = 6,
                    shape = RoundedCornerShape(24.dp),
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                    modifier = Modifier.weight(1f),
                )
                Box(Modifier.padding(start = 4.dp, bottom = 4.dp)) {
                    when {
                        busy -> FilledIconButton(
                            onClick = onStop,
                            colors = IconButtonDefaults.filledIconButtonColors(
                                containerColor = MaterialTheme.colorScheme.error,
                                contentColor = MaterialTheme.colorScheme.onError,
                            ),
                        ) {
                            Icon(painterResource(R.drawable.ic_stop), contentDescription = stringResource(R.string.stop))
                        }
                        sending -> Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
                            CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.5.dp)
                        }
                        else -> FilledIconButton(onClick = onSend, enabled = canSend) {
                            Icon(painterResource(R.drawable.ic_send), contentDescription = stringResource(R.string.send))
                        }
                    }
                }
            }
        }
    }
}
