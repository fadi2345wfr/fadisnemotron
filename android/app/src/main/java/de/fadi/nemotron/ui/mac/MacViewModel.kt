package de.fadi.nemotron.ui.mac

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider.AndroidViewModelFactory.Companion.APPLICATION_KEY
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import de.fadi.nemotron.FadisApplication
import de.fadi.nemotron.data.ApprovalMode
import de.fadi.nemotron.data.Decision
import de.fadi.nemotron.data.ImageUpload
import de.fadi.nemotron.data.Pairing
import de.fadi.nemotron.data.PairingCode
import de.fadi.nemotron.data.PhotoEncoder
import de.fadi.nemotron.data.Prefs
import de.fadi.nemotron.data.RemoteApi
import de.fadi.nemotron.data.RemoteException
import de.fadi.nemotron.data.RemoteState
import de.fadi.nemotron.data.connect
import de.fadi.nemotron.data.normalizeAddress
import de.fadi.nemotron.data.parsePairingCode
import de.fadi.nemotron.ui.common.Attachments
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient

data class MacUiState(
    val pairing: Pairing? = null,
    val remote: RemoteState? = null,
    val connectionLost: Boolean = false,
    val pairingInProgress: Boolean = false,
    val pairingError: String? = null,
    val sending: Boolean = false,
    /** Tool calls whose decision is on its way to the Mac. */
    val deciding: Set<String> = emptySet(),
    val diff: DiffView? = null,
    val message: String? = null,
)

data class DiffView(val path: String, val diff: String)

/** Remote control of the Mac app: pairing, the long-poll loop and the actions. */
class MacViewModel(
    private val prefs: Prefs,
    private val http: OkHttpClient,
    photos: PhotoEncoder,
) : ViewModel() {
    private val _state = MutableStateFlow(MacUiState(pairing = prefs.pairing))
    val state: StateFlow<MacUiState> = _state.asStateFlow()

    /** The message being typed; plain Compose state so the text field never lags. */
    var draft by mutableStateOf("")

    val attachments = Attachments(viewModelScope, photos, ::showMessage)

    private var api: RemoteApi? = prefs.pairing?.let { RemoteApi(it.url, it.token, http) }
    private var visible = false
    private var polling: Job? = null

    /** Polls only while the screen is on display; the UI calls this from its lifecycle. */
    fun setVisible(visible: Boolean) {
        this.visible = visible
        updatePolling()
    }

    private fun updatePolling() {
        val api = api
        if (visible && api != null) {
            if (polling?.isActive != true) polling = viewModelScope.launch { poll(api) }
        } else {
            polling?.cancel()
            polling = null
        }
    }

    private suspend fun poll(api: RemoteApi) {
        var backoff = MIN_BACKOFF_MS
        while (true) {
            try {
                val remote = api.state(after = _state.value.remote?.version ?: 0)
                _state.update { it.copy(remote = remote, connectionLost = false) }
                backoff = MIN_BACKOFF_MS
            } catch (e: RemoteException) {
                if (e.status == 401) {
                    // The Mac made a new key: this phone has to pair again.
                    forget(e.message)
                    return
                }
                _state.update { it.copy(connectionLost = true) }
                delay(backoff)
                backoff = (backoff * 2).coerceAtMost(MAX_BACKOFF_MS)
            }
        }
    }

    fun pairWithCode(raw: String) {
        val code = parsePairingCode(raw)
        if (code == null) {
            _state.update { it.copy(pairingError = "Das ist kein QR-Code von Fadis Nemotron.") }
            return
        }
        pair(code)
    }

    fun pairManually(address: String, token: String) {
        val url = normalizeAddress(address)
        when {
            url == null -> _state.update {
                it.copy(pairingError = "Die Adresse ist ungültig. Beispiel: http://192.168.1.20:7878")
            }
            token.isBlank() -> _state.update { it.copy(pairingError = "Bitte den Schlüssel eingeben.") }
            else -> pair(PairingCode(fadis = 1, urls = listOf(url), token = token.trim()))
        }
    }

    fun scanFailed(reason: String) {
        _state.update { it.copy(pairingError = reason) }
    }

    private fun pair(code: PairingCode) {
        if (_state.value.pairingInProgress) return
        _state.update { it.copy(pairingInProgress = true, pairingError = null) }
        viewModelScope.launch {
            try {
                val pairing = connect(code, http)
                prefs.pairing = pairing
                api = RemoteApi(pairing.url, pairing.token, http)
                _state.value = MacUiState(pairing = pairing)
                updatePolling()
            } catch (e: RemoteException) {
                _state.update { it.copy(pairingInProgress = false, pairingError = e.message) }
            }
        }
    }

    fun disconnect() = forget(null)

    private fun forget(reason: String?) {
        prefs.pairing = null
        api = null
        updatePolling()
        _state.value = MacUiState(pairingError = reason)
    }

    fun send() {
        val api = api ?: return
        val text = draft.trim()
        val photos = attachments.photos.value
        if (text.isEmpty() && photos.isEmpty() || _state.value.sending) return
        _state.update { it.copy(sending = true) }
        viewModelScope.launch {
            try {
                api.send(text, photos.map { ImageUpload(it.name, it.base64) })
                draft = ""
                attachments.clear()
            } catch (e: RemoteException) {
                showMessage(e.message)
            } finally {
                _state.update { it.copy(sending = false) }
            }
        }
    }

    fun stop() = act { it.stop() }

    fun newChat() = act { it.newChat() }

    fun retry() = act { it.retry() }

    fun setApproval(mode: ApprovalMode) = act { it.settings(approval = mode) }

    fun setThinking(enabled: Boolean) = act { it.settings(thinking = enabled) }

    fun undo(path: String) = act { it.undo(path) }

    fun showDiff(path: String) = act { api ->
        val diff = api.diff(path)
        _state.update { it.copy(diff = DiffView(path, diff)) }
    }

    fun closeDiff() = _state.update { it.copy(diff = null) }

    fun decide(callId: String, decision: Decision) {
        if (callId in _state.value.deciding) return
        _state.update { it.copy(deciding = it.deciding + callId) }
        act { api ->
            try {
                api.decide(callId, decision)
            } finally {
                _state.update { it.copy(deciding = it.deciding - callId) }
            }
        }
    }

    fun messageShown() = _state.update { it.copy(message = null) }

    private fun showMessage(message: String?) {
        if (message != null) _state.update { it.copy(message = message) }
    }

    private fun act(action: suspend (RemoteApi) -> Unit) {
        val api = api ?: return
        viewModelScope.launch {
            try {
                action(api)
            } catch (e: RemoteException) {
                showMessage(e.message)
            }
        }
    }

    companion object {
        private const val MIN_BACKOFF_MS = 2_000L
        private const val MAX_BACKOFF_MS = 10_000L

        val Factory = viewModelFactory {
            initializer {
                val app = this[APPLICATION_KEY] as FadisApplication
                MacViewModel(app.prefs, app.http, app.photos)
            }
        }
    }
}
