package de.fadi.nemotron.ui.direct

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider.AndroidViewModelFactory.Companion.APPLICATION_KEY
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import de.fadi.nemotron.FadisApplication
import de.fadi.nemotron.data.ChatMessage
import de.fadi.nemotron.data.DirectApi
import de.fadi.nemotron.data.DirectException
import de.fadi.nemotron.data.DirectSettings
import de.fadi.nemotron.data.MessageStatus
import de.fadi.nemotron.data.Photo
import de.fadi.nemotron.data.PhotoEncoder
import de.fadi.nemotron.data.Prefs
import de.fadi.nemotron.data.Reply
import de.fadi.nemotron.ui.common.Attachments
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** One question and Nemotron's answer to it. */
data class Turn(
    val id: Long,
    val text: String,
    val photos: List<Photo>,
    val reply: Reply = Reply(),
    val status: MessageStatus = MessageStatus.Streaming,
    val error: String? = null,
)

data class DirectUiState(
    val settings: DirectSettings,
    val turns: List<Turn> = emptyList(),
    val message: String? = null,
) {
    val busy: Boolean get() = turns.lastOrNull()?.status == MessageStatus.Streaming
}

/** Chat straight with NVIDIA's API. The conversation lives here, so it survives rotation. */
class DirectViewModel(
    private val prefs: Prefs,
    private val api: DirectApi,
    photos: PhotoEncoder,
) : ViewModel() {
    private val _state = MutableStateFlow(DirectUiState(settings = prefs.direct))
    val state: StateFlow<DirectUiState> = _state.asStateFlow()

    var draft by mutableStateOf("")

    val attachments = Attachments(viewModelScope, photos, ::showMessage)

    private var request: Job? = null
    private var nextId = 0L

    fun send() {
        val text = draft.trim()
        val photos = attachments.photos.value
        val current = _state.value
        if (text.isEmpty() && photos.isEmpty() || current.busy) return

        // Earlier answers go back as plain answers; their thinking stays on the phone.
        val history = current.turns.flatMap { turn ->
            listOfNotNull(
                ChatMessage(ChatMessage.Role.User, turn.text, turn.photos.map { it.base64 }),
                turn.reply.answer.takeIf { it.isNotBlank() }?.let { ChatMessage(ChatMessage.Role.Assistant, it) },
            )
        } + ChatMessage(ChatMessage.Role.User, text, photos.map { it.base64 })

        val turn = Turn(id = nextId++, text = text, photos = photos)
        _state.update { it.copy(turns = it.turns + turn) }
        draft = ""
        attachments.clear()

        request = viewModelScope.launch {
            try {
                val reply = api.complete(current.settings, history) { partial ->
                    updateTurn(turn.id) { it.copy(reply = partial) }
                }
                updateTurn(turn.id) { it.copy(reply = reply, status = MessageStatus.Done) }
            } catch (e: DirectException) {
                updateTurn(turn.id) { it.copy(status = MessageStatus.Failed, error = e.message) }
            } catch (e: CancellationException) {
                updateTurn(turn.id) { it.copy(status = MessageStatus.Cancelled) }
                throw e
            }
        }
    }

    fun stop() {
        request?.cancel()
    }

    fun newChat() {
        request?.cancel()
        _state.update { it.copy(turns = emptyList()) }
    }

    fun saveSettings(settings: DirectSettings) {
        prefs.direct = settings
        _state.update { it.copy(settings = settings) }
    }

    fun messageShown() = _state.update { it.copy(message = null) }

    private fun showMessage(message: String) = _state.update { it.copy(message = message) }

    private fun updateTurn(id: Long, change: (Turn) -> Turn) = _state.update { state ->
        state.copy(turns = state.turns.map { if (it.id == id) change(it) else it })
    }

    companion object {
        val Factory = viewModelFactory {
            initializer {
                val app = this[APPLICATION_KEY] as FadisApplication
                DirectViewModel(app.prefs, DirectApi(app.http), app.photos)
            }
        }
    }
}
