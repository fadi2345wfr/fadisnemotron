package de.fadi.nemotron.data

import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.json.Json

/**
 * What was sent to Nemotron before, to call it back: with the history button,
 * or with the arrow keys of a keyboard (up = older, down = newer). Kept
 * across restarts through [load] and [save].
 */
class PromptHistory(
    load: () -> String?,
    private val save: (String) -> Unit,
) {
    private val entries: MutableList<String> = decode(load()).toMutableList()
    private var position: Int? = null
    private var draft = ""

    /** Newest first, for a menu. */
    val recent: List<String> get() = entries.asReversed()

    val isEmpty: Boolean get() = entries.isEmpty()

    /** Remembers a message that was sent; an older identical one moves up. */
    fun add(text: String) {
        position = null
        draft = ""
        val clean = text.trim().take(MAX_CHARS)
        if (clean.isEmpty()) return
        entries.remove(clean)
        entries.add(clean)
        while (entries.size > MAX_ENTRIES) entries.removeAt(0)
        save(Json.encodeToString(ListSerializer(String.serializer()), entries))
    }

    /** True while an entry is shown unchanged: up and down then walk the history. */
    fun isBrowsing(current: String): Boolean = position?.let { entries.getOrNull(it) == current } == true

    /** One step back; [current] is what the field holds now. `null` if there is nothing older. */
    fun older(current: String): String? {
        val target = when (val p = position) {
            null -> {
                if (entries.isEmpty()) return null
                draft = current
                entries.size - 1
            }
            0 -> return null
            else -> p - 1
        }
        position = target
        return entries[target]
    }

    /** One step forward; after the newest comes back what was typed before. */
    fun newer(): String? {
        val p = position ?: return null
        return if (p + 1 < entries.size) {
            position = p + 1
            entries[p + 1]
        } else {
            position = null
            draft.also { draft = "" }
        }
    }

    /** A pick from the menu: shown like any entry, without browsing from there. */
    fun stopBrowsing() {
        position = null
        draft = ""
    }

    private companion object {
        const val MAX_ENTRIES = 50
        const val MAX_CHARS = 4_000

        fun decode(raw: String?): List<String> = try {
            if (raw.isNullOrBlank()) {
                emptyList()
            } else {
                Json.decodeFromString(ListSerializer(String.serializer()), raw).takeLast(MAX_ENTRIES)
            }
        } catch (e: IllegalArgumentException) {
            // Damaged: start over rather than crash.
            emptyList()
        }
    }
}
