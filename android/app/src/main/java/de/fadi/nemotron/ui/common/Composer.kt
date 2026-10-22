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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.key
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import de.fadi.nemotron.R

/**
 * The input row: text, photos and videos, and Send, which turns into Stop
 * while Nemotron is busy. [preparing] means picked media are still being
 * read. [sending] shows a message on its way, with the [upload] progress of
 * its attachments.
 */
@Composable
fun Composer(
    text: String,
    onTextChange: (String) -> Unit,
    attachments: List<Attachment>,
    preparing: Boolean,
    onAddMedia: (List<Uri>) -> Unit,
    onRemoveAttachment: (Attachment) -> Unit,
    busy: Boolean,
    sending: Boolean,
    upload: UploadProgress?,
    onSend: () -> Unit,
    onStop: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val picker = rememberLauncherForActivityResult(
        ActivityResultContracts.PickMultipleVisualMedia(Attachments.MAX_ITEMS),
    ) { uris -> if (uris.isNotEmpty()) onAddMedia(uris) }
    val shrinking = attachments.any { it.upload == null }
    val canSend = !busy && !sending && !preparing && !shrinking && (text.isNotBlank() || attachments.isNotEmpty())

    Surface(color = MaterialTheme.colorScheme.surfaceContainerLow, modifier = modifier.fillMaxWidth()) {
        Column(Modifier.padding(horizontal = 8.dp, vertical = 8.dp)) {
            if (attachments.isNotEmpty() || preparing) {
                Row(
                    Modifier.horizontalScroll(rememberScrollState()).padding(start = 8.dp, bottom = 8.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    for (attachment in attachments) {
                        key(attachment.key) {
                            AttachmentChip(attachment, removable = !sending, onRemove = { onRemoveAttachment(attachment) })
                        }
                    }
                    if (preparing) PendingChip()
                }
            }
            if (upload != null) UploadStatus(upload)
            Row(verticalAlignment = Alignment.Bottom) {
                IconButton(
                    onClick = {
                        picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageAndVideo))
                    },
                    enabled = !sending && attachments.size < Attachments.MAX_ITEMS,
                    modifier = Modifier.padding(bottom = 4.dp),
                ) {
                    Icon(painterResource(R.drawable.ic_image), contentDescription = stringResource(R.string.add_media))
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

@Composable
private fun UploadStatus(upload: UploadProgress) {
    Column(Modifier.padding(start = 8.dp, end = 8.dp, bottom = 8.dp)) {
        Text(
            stringResource(R.string.uploading, formatMegabytes(upload.sent, upload.total)),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        LinearProgressIndicator(
            progress = { if (upload.total > 0) upload.sent.toFloat() / upload.total else 0f },
            modifier = Modifier.fillMaxWidth().padding(top = 4.dp),
        )
    }
}
