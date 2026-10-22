package de.fadi.nemotron.markdown

fun parseInlines(text: String): List<Inline> = InlineParser(text).parse(0, text.length)

private const val ESCAPABLE = "\\`*_{}[]()#+-.!|<>~"
private const val URL_END = " \t\n<>\"'"
private const val URL_TRAILING = ".,;:!?)"

private class InlineParser(private val s: String) {

    fun parse(start: Int, end: Int): List<Inline> {
        val out = mutableListOf<Inline>()
        val text = StringBuilder()
        fun flush() {
            if (text.isNotEmpty()) {
                out += Inline.Text(text.toString())
                text.clear()
            }
        }

        var i = start
        while (i < end) {
            val c = s[i]
            when {
                c == '\\' && i + 1 < end && s[i + 1] in ESCAPABLE -> {
                    text.append(s[i + 1])
                    i += 2
                }
                c == '`' -> {
                    val run = runLength(i, end, c)
                    val close = closingBackticks(i + run, end, run)
                    if (close < 0) {
                        text.append(s, i, i + run)
                        i += run
                    } else {
                        flush()
                        out += Inline.Code(codeContent(s.substring(i + run, close)))
                        i = close + run
                    }
                }
                c == '*' || c == '_' -> {
                    val run = runLength(i, end, c)
                    val close = if (run <= 3 && canOpen(i, run, end)) closingEmphasis(i + run, end, c, run) else -1
                    if (close < 0) {
                        text.append(s, i, i + run)
                        i += run
                    } else {
                        flush()
                        val inner = parse(i + run, close)
                        out += when (run) {
                            1 -> Inline.Italic(inner)
                            2 -> Inline.Bold(inner)
                            else -> Inline.Bold(listOf(Inline.Italic(inner)))
                        }
                        i = close + run
                    }
                }
                c == '[' -> {
                    val link = link(i, end)
                    if (link == null) {
                        text.append(c)
                        i++
                    } else {
                        flush()
                        out += link.first
                        i = link.second
                    }
                }
                c == 'h' && startsUrl(i, start) -> {
                    var stop = i
                    while (stop < end && s[stop] !in URL_END) stop++
                    while (stop > i && s[stop - 1] in URL_TRAILING) stop--
                    val url = s.substring(i, stop)
                    flush()
                    out += Inline.Link(listOf(Inline.Text(url)), url)
                    i = stop
                }
                else -> {
                    text.append(c)
                    i++
                }
            }
        }
        flush()
        return out
    }

    private fun runLength(from: Int, end: Int, c: Char): Int {
        var j = from
        while (j < end && s[j] == c) j++
        return j - from
    }

    private fun closingBackticks(from: Int, end: Int, run: Int): Int {
        var j = from
        while (j < end) {
            if (s[j] == '`') {
                val length = runLength(j, end, '`')
                if (length == run) return j
                j += length
            } else {
                j++
            }
        }
        return -1
    }

    /** `*` and `_` only open emphasis before a non-space; `_` not inside a word like snake_case. */
    private fun canOpen(at: Int, run: Int, end: Int): Boolean {
        val next = at + run
        if (next >= end || s[next].isWhitespace()) return false
        return s[at] != '_' || at == 0 || !s[at - 1].isLetterOrDigit()
    }

    private fun closingEmphasis(from: Int, end: Int, c: Char, run: Int): Int {
        var j = from
        while (j < end) {
            when (s[j]) {
                '\\' -> j += 2
                '`' -> {
                    val length = runLength(j, end, '`')
                    val close = closingBackticks(j + length, end, length)
                    j = if (close < 0) j + length else close + length
                }
                c -> {
                    val length = runLength(j, end, c)
                    val after = j + length
                    val closes = length == run && j > from && !s[j - 1].isWhitespace() &&
                        (c != '_' || after >= end || !s[after].isLetterOrDigit())
                    if (closes) return j
                    j = after
                }
                else -> j++
            }
        }
        return -1
    }

    /** `[text](url)`; returns the link and the index after it. */
    private fun link(open: Int, end: Int): Pair<Inline, Int>? {
        var depth = 0
        var j = open
        while (j < end) {
            when (s[j]) {
                '\\' -> j++
                '[' -> depth++
                ']' -> if (--depth == 0) break
            }
            j++
        }
        if (j >= end || j + 1 >= end || s[j + 1] != '(') return null
        val close = s.indexOf(')', j + 2).takeIf { it in 0 until end } ?: return null
        val url = s.substring(j + 2, close).trim().substringBefore(' ').removePrefix("<").removeSuffix(">")
        if (url.isEmpty()) return null
        return Inline.Link(parse(open + 1, j), url) to close + 1
    }

    private fun startsUrl(at: Int, start: Int): Boolean =
        (s.startsWith("https://", at) || s.startsWith("http://", at)) &&
            (at == start || !s[at - 1].isLetterOrDigit())

    private fun codeContent(code: String): String =
        if (code.length >= 2 && code.startsWith(' ') && code.endsWith(' ') && code.isNotBlank()) {
            code.substring(1, code.length - 1)
        } else {
            code
        }
}
