package de.fadi.nemotron.data

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/**
 * Lenient on purpose: the Mac app may be newer or older than this app, so
 * unknown keys are skipped, and missing values, `null`s and unknown enum
 * values fall back to the defaults below.
 */
internal val remoteJson = Json {
    ignoreUnknownKeys = true
    coerceInputValues = true
    explicitNulls = false
    isLenient = true
}

/** Everything the Mac app shows, as served by `GET /api/v1/state`. */
@Serializable
data class RemoteState(
    val version: Long = 0,
    val busy: Boolean = false,
    /** Nemotron is working but has said nothing for a while: the server probably hangs. */
    val stalled: Boolean = false,
    val thinking: Boolean = true,
    val approval: ApprovalMode = ApprovalMode.AutoEdits,
    val workspace: Workspace? = null,
    val notice: Notice? = null,
    /** The chat open on the Mac (and so on the phone); [messages] belong to it. */
    val chat: OpenChat? = null,
    /** All saved chats, newest first. */
    val chats: List<ChatSummary> = emptyList(),
    val changes: List<Change> = emptyList(),
    val messages: List<RemoteMessage> = emptyList(),
)

/** The agent's working folder; [home] when no project is open and it works in the Mac's home folder. */
@Serializable
data class Workspace(val name: String = "", val path: String = "", val home: Boolean = false)

@Serializable
data class OpenChat(val id: String = "", val title: String = "")

@Serializable
data class ChatSummary(
    val id: String = "",
    val title: String = "",
    /** Unix time in seconds. */
    val updated: Long = 0,
    val messages: Int = 0,
)

@Serializable
data class Notice(val text: String = "", val error: Boolean = false)

@Serializable
data class Change(
    val path: String = "",
    val added: Int = 0,
    val removed: Int = 0,
    val created: Boolean = false,
)

@Serializable
data class RemoteMessage(
    val role: String = "assistant",
    val text: String = "",
    val attachments: List<String> = emptyList(),
    val status: MessageStatus = MessageStatus.Done,
    val error: String? = null,
    @SerialName("finish_reason") val finishReason: String? = null,
    val notices: List<String> = emptyList(),
    /** The chat got too long and the work went on in the chat with this id. */
    @SerialName("continued_in") val continuedIn: String? = null,
    val steps: List<Step> = emptyList(),
) {
    val isUser: Boolean get() = role == "user"
}

@Serializable
data class Step(
    val reasoning: String = "",
    val content: String = "",
    val tools: List<Tool> = emptyList(),
)

@Serializable
data class Tool(
    val id: String = "",
    val name: String = "",
    val title: String = "",
    val state: ToolState = ToolState.Planned,
    val ok: Boolean = true,
    val output: String = "",
    val diff: String = "",
    val added: Int = 0,
    val removed: Int = 0,
    val warning: String? = null,
)

@Serializable
enum class ToolState {
    @SerialName("planned") Planned,
    @SerialName("awaiting_approval") AwaitingApproval,
    @SerialName("running") Running,
    @SerialName("done") Done,
    @SerialName("declined") Declined,
    @SerialName("not_run") NotRun,
    /** Was running when the user pressed Stop; may have partly run. */
    @SerialName("interrupted") Interrupted,
}

/** What Nemotron may do on the Mac without asking. */
@Serializable
enum class ApprovalMode { AskAll, AutoEdits, AutoAll }

@Serializable
enum class Decision {
    @SerialName("allow") Allow,
    @SerialName("allow_for_run") AllowForRun,
    @SerialName("deny") Deny,
}

@Serializable
data class UploadResult(val id: String = "", val kind: String = "", val size: Long = 0)
