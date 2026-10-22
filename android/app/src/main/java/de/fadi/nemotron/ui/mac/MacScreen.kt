package de.fadi.nemotron.ui.mac

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleStartEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import de.fadi.nemotron.R
import de.fadi.nemotron.data.RemoteApi
import de.fadi.nemotron.data.RemoteState
import de.fadi.nemotron.ui.common.ChatList
import de.fadi.nemotron.ui.common.Composer
import de.fadi.nemotron.ui.common.UserBubble
import de.fadi.nemotron.ui.common.rememberChatListState
import kotlinx.coroutines.launch

/** The whole app: pairing until a Mac is known, then its chats. */
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

@Composable
private fun RemoteChat(viewModel: MacViewModel, state: MacUiState, macName: String) {
    val remote = state.remote
    val drawer = rememberDrawerState(DrawerValue.Closed)
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }

    state.message?.let { message ->
        LaunchedEffect(message) {
            snackbar.showSnackbar(message)
            viewModel.messageShown()
        }
    }

    // Outside the drawer, so a message about the chat list stays readable while the list is open.
    Box(Modifier.fillMaxSize()) {
        ModalNavigationDrawer(
            drawerState = drawer,
            // Swiping only closes it: an edge swipe to open would fight with code blocks scrolling sideways.
            gesturesEnabled = drawer.isOpen,
            drawerContent = {
                ChatDrawer(
                    drawerState = drawer,
                    chats = remote?.chats.orEmpty(),
                    openChatId = remote?.chat?.id,
                    onNewChat = {
                        scope.launch { drawer.close() }
                        viewModel.newChat()
                    },
                    onOpen = { id ->
                        scope.launch { drawer.close() }
                        viewModel.openChat(id)
                    },
                    onDelete = viewModel::deleteChat,
                )
            },
        ) {
            ChatScaffold(viewModel, state, macName, onOpenDrawer = { scope.launch { drawer.open() } })
        }
        SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter).safeDrawingPadding())
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ChatScaffold(viewModel: MacViewModel, state: MacUiState, macName: String, onOpenDrawer: () -> Unit) {
    val remote = state.remote
    val busy = remote?.busy == true
    var showChanges by rememberSaveable { mutableStateOf(false) }

    Scaffold(
        topBar = {
            TopAppBar(
                navigationIcon = {
                    IconButton(onClick = onOpenDrawer, enabled = remote != null) {
                        Icon(painterResource(R.drawable.ic_menu), contentDescription = stringResource(R.string.chats))
                    }
                },
                title = { ChatTitle(remote, macName) },
                actions = {
                    if (busy) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    MacMenu(
                        remote = remote,
                        onRetry = viewModel::retry,
                        onChanges = { showChanges = true },
                        onApproval = viewModel::setApproval,
                        onThinking = viewModel::setThinking,
                        onUnpair = viewModel::unpair,
                    )
                },
            )
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding)
                .consumeWindowInsets(padding)
                .imePadding(),
        ) {
            if (state.macTooOld) {
                MacTooOld(onCheckAgain = viewModel::checkAgain)
            } else {
                Conversation(viewModel, state, macName, Modifier.weight(1f))
            }
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

/** The open chat's title, and below it where Nemotron works and on which Mac. */
@Composable
private fun ChatTitle(remote: RemoteState?, macName: String) {
    if (remote == null) {
        Text(macName, maxLines = 1, overflow = TextOverflow.Ellipsis)
        return
    }
    val workspace = remote.workspace?.let { if (it.home) stringResource(R.string.home_folder) else it.name }
    Column {
        Text(
            remote.chat?.title?.ifBlank { null } ?: stringResource(R.string.new_chat),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            listOfNotNull(workspace, macName).joinToString(" · "),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
private fun Conversation(viewModel: MacViewModel, state: MacUiState, macName: String, modifier: Modifier) {
    val remote = state.remote
    val attachments by viewModel.attachments.items.collectAsStateWithLifecycle()
    val preparing by viewModel.attachments.preparing.collectAsStateWithLifecycle()
    val chat = rememberChatListState()
    val chatId = remote?.chat?.id
    // The Mac may switch chats too; the phone follows and starts at the end of the new one.
    // Remembered across rotation, which must not lose the reading position in the same chat.
    var shownChatId by rememberSaveable { mutableStateOf(chatId) }
    LaunchedEffect(chatId) {
        if (chatId != shownChatId) {
            shownChatId = chatId
            chat.jumpToEnd()
        }
    }

    if (state.connectionLost) {
        Banner(stringResource(R.string.connection_lost), MaterialTheme.colorScheme.errorContainer)
    }
    remote?.notice?.let { NoticeBanner(it) }
    Box(modifier) {
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
                stringResource(if (remote.workspace?.home == false) R.string.empty_project else R.string.empty_home),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.align(Alignment.Center).padding(32.dp),
            )
            else -> ChatList(chat, contentKey = remote.version, Modifier.fillMaxSize()) {
                // Keyed by chat too, so folded thinking or tool details do not carry over to another chat.
                itemsIndexed(remote.messages, key = { index, _ -> "$chatId/$index" }) { _, message ->
                    if (message.isUser) {
                        UserBubble(message.text, message.attachments)
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
        attachments = attachments,
        preparing = preparing > 0,
        onAddMedia = viewModel.attachments::add,
        onRemoveAttachment = { viewModel.attachments.remove(it.key) },
        busy = remote?.busy == true,
        sending = state.sending,
        upload = state.upload,
        onSend = {
            chat.jumpToEnd()
            viewModel.send()
        },
        onStop = viewModel::stop,
    )
}

@Composable
private fun MacTooOld(onCheckAgain: () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(RemoteApi.MAC_TOO_OLD, style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center)
        Button(onClick = onCheckAgain) { Text(stringResource(R.string.check_again)) }
    }
}
