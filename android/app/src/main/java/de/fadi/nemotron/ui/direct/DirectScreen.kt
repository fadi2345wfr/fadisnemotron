package de.fadi.nemotron.ui.direct

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import de.fadi.nemotron.R
import de.fadi.nemotron.data.MessageStatus
import de.fadi.nemotron.ui.common.AssistantCard
import de.fadi.nemotron.ui.common.ChatList
import de.fadi.nemotron.ui.common.Composer
import de.fadi.nemotron.ui.common.CopyButton
import de.fadi.nemotron.ui.common.MarkdownText
import de.fadi.nemotron.ui.common.MessageOutcome
import de.fadi.nemotron.ui.common.ThinkingSection
import de.fadi.nemotron.ui.common.UserBubble
import de.fadi.nemotron.ui.common.rememberChatListState

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DirectScreen(viewModel: DirectViewModel) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    val photos by viewModel.attachments.photos.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    val chat = rememberChatListState()
    val needsKey = state.settings.apiKey.isBlank()
    var showSettings by rememberSaveable { mutableStateOf(false) }

    state.message?.let { message ->
        LaunchedEffect(message) {
            snackbar.showSnackbar(message)
            viewModel.messageShown()
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(stringResource(R.string.direct_title))
                        Text(
                            state.settings.model.substringAfterLast('/'),
                            style = MaterialTheme.typography.labelMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                },
                actions = {
                    IconButton(onClick = viewModel::newChat, enabled = state.turns.isNotEmpty()) {
                        Icon(painterResource(R.drawable.ic_add), contentDescription = stringResource(R.string.new_chat))
                    }
                    IconButton(onClick = { showSettings = true }) {
                        Icon(painterResource(R.drawable.ic_settings), contentDescription = stringResource(R.string.settings))
                    }
                },
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding)
                .consumeWindowInsets(padding)
                .imePadding(),
        ) {
            Box(Modifier.weight(1f)) {
                if (state.turns.isEmpty()) {
                    Column(
                        Modifier.align(Alignment.Center).padding(32.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(16.dp),
                    ) {
                        Text(
                            stringResource(if (needsKey) R.string.direct_needs_key else R.string.direct_empty),
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            textAlign = TextAlign.Center,
                        )
                        if (needsKey) {
                            Button(onClick = { showSettings = true }) { Text(stringResource(R.string.enter_key)) }
                        }
                    }
                } else {
                    val last = state.turns.last()
                    ChatList(chat, contentKey = last.id to last.reply.thinking.length + last.reply.answer.length) {
                        for (turn in state.turns) {
                            item(key = "frage-${turn.id}") { UserBubble(turn.text, photos = turn.photos) }
                            item(key = "antwort-${turn.id}") { DirectReply(turn) }
                        }
                    }
                }
            }
            Composer(
                text = viewModel.draft,
                onTextChange = { viewModel.draft = it },
                photos = photos,
                onAddPhotos = viewModel.attachments::add,
                onRemovePhoto = viewModel.attachments::remove,
                busy = state.busy,
                sending = false,
                onSend = {
                    if (needsKey) {
                        showSettings = true
                    } else {
                        chat.jumpToEnd()
                        viewModel.send()
                    }
                },
                onStop = viewModel::stop,
            )
        }
    }

    if (showSettings) {
        DirectSettingsDialog(
            settings = state.settings,
            onSave = {
                viewModel.saveSettings(it)
                showSettings = false
            },
            onDismiss = { showSettings = false },
        )
    }
}

@Composable
private fun DirectReply(turn: Turn) {
    val streaming = turn.status == MessageStatus.Streaming
    val reply = turn.reply
    val activity = when {
        !streaming -> null
        reply.answer.isBlank() -> stringResource(R.string.activity_thinking)
        else -> stringResource(R.string.activity_writing)
    }
    AssistantCard(activity) {
        if (reply.thinking.isNotEmpty()) {
            ThinkingSection(reply.thinking, expandedByDefault = reply.answer.isBlank())
        }
        if (reply.answer.isNotBlank()) MarkdownText(reply.answer.trim())
        MessageOutcome(turn.status, turn.error, reply.finishReason, empty = reply.answer.isBlank())
        if (!streaming && reply.answer.isNotBlank()) CopyButton(reply.answer.trim())
    }
}
