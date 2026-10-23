package de.fadi.nemotron.data

/**
 * Splits a model turn into `(thinking, answer)`.
 *
 * Nemotron's thinking normally arrives in its own `reasoning` field. Servers
 * without a reasoning parser leave it inline as `<think>…</think>` at the
 * start of the content instead; while the closing tag has not arrived yet,
 * everything after `<think>` is still thinking.
 */
fun thinkingAndAnswer(reasoning: String, content: String): Pair<String, String> {
    if (reasoning.isNotBlank()) return reasoning.trim() to content.trimStart()
    val end = content.indexOf(THINK_END)
    if (end >= 0) {
        val thinking = content.substring(0, end).trimStart().removePrefix(THINK_START).trim()
        return thinking to content.substring(end + THINK_END.length).trimStart()
    }
    val trimmed = content.trimStart()
    if (trimmed.startsWith(THINK_START)) return trimmed.removePrefix(THINK_START).trim() to ""
    return "" to content
}

private const val THINK_START = "<think>"
private const val THINK_END = "</think>"
