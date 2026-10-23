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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import de.fadi.nemotron.R
import de.fadi.nemotron.data.ApprovalMode

/**
 * The input bar: the mode switch (Prüfen / Dateien / Autonom), the history of
 * earlier messages, and the input row with text, photos and videos. While
 * Nemotron is [busy] one can go on typing: Send then interrupts it (it is
 * what to do when the server [stalled]), and Stop sits beside it. [preparing]
 * means picked media are still being read. [sending] shows a message on its
 * way, with the [upload] progress of its attachments.
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
    interruptedBy: String?,
    onContinue: () -> Unit,
    onRetry: () -> Unit,
    stalled: Boolean,
    sending: Boolean,
    upload: UploadProgress?,
    onSend: () -> Unit,
    onStop: () -> Unit,
    approval: ApprovalMode,
    onApproval: (ApprovalMode) -> Unit,
    history: List<String>,
    onPickHistory: (String) -> Unit,
    onHistoryOlder: () -> Boolean,
    onHistoryNewer: () -> Boolean,
    modifier: Modifier = Modifier,
) {
    val picker = rememberLauncherForActivityResult(
        ActivityResultContracts.PickMultipleVisualMedia(Attachments.MAX_ITEMS),
    ) { uris -> if (uris.isNotEmpty()) onAddMedia(uris) }
    val shrinking = attachments.any { it.upload == null }
    val canSend = !sending && !preparing && !shrinking && (text.isNotBlank() || attachments.isNotEmpty())

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
            if (interruptedBy != null && !busy) {
                InterruptedHint(interruptedBy, onContinue, onRetry)
            }
            if (busy && stalled) {
                Text(
                    stringResource(R.string.stalled_hint),
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
                )
            }
            ModeSwitch(approval, onApproval, history, onPickHistory)
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
                    // A keyboard's arrow keys call back earlier messages.
                    modifier = Modifier.weight(1f).onPreviewKeyEvent { event ->
                        when {
                            event.type != KeyEventType.KeyDown -> false
                            event.key == Key.DirectionUp -> onHistoryOlder()
                            event.key == Key.DirectionDown -> onHistoryNewer()
                            else -> false
                        }
                    },
                )
                Row(Modifier.padding(start = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    if (busy) {
                        FilledIconButton(
                            onClick = onStop,
                            colors = IconButtonDefaults.filledIconButtonColors(
                                containerColor = MaterialTheme.colorScheme.error,
                                contentColor = MaterialTheme.colorScheme.onError,
                            ),
                        ) {
                            Icon(painterResource(R.drawable.ic_stop), contentDescription = stringResource(R.string.stop))
                        }
                    }
                    if (sending) {
                        Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
                            CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.5.dp)
                        }
                    } else {
                        FilledIconButton(onClick = onSend, enabled = canSend) {
                            Icon(
                                painterResource(R.drawable.ic_send),
                                contentDescription = stringResource(if (busy) R.string.send_now else R.string.send),
                            )
                        }
                    }
                }
            }
        }
    }
}

/** An error ended the last answer: one tap goes on where it stopped. */
@Composable
private fun InterruptedHint(error: String, onContinue: () -> Unit, onRetry: () -> Unit) {
    Column(Modifier.padding(horizontal = 8.dp, vertical = 4.dp)) {
        Text(
            stringResource(R.string.interrupted_by, error),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.error,
            maxLines = 3,
            overflow = TextOverflow.Ellipsis,
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 4.dp)) {
            Button(onClick = onContinue) { Text(stringResource(R.string.continue_work)) }
            OutlinedButton(onClick = onRetry) { Text(stringResource(R.string.try_again)) }
        }
    }
}

/**
 * Prüfen (Request Review: every proposal is decided with a tick or a cross),
 * Dateien, Autonom; and the menu of earlier messages. The mode counts at once,
 * also for the answer that is running.
 */
@Composable
private fun ModeSwitch(
    approval: ApprovalMode,
    onApproval: (ApprovalMode) -> Unit,
    history: List<String>,
    onPickHistory: (String) -> Unit,
) {
    Column(Modifier.padding(horizontal = 4.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(
                Modifier.weight(1f).horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                for (mode in ApprovalMode.entries) {
                    FilterChip(
                        selected = mode == approval,
                        onClick = { onApproval(mode) },
                        label = { Text(stringResource(mode.shortLabel), maxLines = 1) },
                        leadingIcon = {
                            Icon(painterResource(mode.icon), contentDescription = null, modifier = Modifier.size(18.dp))
                        },
                    )
                }
            }
            HistoryButton(history, onPickHistory)
        }
        Text(
            stringResource(approval.hint),
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(start = 4.dp, bottom = 2.dp),
        )
    }
}

/** A menu of what was sent before, newest first. */
@Composable
private fun HistoryButton(history: List<String>, onPick: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }) {
            Icon(painterResource(R.drawable.ic_history), contentDescription = stringResource(R.string.history))
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            if (history.isEmpty()) {
                DropdownMenuItem(text = { Text(stringResource(R.string.history_empty)) }, onClick = { open = false }, enabled = false)
            }
            for (prompt in history.take(HISTORY_MENU_ITEMS)) {
                DropdownMenuItem(
                    text = { Text(prompt, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.width(260.dp)) },
                    onClick = {
                        open = false
                        onPick(prompt)
                    },
                )
            }
        }
    }
}

private const val HISTORY_MENU_ITEMS = 15

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
