package de.fadi.nemotron.ui.mac

import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.key
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import de.fadi.nemotron.R
import de.fadi.nemotron.data.Decision
import de.fadi.nemotron.data.MessageStatus
import de.fadi.nemotron.data.RemoteMessage
import de.fadi.nemotron.data.Step
import de.fadi.nemotron.data.ToolState
import de.fadi.nemotron.data.thinkingAndAnswer
import de.fadi.nemotron.ui.common.AssistantCard
import de.fadi.nemotron.ui.common.CopyButton
import de.fadi.nemotron.ui.common.MarkdownText
import de.fadi.nemotron.ui.common.MessageOutcome
import de.fadi.nemotron.ui.common.NoticeText
import de.fadi.nemotron.ui.common.ThinkingSection

/** An answer from the agent on the Mac: thinking, text and tool steps. */
@Composable
fun RemoteAssistantMessage(
    message: RemoteMessage,
    deciding: Set<String>,
    onDecide: (callId: String, Decision) -> Unit,
    onOpenChat: (String) -> Unit,
) {
    val streaming = message.status == MessageStatus.Streaming
    val parts = message.steps.map { thinkingAndAnswer(it.reasoning, it.content) }
    val answer = parts.lastOrNull { it.second.isNotBlank() }?.second?.trim().orEmpty()
    val hasTools = message.steps.any { it.tools.isNotEmpty() }

    AssistantCard(activity = if (streaming) activity(message.steps.lastOrNull(), parts.lastOrNull()?.second) else null) {
        message.steps.forEachIndexed { index, step ->
            key(index) {
                val (thinking, text) = parts[index]
                if (thinking.isNotEmpty()) {
                    ThinkingSection(thinking, expandedByDefault = text.isBlank() && step.tools.isEmpty())
                }
                if (text.isNotBlank()) MarkdownText(text)
                for (tool in step.tools) {
                    key(tool.id) {
                        ToolCard(tool, deciding = tool.id in deciding, onDecide = { onDecide(tool.id, it) })
                    }
                }
            }
        }
        for (notice in message.notices) NoticeText(notice)
        // The chat got too long: Nemotron went on in a new one, one tap away.
        message.continuedIn?.let { next ->
            Button(onClick = { onOpenChat(next) }) {
                Icon(painterResource(R.drawable.ic_arrow_forward), contentDescription = null, modifier = Modifier.size(18.dp))
                Text(stringResource(R.string.continue_in_new_chat), modifier = Modifier.padding(start = 8.dp))
            }
        }
        MessageOutcome(message.status, message.error, message.finishReason, empty = answer.isEmpty() && !hasTools)
        if (!streaming && answer.isNotEmpty()) CopyButton(answer)
    }
}

/** What the agent is doing right now, judged by its latest step. */
@Composable
private fun activity(step: Step?, text: String?): String {
    val tools = step?.tools.orEmpty()
    return stringResource(
        when {
            tools.any { it.state == ToolState.AwaitingApproval } -> R.string.activity_waiting
            tools.any { it.state == ToolState.Running || it.state == ToolState.Planned } -> R.string.activity_working
            !text.isNullOrBlank() -> R.string.activity_writing
            else -> R.string.activity_thinking
        },
    )
}
