package de.fadi.nemotron.ui.common

import java.time.Duration
import java.time.Instant
import java.time.ZonedDateTime
import java.time.format.DateTimeFormatter
import java.time.temporal.ChronoUnit
import java.util.Locale

/** When a chat was last used, as the list shows it: "vor 5 Min.", "gestern", "12.10.". */
fun relativeTime(epochSeconds: Long, now: ZonedDateTime): String {
    val then = Instant.ofEpochSecond(epochSeconds).atZone(now.zone)
    val minutes = Duration.between(then, now).toMinutes()
    val days = ChronoUnit.DAYS.between(then.toLocalDate(), now.toLocalDate())
    return when {
        // Also when the Mac's clock runs a little ahead of the phone's.
        minutes < 1 -> "gerade eben"
        minutes < 60 -> "vor $minutes Min."
        days == 0L -> "vor ${minutes / 60} Std."
        days == 1L -> "gestern"
        then.year == now.year -> then.format(DAY_MONTH)
        else -> then.format(FULL_DATE)
    }
}

/** A video's length: "0:42", "12:05" or "1:02:03". */
fun formatDuration(millis: Long): String {
    val seconds = (millis + 500) / 1000
    val (hours, minutes, rest) = Triple(seconds / 3600, seconds / 60 % 60, seconds % 60)
    return if (hours > 0) {
        "%d:%02d:%02d".format(Locale.ROOT, hours, minutes, rest)
    } else {
        "%d:%02d".format(Locale.ROOT, minutes, rest)
    }
}

/** Upload progress in megabytes: "3,2 / 14 MB". */
fun formatMegabytes(sent: Long, total: Long): String = "${megabytes(sent)} / ${megabytes(total)} MB"

private fun megabytes(bytes: Long): String {
    val value = bytes / (1024.0 * 1024.0)
    return if (value >= 10) "%.0f".format(Locale.GERMANY, value) else "%.1f".format(Locale.GERMANY, value)
}

private val DAY_MONTH = DateTimeFormatter.ofPattern("d.M.")
private val FULL_DATE = DateTimeFormatter.ofPattern("d.M.yyyy")
