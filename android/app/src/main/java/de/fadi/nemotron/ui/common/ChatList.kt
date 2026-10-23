package de.fadi.nemotron.ui.common

import androidx.compose.foundation.interaction.DragInteraction
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Icon
import androidx.compose.material3.SmallFloatingActionButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import de.fadi.nemotron.R
import kotlinx.coroutines.launch

/**
 * Scroll position of a chat plus whether it follows new content.
 *
 * It follows while the reader is at the end. Dragging the list away from the
 * end stops following, so the reader can scroll freely while an answer
 * streams in; scrolling back to the end, or [jumpToEnd], resumes it.
 */
@Stable
class ChatListState(val list: LazyListState, follow: MutableState<Boolean>) {
    var follow by follow

    fun jumpToEnd() {
        follow = true
    }
}

@Composable
fun rememberChatListState(): ChatListState {
    val list = rememberLazyListState()
    val follow = rememberSaveable { mutableStateOf(true) }
    return remember(list, follow) { ChatListState(list, follow) }
}

/**
 * A chat transcript. [contentKey] must change whenever the content grows,
 * e.g. the state version or the length of the streamed text.
 */
@Composable
fun ChatList(
    state: ChatListState,
    contentKey: Any,
    modifier: Modifier = Modifier,
    content: LazyListScope.() -> Unit,
) {
    val list = state.list
    LaunchedEffect(list) {
        var dragged = false
        launch {
            list.interactionSource.interactions.collect { if (it is DragInteraction.Start) dragged = true }
        }
        // Programmatic scrolls and growing content never change `follow`, only
        // the end of a scroll gesture (including its fling) does.
        snapshotFlow { list.isScrollInProgress }.collect { scrolling ->
            if (!scrolling && dragged) {
                dragged = false
                state.follow = !list.canScrollForward
            }
        }
    }
    val key by rememberUpdatedState(contentKey)
    LaunchedEffect(list) {
        // The item count is part of the key because new items are only counted after the next layout.
        snapshotFlow { listOf(key, state.follow, list.layoutInfo.viewportSize, list.layoutInfo.totalItemsCount) }
            .collect { if (state.follow && !list.isScrollInProgress) list.scrollToEnd() }
    }

    Box(modifier) {
        LazyColumn(
            state = list,
            contentPadding = PaddingValues(horizontal = 12.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier.fillMaxSize(),
        ) {
            content()
            // Scrolling to this last item puts the end of the real content in view.
            item(key = END_KEY) {}
        }
        if (!state.follow && list.canScrollForward) {
            SmallFloatingActionButton(
                onClick = state::jumpToEnd,
                modifier = Modifier.align(Alignment.BottomEnd).padding(16.dp),
            ) {
                Icon(painterResource(R.drawable.ic_arrow_down), contentDescription = stringResource(R.string.jump_to_end))
            }
        }
    }
}

private suspend fun LazyListState.scrollToEnd() {
    val last = layoutInfo.totalItemsCount - 1
    if (last >= 0) scrollToItem(last)
}

private const val END_KEY = "ende"
