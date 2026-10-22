package de.fadi.nemotron.ui.mac

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleStartEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import de.fadi.nemotron.R
import de.fadi.nemotron.data.ApprovalMode
import de.fadi.nemotron.data.Notice
import de.fadi.nemotron.data.RemoteState
import de.fadi.nemotron.ui.common.ChatList
import de.fadi.nemotron.ui.common.Composer
import de.fadi.nemotron.ui.common.UserBubble
import de.fadi.nemotron.ui.common.rememberChatListState

@Composable
fun MacScreen(viewModel: MacViewModel) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    LifecycleStartEffect(viewModel) {
        viewModel.setVisible(true)
        onStopOrDispose { viewModel.setVisible(false) }
    }

    val pairing = state.pairing
    if (pairing == null) {
        val context = LocalContext.current
        val scannerMissing = stringResource(R.string.scanner_failed)
        PairingScreen(
            inProgress = state.pairingInProgress,
            error = state.pairingError,
            onScan = {
                scanQrCode(
                    context,
                    onCode = viewModel::pairWithCode,
                    onError = { viewModel.scanFailed(scannerMissing) },
                )
            },
            onConnect = viewModel::pairManually,
        )
    } else {
        RemoteChat(viewModel, state, pairing.name)
    }
}

private fun scanQrCode(context: Context, onCode: (String) -> Unit, onError: () -> Unit) {
    val options = GmsBarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build()
    GmsBarcodeScanning.getClient(context, options)
        .startScan()
        .addOnSuccessListener { barcode -> barcode.rawValue?.let(onCode) }
        .addOnFailureListener { onError() }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RemoteChat(viewModel: MacViewModel, state: MacUiState, macName: String) {
    val remote = state.remote
    val busy = remote?.busy == true
    val photos by viewModel.attachments.photos.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    val chat = rememberChatListState()
    var showChanges by rememberSaveable { mutableStateOf(false) }

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
                        Text(
                            when {
                                remote == null -> macName
                                remote.workspace == null -> stringResource(R.string.chat_only)
                                else -> remote.workspace.name
                            },
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        if (remote != null) {
                            Text(
                                macName,
                                style = MaterialTheme.typography.labelMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                },
                actions = {
                    if (busy) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    MacMenu(
                        remote = remote,
                        onNewChat = viewModel::newChat,
                        onRetry = viewModel::retry,
                        onChanges = { showChanges = true },
                        onApproval = viewModel::setApproval,
                        onThinking = viewModel::setThinking,
                        onDisconnect = viewModel::disconnect,
                    )
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
            if (state.connectionLost) {
                Banner(stringResource(R.string.connection_lost), MaterialTheme.colorScheme.errorContainer)
            }
            remote?.notice?.let { NoticeBanner(it) }
            Box(Modifier.weight(1f)) {
                when {
                    remote == null -> Row(
                        Modifier.align(Alignment.Center),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                        Text(stringResource(R.string.connecting_to, macName))
                    }
                    remote.messages.isEmpty() -> Text(
                        stringResource(if (remote.workspace == null) R.string.empty_chat else R.string.empty_project),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.align(Alignment.Center).padding(32.dp),
                    )
                    else -> ChatList(chat, contentKey = remote.version, Modifier.fillMaxSize()) {
                        itemsIndexed(remote.messages, key = { index, _ -> index }) { _, message ->
                            if (message.isUser) {
                                UserBubble(message.text, attachments = message.attachments)
                            } else {
                                RemoteAssistantMessage(message, state.deciding, viewModel::decide)
                            }
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
                busy = busy,
                sending = state.sending,
                onSend = {
                    chat.jumpToEnd()
                    viewModel.send()
                },
                onStop = viewModel::stop,
            )
        }
    }

    if (showChanges && remote != null) {
        ChangesSheet(
            changes = remote.changes,
            busy = busy,
            onDiff = viewModel::showDiff,
            onUndo = viewModel::undo,
            onDismiss = { showChanges = false },
        )
    }
    state.diff?.let { DiffDialog(it.path, it.diff, viewModel::closeDiff) }
}

@Composable
private fun MacMenu(
    remote: RemoteState?,
    onNewChat: () -> Unit,
    onRetry: () -> Unit,
    onChanges: () -> Unit,
    onApproval: (ApprovalMode) -> Unit,
    onThinking: (Boolean) -> Unit,
    onDisconnect: () -> Unit,
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
        DropdownMenuItem(text = { Text(stringResource(R.string.new_chat)) }, onClick = item(onNewChat), enabled = connected)
        DropdownMenuItem(
            text = { Text(stringResource(R.string.retry)) },
            onClick = item(onRetry),
            enabled = connected && !busy && remote.messages.isNotEmpty(),
        )
        DropdownMenuItem(
            text = { Text(stringResource(R.string.changes_menu, remote?.changes?.size ?: 0)) },
            onClick = item(onChanges),
            enabled = remote?.workspace != null,
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
        DropdownMenuItem(text = { Text(stringResource(R.string.disconnect)) }, onClick = item(onDisconnect))
    }
}

private val ApprovalMode.label: Int
    get() = when (this) {
        ApprovalMode.AskAll -> R.string.approval_ask_all
        ApprovalMode.AutoEdits -> R.string.approval_auto_edits
        ApprovalMode.AutoAll -> R.string.approval_auto_all
    }

@Composable
private fun NoticeBanner(notice: Notice) {
    var dismissed by rememberSaveable { mutableStateOf<String?>(null) }
    if (notice.text.isBlank() || notice.text == dismissed) return
    Banner(
        notice.text,
        if (notice.error) MaterialTheme.colorScheme.errorContainer else MaterialTheme.colorScheme.secondaryContainer,
        onDismiss = { dismissed = notice.text },
    )
}

@Composable
private fun Banner(text: String, color: Color, onDismiss: (() -> Unit)? = null) {
    Surface(color = color, modifier = Modifier.fillMaxWidth()) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.padding(start = 16.dp, end = 4.dp, top = 8.dp, bottom = 8.dp),
        ) {
            Text(text, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f).padding(vertical = 4.dp))
            if (onDismiss != null) {
                IconButton(onClick = onDismiss) {
                    Icon(painterResource(R.drawable.ic_close), contentDescription = stringResource(R.string.close))
                }
            }
        }
    }
}
