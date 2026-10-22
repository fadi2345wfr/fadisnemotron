package de.fadi.nemotron.ui.mac

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import de.fadi.nemotron.R
import de.fadi.nemotron.data.Change
import de.fadi.nemotron.ui.common.DiffBox
import de.fadi.nemotron.ui.theme.FadisTheme

/** The files Nemotron changed on the Mac, each with its diff and an undo. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChangesSheet(
    changes: List<Change>,
    busy: Boolean,
    onDiff: (String) -> Unit,
    onUndo: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var confirmUndo by rememberSaveable { mutableStateOf<String?>(null) }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Text(
            stringResource(R.string.changes_title),
            style = MaterialTheme.typography.titleLarge,
            modifier = Modifier.padding(horizontal = 20.dp),
        )
        if (changes.isEmpty()) {
            Text(
                stringResource(R.string.changes_empty),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(20.dp),
            )
        }
        LazyColumn(Modifier.padding(bottom = 16.dp)) {
            items(changes, key = { it.path }) { change ->
                Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(
                            change.path,
                            style = MaterialTheme.typography.bodyLarge,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f),
                        )
                        if (change.created) {
                            Text(stringResource(R.string.change_new), color = MaterialTheme.colorScheme.primary)
                        }
                        Text("+${change.added}", color = FadisTheme.status.added)
                        Text("−${change.removed}", color = FadisTheme.status.removed)
                    }
                    Row {
                        TextButton(onClick = { onDiff(change.path) }) { Text(stringResource(R.string.diff)) }
                        TextButton(onClick = { confirmUndo = change.path }, enabled = !busy) {
                            Text(stringResource(R.string.undo))
                        }
                    }
                }
                HorizontalDivider()
            }
        }
    }

    confirmUndo?.let { path ->
        val created = changes.find { it.path == path }?.created == true
        AlertDialog(
            onDismissRequest = { confirmUndo = null },
            title = { Text(stringResource(R.string.undo_title)) },
            text = { Text(stringResource(if (created) R.string.undo_text_created else R.string.undo_text, path)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmUndo = null
                    onUndo(path)
                }) { Text(stringResource(R.string.undo)) }
            },
            dismissButton = { TextButton(onClick = { confirmUndo = null }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}

/** A whole diff on its own screen; it scrolls both ways. */
@Composable
fun DiffDialog(path: String, diff: String, onDismiss: () -> Unit) {
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize()) {
            Column(Modifier.safeDrawingPadding()) {
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(start = 16.dp, end = 4.dp)) {
                    Text(
                        path,
                        style = MaterialTheme.typography.titleMedium,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    IconButton(onClick = onDismiss) {
                        Icon(painterResource(R.drawable.ic_close), contentDescription = stringResource(R.string.close))
                    }
                }
                HorizontalDivider()
                Box(Modifier.verticalScroll(rememberScrollState()).padding(12.dp)) {
                    if (diff.isBlank()) {
                        Text(stringResource(R.string.diff_empty), color = MaterialTheme.colorScheme.onSurfaceVariant)
                    } else {
                        DiffBox(diff)
                    }
                }
            }
        }
    }
}
