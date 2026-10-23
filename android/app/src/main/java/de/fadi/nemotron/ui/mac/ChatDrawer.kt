package de.fadi.nemotron.ui.mac

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DrawerState
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.NavigationDrawerItem
import androidx.compose.material3.NavigationDrawerItemDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import de.fadi.nemotron.R
import de.fadi.nemotron.data.ChatSummary
import de.fadi.nemotron.ui.common.relativeTime
import java.time.ZonedDateTime
import kotlinx.coroutines.delay

/** The chats saved on the Mac, newest first, with the open one highlighted. Back closes it. */
@Composable
fun ChatDrawer(
    drawerState: DrawerState,
    chats: List<ChatSummary>,
    openChatId: String?,
    onNewChat: () -> Unit,
    onOpen: (String) -> Unit,
    onDelete: (String) -> Unit,
) {
    var confirmDelete by rememberSaveable { mutableStateOf<String?>(null) }
    // Keeps "vor 5 Min." current while the drawer stays open.
    val now by produceState(ZonedDateTime.now()) {
        while (true) {
            delay(30_000)
            value = ZonedDateTime.now()
        }
    }

    ModalDrawerSheet(drawerState) {
        Text(
            stringResource(R.string.chats),
            style = MaterialTheme.typography.titleLarge,
            modifier = Modifier.padding(start = 28.dp, top = 20.dp, bottom = 12.dp),
        )
        NavigationDrawerItem(
            icon = { Icon(painterResource(R.drawable.ic_add), contentDescription = null) },
            label = { Text(stringResource(R.string.new_chat)) },
            selected = false,
            onClick = onNewChat,
            modifier = Modifier.padding(NavigationDrawerItemDefaults.ItemPadding),
        )
        HorizontalDivider(Modifier.padding(horizontal = 28.dp, vertical = 8.dp))
        LazyColumn {
            items(chats, key = { it.id }) { chat ->
                val title = chat.title.ifBlank { stringResource(R.string.new_chat) }
                NavigationDrawerItem(
                    label = {
                        Column {
                            Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text(
                                relativeTime(chat.updated, now),
                                style = MaterialTheme.typography.labelMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    },
                    selected = chat.id == openChatId,
                    onClick = { onOpen(chat.id) },
                    badge = {
                        IconButton(onClick = { confirmDelete = chat.id }) {
                            Icon(
                                painterResource(R.drawable.ic_delete),
                                contentDescription = stringResource(R.string.delete_chat, title),
                            )
                        }
                    },
                    modifier = Modifier.padding(NavigationDrawerItemDefaults.ItemPadding),
                )
            }
        }
    }

    confirmDelete?.let { id ->
        val title = chats.find { it.id == id }?.title?.ifBlank { null } ?: stringResource(R.string.new_chat)
        AlertDialog(
            onDismissRequest = { confirmDelete = null },
            title = { Text(stringResource(R.string.delete_chat_title)) },
            text = { Text(stringResource(R.string.delete_chat_text, title)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmDelete = null
                    onDelete(id)
                }) { Text(stringResource(R.string.delete)) }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = null }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}
