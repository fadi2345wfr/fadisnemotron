package de.fadi.nemotron.ui.mac

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import de.fadi.nemotron.R
import de.fadi.nemotron.data.Decision
import de.fadi.nemotron.data.Tool
import de.fadi.nemotron.data.ToolState
import de.fadi.nemotron.ui.common.CodeBox
import de.fadi.nemotron.ui.common.DiffBox
import de.fadi.nemotron.ui.theme.FadisTheme

/** One step of the agent: reading, searching, editing or running something on the Mac. */
@Composable
fun ToolCard(tool: Tool, deciding: Boolean, onDecide: (Decision) -> Unit) {
    val awaiting = tool.state == ToolState.AwaitingApproval
    val hasDetails = !awaiting && (tool.diff.isNotBlank() || tool.output.isNotBlank())
    var expanded by rememberSaveable(tool.id) { mutableStateOf(false) }
    val colors = MaterialTheme.colorScheme

    Surface(
        shape = RoundedCornerShape(10.dp),
        color = colors.surfaceContainerLow,
        border = BorderStroke(if (awaiting) 1.5.dp else 1.dp, if (awaiting) colors.primary else colors.outlineVariant),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                modifier = if (hasDetails) Modifier.clickable { expanded = !expanded } else Modifier,
            ) {
                StateIcon(tool)
                Text(toolEmoji(tool.name))
                Text(
                    tool.title.ifBlank { tool.name },
                    fontWeight = if (tool.name in CHANGING_TOOLS) FontWeight.SemiBold else FontWeight.Normal,
                    style = MaterialTheme.typography.bodyMedium,
                    // A command waiting for approval must be readable in full.
                    maxLines = if (awaiting) Int.MAX_VALUE else 2,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                if (tool.added > 0 || tool.removed > 0) {
                    Text("+${tool.added}", color = FadisTheme.status.added, style = MaterialTheme.typography.labelMedium)
                    Text("−${tool.removed}", color = FadisTheme.status.removed, style = MaterialTheme.typography.labelMedium)
                }
                if (hasDetails) {
                    Icon(
                        painterResource(if (expanded) R.drawable.ic_expand_less else R.drawable.ic_expand_more),
                        contentDescription = stringResource(R.string.details),
                        tint = colors.onSurfaceVariant,
                        modifier = Modifier.size(20.dp),
                    )
                }
            }
            when {
                awaiting -> Approval(tool, deciding, onDecide)
                expanded -> if (tool.diff.isNotBlank()) DiffBox(tool.diff) else CodeBox(tool.output.trimEnd())
            }
        }
    }
}

@Composable
private fun StateIcon(tool: Tool) {
    val colors = MaterialTheme.colorScheme
    val status = FadisTheme.status
    if (tool.state == ToolState.Running) {
        CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
        return
    }
    val (icon, tint, label) = when (tool.state) {
        ToolState.Done -> if (tool.ok) {
            Triple(R.drawable.ic_check, status.added, R.string.tool_ok)
        } else {
            Triple(R.drawable.ic_warning, status.warning, R.string.tool_failed)
        }
        ToolState.AwaitingApproval -> Triple(R.drawable.ic_pause, colors.primary, R.string.tool_waiting)
        ToolState.Declined -> Triple(R.drawable.ic_close, colors.onSurfaceVariant, R.string.tool_declined)
        ToolState.NotRun -> Triple(R.drawable.ic_remove, colors.onSurfaceVariant, R.string.tool_not_run)
        ToolState.Planned, ToolState.Running -> Triple(R.drawable.ic_circle_outline, colors.onSurfaceVariant, R.string.tool_planned)
    }
    Icon(painterResource(icon), contentDescription = stringResource(label), tint = tint, modifier = Modifier.size(18.dp))
}

@Composable
private fun Approval(tool: Tool, deciding: Boolean, onDecide: (Decision) -> Unit) {
    Text(stringResource(R.string.approve_question), fontWeight = FontWeight.SemiBold)
    tool.warning?.takeIf { it.isNotBlank() }?.let {
        Text(stringResource(R.string.approve_warning, it), color = FadisTheme.status.warning)
    }
    when {
        tool.diff.isNotBlank() -> DiffBox(tool.diff)
        tool.output.isNotBlank() -> CodeBox(tool.output.trimEnd())
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Button(onClick = { onDecide(Decision.Allow) }, enabled = !deciding, modifier = Modifier.weight(1f)) {
            Text(stringResource(R.string.allow))
        }
        OutlinedButton(
            onClick = { onDecide(Decision.Deny) },
            enabled = !deciding,
            modifier = Modifier.weight(1f),
        ) {
            Text(stringResource(R.string.deny))
        }
    }
    OutlinedButton(
        onClick = { onDecide(Decision.AllowForRun) },
        enabled = !deciding,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Text(stringResource(R.string.allow_for_run))
    }
}

private val CHANGING_TOOLS = setOf("edit_file", "write_file", "run_command")

private fun toolEmoji(name: String): String = when (name) {
    "list_dir" -> "📂"
    "find_files" -> "🔎"
    "read_file" -> "📄"
    "search" -> "🔍"
    "edit_file" -> "✏️"
    "write_file" -> "📝"
    "run_command" -> "▶️"
    else -> "🔧"
}

