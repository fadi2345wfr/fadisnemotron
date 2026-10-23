package de.fadi.nemotron.ui.common

import android.content.ClipData
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import de.fadi.nemotron.R
import de.fadi.nemotron.data.MessageStatus
import de.fadi.nemotron.ui.theme.FadisTheme
import kotlinx.coroutines.launch

/** The user's message: a green-tinted bubble on the right. */
@Composable
fun UserBubble(text: String, attachments: List<String>) {
    Box(Modifier.fillMaxWidth().padding(start = 40.dp), contentAlignment = Alignment.CenterEnd) {
        Surface(
            color = MaterialTheme.colorScheme.primary.copy(alpha = 0.16f),
            shape = RoundedCornerShape(18.dp, 18.dp, 4.dp, 18.dp),
        ) {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                for (name in attachments) {
                    Text(
                        stringResource(R.string.attachment, name),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (text.isNotBlank()) {
                    SelectionContainer { Text(text, style = MaterialTheme.typography.bodyLarge) }
                }
            }
        }
    }
}

/**
 * Nemotron's answer as a card. While it streams, [activity] says what it is
 * doing next to a spinner.
 */
@Composable
fun AssistantCard(activity: String?, content: @Composable ColumnScope.() -> Unit) {
    Surface(
        color = MaterialTheme.colorScheme.surfaceContainer,
        shape = RoundedCornerShape(16.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    stringResource(R.string.nemotron),
                    style = MaterialTheme.typography.labelLarge,
                    fontWeight = FontWeight.Bold,
                    color = MaterialTheme.colorScheme.primary,
                )
                if (activity != null) {
                    CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
                    Text(
                        activity,
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            content()
        }
    }
}

/** The model's thinking, folded away by default once there is an answer to read. */
@Composable
fun ThinkingSection(thinking: String, expandedByDefault: Boolean) {
    // null until the reader taps; until then the default follows the stream.
    var choice by rememberSaveable { mutableStateOf<Boolean?>(null) }
    val expanded = choice ?: expandedByDefault
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .clip(RoundedCornerShape(8.dp))
                .clickable { choice = !expanded }
                .padding(vertical = 4.dp),
        ) {
            val characters = thinking.codePointCount(0, thinking.length)
            Text(
                pluralStringResource(R.plurals.thinking_header, characters, characters),
                style = MaterialTheme.typography.labelLarge,
                color = muted,
            )
            Icon(
                painterResource(if (expanded) R.drawable.ic_expand_less else R.drawable.ic_expand_more),
                contentDescription = null,
                tint = muted,
                modifier = Modifier.size(20.dp),
            )
        }
        if (expanded) {
            SelectionContainer {
                Text(thinking, style = MaterialTheme.typography.bodyMedium, color = muted)
            }
        }
    }
}

/**
 * What to say below an answer that ended: the error, "Gestoppt." or a hint
 * when the answer was cut off or stayed empty.
 */
@Composable
fun MessageOutcome(status: MessageStatus, error: String?, finishReason: String?, empty: Boolean) {
    val text = when (status) {
        MessageStatus.Streaming -> null
        MessageStatus.Failed -> error ?: stringResource(R.string.failed)
        MessageStatus.Cancelled -> stringResource(R.string.stopped)
        MessageStatus.Done -> when {
            finishReason == "length" -> stringResource(R.string.cut_off)
            empty -> stringResource(R.string.empty_answer)
            else -> null
        }
    } ?: return
    val color = when (status) {
        MessageStatus.Failed -> MaterialTheme.colorScheme.error
        MessageStatus.Cancelled -> MaterialTheme.colorScheme.onSurfaceVariant
        else -> FadisTheme.status.warning
    }
    Text(text, color = color, style = MaterialTheme.typography.bodyMedium)
}

@Composable
fun NoticeText(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        fontStyle = FontStyle.Italic,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
fun CopyButton(text: String) {
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    val label = stringResource(R.string.answer)
    TextButton(onClick = {
        scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText(label, text))) }
    }) {
        Icon(painterResource(R.drawable.ic_copy), contentDescription = null, modifier = Modifier.size(18.dp))
        Text(stringResource(R.string.copy), Modifier.padding(start = 8.dp))
    }
}
