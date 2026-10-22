package de.fadi.nemotron.data

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/** Where an answer stands. */
@Serializable
enum class MessageStatus {
    @SerialName("streaming") Streaming,
    @SerialName("done") Done,
    @SerialName("cancelled") Cancelled,
    @SerialName("failed") Failed,
}
