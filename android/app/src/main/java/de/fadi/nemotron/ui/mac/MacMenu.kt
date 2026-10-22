package de.fadi.nemotron.ui.mac

import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import de.fadi.nemotron.R
import de.fadi.nemotron.data.ApprovalMode
import de.fadi.nemotron.data.RemoteState

/** The overflow menu: retry, changes, what Nemotron may do, thinking, unpairing. */
@Composable
fun MacMenu(
    remote: RemoteState?,
    onRetry: () -> Unit,
    onChanges: () -> Unit,
    onApproval: (ApprovalMode) -> Unit,
    onThinking: (Boolean) -> Unit,
    onUnpair: () -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    val connected = remote != null
    val busy = remote?.busy == true
    IconButton(onClick = { open = true }) {
        Icon(painterResource(R.drawable.ic_more_vert), contentDescription = stringResource(R.string.menu))
    }
    DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
        fun item(action: () -> Unit): () -> Unit = {
            open = false
            action()
        }
        DropdownMenuItem(
            text = { Text(stringResource(R.string.retry)) },
            onClick = item(onRetry),
            enabled = connected && !busy && remote.messages.isNotEmpty(),
        )
        DropdownMenuItem(
            text = { Text(stringResource(R.string.changes_menu, remote?.changes?.size ?: 0)) },
            onClick = item(onChanges),
            enabled = connected,
        )
        HorizontalDivider()
        Text(
            stringResource(R.string.approval_title),
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
        )
        for (mode in ApprovalMode.entries) {
            DropdownMenuItem(
                text = { Text(stringResource(mode.label)) },
                onClick = item { onApproval(mode) },
                trailingIcon = { RadioButton(selected = remote?.approval == mode, onClick = null) },
                enabled = connected,
            )
        }
        HorizontalDivider()
        DropdownMenuItem(
            text = { Text(stringResource(R.string.thinking_mode)) },
            onClick = item { onThinking(remote?.thinking != true) },
            trailingIcon = { Checkbox(checked = remote?.thinking == true, onCheckedChange = null) },
            enabled = connected,
        )
        HorizontalDivider()
        DropdownMenuItem(text = { Text(stringResource(R.string.unpair)) }, onClick = item(onUnpair))
    }
}

private val ApprovalMode.label: Int
    get() = when (this) {
        ApprovalMode.AskAll -> R.string.approval_ask_all
        ApprovalMode.AutoEdits -> R.string.approval_auto_edits
        ApprovalMode.AutoAll -> R.string.approval_auto_all
    }
