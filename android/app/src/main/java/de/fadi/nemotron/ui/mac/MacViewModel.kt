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
import de.fadi.nemotron.data.Pairing
import de.fadi.nemotron.data.PairingCode
import de.fadi.nemotron.data.PhotoEncoder
import de.fadi.nemotron.data.Prefs
import de.fadi.nemotron.data.PromptHistory
import de.fadi.nemotron.data.RemoteApi
import de.fadi.nemotron.data.RemoteException
import de.fadi.nemotron.data.RemoteState
import de.fadi.nemotron.data.Upload
import de.fadi.nemotron.data.VideoEncoder
import de.fadi.nemotron.data.connect
import de.fadi.nemotron.data.invertDiff
import de.fadi.nemotron.data.normalizeAddress
import de.fadi.nemotron.data.parsePairingCode
import de.fadi.nemotron.ui.common.Attachments
import de.fadi.nemotron.ui.common.UploadProgress
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
    /** The Mac app answers, but speaks an older protocol than this app needs. */
    val macTooOld: Boolean = false,
    val pairingInProgress: Boolean = false,
    val pairingError: String? = null,
    val sending: Boolean = false,
    /** Set while the attachments of the message being sent are uploaded. */
    val upload: UploadProgress? = null,
    /** Tool calls whose decision is on its way to the Mac. */
    val deciding: Set<String> = emptySet(),
    val diff: DiffView? = null,
    val message: String? = null,
)

/** A file's diff on screen. After an undo [undone] is set and [diff] shows the undo itself. */
data class DiffView(val path: String, val diff: String, val undone: Boolean = false)

/** The Mac app on the phone: pairing, the long-poll loop and the actions. */
class MacViewModel(
    private val prefs: Prefs,
    private val http: OkHttpClient,
    photos: PhotoEncoder,
    videos: VideoEncoder,
) : ViewModel() {
    private val _state = MutableStateFlow(MacUiState(pairing = prefs.pairing))
    val state: StateFlow<MacUiState> = _state.asStateFlow()

    /** The message being typed; plain Compose state so the text field never lags. */
    var draft by mutableStateOf("")

    val attachments = Attachments(viewModelScope, photos, videos, ::showMessage)

    /** What was sent from this phone, for the history button and a keyboard's arrow keys. */
    private val history = PromptHistory(load = { prefs.history }, save = { prefs.history = it })

    /** Newest first. Read when the menu is built, so it is current after every send. */
    val recentPrompts: List<String> get() = history.recent

    /** Arrow up: an earlier message, if the field is empty or shows one. True if the key was used. */
    fun historyOlder(): Boolean {
        if (draft.isNotEmpty() && !history.isBrowsing(draft)) return false
        val older = history.older(draft) ?: return history.isBrowsing(draft)
        draft = older
        return true
    }

    /** Arrow down while an earlier message shows: the next one, or what was typed before. */
    fun historyNewer(): Boolean {
        if (!history.isBrowsing(draft)) return false
        draft = history.newer() ?: return false
        return true
    }

    fun pickPrompt(text: String) {
        history.stopBrowsing()
        draft = text
    }

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
        // Checked again after every lost connection: the Mac app may have been replaced meanwhile.
        var checked = false
        while (true) {
            try {
                if (!checked) {
                    if (!api.ping().supported) {
                        _state.update { it.copy(macTooOld = true, remote = null, connectionLost = false) }
                        return
                    }
                    _state.update { it.copy(macTooOld = false) }
                    checked = true
                }
                val remote = api.state(after = _state.value.remote?.version ?: 0)
                _state.update { it.copy(remote = remote, connectionLost = false) }
                backoff = MIN_BACKOFF_MS
            } catch (e: RemoteException) {
                if (e.status == 401) {
                    // The Mac made a new key: this phone has to pair again.
                    forget(e.message)
                    return
                }
                checked = false
                _state.update { it.copy(connectionLost = true) }
                delay(backoff)
                backoff = (backoff * 2).coerceAtMost(MAX_BACKOFF_MS)
            }
        }
    }

    /** After the Mac app was updated. */
    fun checkAgain() {
        _state.update { it.copy(macTooOld = false) }
        updatePolling()
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

    fun unpair() = forget(null)

    private fun forget(reason: String?) {
        prefs.pairing = null
        api = null
        updatePolling()
        _state.value = MacUiState(pairingError = reason)
    }

    /** Uploads the attachments, then sends; on failure text and attachments stay for another try. */
    fun send() {
        val api = api ?: return
        val typed = draft
        val text = typed.trim()
        val items = attachments.items.value
        if (text.isEmpty() && items.isEmpty() || _state.value.sending || attachments.preparing.value > 0) return
        val uploads = items.map { it.upload ?: return }
        // While Nemotron works, a new message ends that answer and is answered at once.
        val interrupt = _state.value.remote?.busy == true
        _state.update { it.copy(sending = true) }
        viewModelScope.launch {
            try {
                api.send(text, upload(api, uploads), interrupt)
                history.add(text)
                // The field stays editable during a long upload; whatever was typed meanwhile stays.
                draft = draftAfterSending(draft, typed)
                items.forEach { attachments.remove(it.key) }
            } catch (e: RemoteException) {
                showMessage(e.message)
            } finally {
                _state.update { it.copy(sending = false, upload = null) }
            }
        }
    }

    private suspend fun upload(api: RemoteApi, uploads: List<Upload>): List<String> {
        val total = uploads.sumOf { it.body.length }
        var done = 0L
        return uploads.map { upload ->
            val result = api.upload(upload.name, upload.contentType, upload.body) { sent ->
                _state.update { it.copy(upload = UploadProgress(done + sent, total)) }
            }
            done += upload.body.length
            result.id
        }
    }

    fun stop() = act { it.stop() }

    fun newChat() = whenIdle { it.newChat() }

    fun openChat(id: String) {
        if (id != _state.value.remote?.chat?.id) whenIdle { it.openChat(id) }
    }

    fun deleteChat(id: String) = whenIdle { it.deleteChat(id) }

    fun retry() = act { it.retry() }

    fun setApproval(mode: ApprovalMode) = act { it.settings(approval = mode) }

    fun setThinking(enabled: Boolean) = act { it.settings(thinking = enabled) }

    fun undo(path: String) = act { it.undo(path) }

    fun showDiff(path: String) = act { api ->
        val diff = api.diff(path)
        _state.update { it.copy(diff = DiffView(path, diff)) }
    }

    fun closeDiff() = _state.update { it.copy(diff = null) }

    /** Undo from the diff itself; the dialog then shows what the undo did. */
    fun undoFromDiff() {
        val view = _state.value.diff ?: return
        if (view.undone) return
        if (_state.value.remote?.busy == true) {
            showMessage(RemoteApi.BUSY)
            return
        }
        act { api ->
            api.undo(view.path)
            // Still the same dialog? Then it turns into the result.
            _state.update {
                if (it.diff?.path == view.path) it.copy(diff = DiffView(view.path, invertDiff(view.diff), undone = true)) else it
            }
        }
    }

    /** After an error stopped the answer: Nemotron goes on where it stopped. */
    fun continueAfterError() = act { it.send(CONTINUE_TEXT) }

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

    /**
     * Chats cannot change while an answer runs; saying so right away beats a
     * round trip. Nor while a message is still uploading: the Mac puts it
     * into whichever chat is open once the upload is done.
     */
    private fun whenIdle(action: suspend (RemoteApi) -> Unit) {
        val state = _state.value
        when {
            state.sending -> showMessage(STILL_SENDING)
            state.remote?.busy == true -> showMessage(RemoteApi.BUSY)
            else -> act(action)
        }
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

    override fun onCleared() = attachments.clear()

    companion object {
        private const val MIN_BACKOFF_MS = 2_000L
        private const val MAX_BACKOFF_MS = 10_000L
        private const val STILL_SENDING = "Die Nachricht wird noch gesendet."
        private const val CONTINUE_TEXT =
            "Weiter: Mach genau dort weiter, wo du wegen des Fehlers aufgehört hast. " +
                "Schau kurz nach, was schon erledigt ist, und mach nichts doppelt."

        val Factory = viewModelFactory {
            initializer {
                val app = this[APPLICATION_KEY] as FadisApplication
                MacViewModel(app.prefs, app.http, app.photos, app.videos)
            }
        }
    }
}

/**
 * The text field after [sent] went out: empty, or what was typed after it
 * while it was on its way. If [sent] was edited meanwhile, all is kept.
 */
internal fun draftAfterSending(draft: String, sent: String): String =
    if (draft.startsWith(sent)) draft.substring(sent.length).trimStart() else draft
