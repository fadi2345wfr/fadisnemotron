package de.fadi.nemotron.markdown

/** The subset of Markdown that chat answers use. */
sealed interface Block {
    data class Heading(val level: Int, val text: List<Inline>) : Block
    data class Paragraph(val text: List<Inline>) : Block
    data class Code(val language: String, val code: String) : Block
    data class Bullets(val items: List<ListItem>) : Block
    data class Quote(val blocks: List<Block>) : Block
    data class Table(val header: List<String>, val rows: List<List<String>>) : Block
    data object Rule : Block
}

/** A list item; [marker] is what to show (`•`, `3.`), [level] its nesting depth. */
data class ListItem(val marker: String, val level: Int, val text: List<Inline>)

sealed interface Inline {
    data class Text(val text: String) : Inline
    data class Bold(val children: List<Inline>) : Inline
    data class Italic(val children: List<Inline>) : Inline
    data class Code(val code: String) : Inline
    data class Link(val children: List<Inline>, val url: String) : Inline
}

object Markdown {
    fun parse(text: String): List<Block> = BlockParser(text.replace("\r\n", "\n").split('\n')).parse()
}

fun List<Inline>.plainText(): String = joinToString("") {
    when (it) {
        is Inline.Text -> it.text
        is Inline.Code -> it.code
        is Inline.Bold -> it.children.plainText()
        is Inline.Italic -> it.children.plainText()
        is Inline.Link -> it.children.plainText()
    }
}

private val HEADING = Regex("""^ {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t]*$""")
private val CLOSING_HASHES = Regex("""[ \t]+#+$""")
private val RULE = Regex("""^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$""")
private val LIST_ITEM = Regex("""^([ \t]*)([-*+]|\d{1,9}[.)])[ \t]+(.*)$""")
private val QUOTE = Regex("""^ {0,3}>[ ]?(.*)$""")
private val DELIMITER_CELL = Regex(""":?-+:?""")
private val BULLETS = listOf("•", "◦", "▪")

private class BlockParser(private val lines: List<String>) {
    private var i = 0

    fun parse(): List<Block> {
        val blocks = mutableListOf<Block>()
        while (i < lines.size) {
            val line = lines[i]
            if (line.isBlank()) {
                i++
                continue
            }
            blocks += when {
                fenceOf(line) != null -> code()
                HEADING.matches(line) -> heading()
                RULE.matches(line) -> {
                    i++
                    Block.Rule
                }
                LIST_ITEM.matches(line) -> bullets()
                QUOTE.matches(line) -> quote()
                isTableStart(i) -> table()
                else -> paragraph()
            }
        }
        return blocks
    }

    private fun code(): Block {
        val opening = lines[i]
        val (fence, language) = fenceOf(opening)!!
        val indent = opening.length - opening.trimStart().length
        val body = mutableListOf<String>()
        i++
        while (i < lines.size && !isClosingFence(lines[i], fence)) {
            body += lines[i].removeIndent(indent)
            i++
        }
        i++ // the closing fence; an unclosed block runs to the end, as while streaming
        return Block.Code(language, body.joinToString("\n"))
    }

    private fun heading(): Block {
        val match = HEADING.matchEntire(lines[i++])!!
        val text = match.groupValues[2].replace(CLOSING_HASHES, "")
        return Block.Heading(match.groupValues[1].length, parseInlines(text))
    }

    private fun bullets(): Block {
        val items = mutableListOf<ListItem>()
        val indents = ArrayDeque<Int>()
        val numbers = mutableMapOf<Int, Int>()
        while (i < lines.size) {
            val line = lines[i]
            if (line.isBlank()) {
                // A blank line only continues the list if another item follows.
                val next = (i until lines.size).firstOrNull { lines[it].isNotBlank() }
                if (next != null && LIST_ITEM.matches(lines[next]) && !RULE.matches(lines[next])) {
                    i = next
                    continue
                }
                break
            }
            val match = LIST_ITEM.matchEntire(line)
            if (match == null || RULE.matches(line)) break
            val indent = match.groupValues[1].replace("\t", "    ").length
            while (indents.isNotEmpty() && indents.last() > indent) indents.removeLast()
            if (indents.isEmpty() || indents.last() < indent) indents.addLast(indent)
            val level = indents.size - 1
            numbers.keys.removeAll { it > level }

            val marker = match.groupValues[2]
            val shown = if (marker[0].isDigit()) {
                // Models often number every item "1."; count on from the first number instead.
                val number = numbers[level]?.plus(1) ?: marker.dropLast(1).toInt()
                numbers[level] = number
                "$number."
            } else {
                numbers.remove(level)
                BULLETS[minOf(level, BULLETS.lastIndex)]
            }

            val text = StringBuilder(match.groupValues[3])
            i++
            while (i < lines.size && lines[i].isNotBlank() && !LIST_ITEM.matches(lines[i]) && !startsBlock(lines[i])) {
                text.append('\n').append(lines[i].trim())
                i++
            }
            items += ListItem(shown, level, parseInlines(text.toString()))
        }
        return Block.Bullets(items)
    }

    private fun quote(): Block {
        val inner = mutableListOf<String>()
        while (i < lines.size) {
            inner += QUOTE.matchEntire(lines[i])?.groupValues?.get(1) ?: break
            i++
        }
        return Block.Quote(BlockParser(inner).parse())
    }

    private fun isTableStart(at: Int): Boolean =
        '|' in lines[at] && at + 1 < lines.size && isDelimiterRow(lines[at + 1])

    private fun table(): Block {
        val header = cells(lines[i])
        i += 2
        val rows = mutableListOf<List<String>>()
        while (i < lines.size && lines[i].isNotBlank() && '|' in lines[i]) {
            val row = cells(lines[i++])
            rows += List(header.size) { row.getOrElse(it) { "" } }
        }
        return Block.Table(header, rows)
    }

    private fun paragraph(): Block {
        val text = StringBuilder(lines[i++].trim())
        while (i < lines.size) {
            val line = lines[i]
            if (line.isBlank() || startsBlock(line) || LIST_ITEM.matches(line) || isTableStart(i)) break
            text.append('\n').append(line.trim())
            i++
        }
        return Block.Paragraph(parseInlines(text.toString()))
    }

    private fun startsBlock(line: String) =
        fenceOf(line) != null || HEADING.matches(line) || RULE.matches(line) || QUOTE.matches(line)
}

/** `(fence, language)` if the line opens a fenced code block. */
private fun fenceOf(line: String): Pair<String, String>? {
    val trimmed = line.trimStart()
    if (line.length - trimmed.length > 3) return null
    val char = trimmed.firstOrNull()?.takeIf { it == '`' || it == '~' } ?: return null
    val fence = trimmed.takeWhile { it == char }
    if (fence.length < 3) return null
    val info = trimmed.substring(fence.length).trim()
    // ```a``` on one line is inline code, not a fence.
    if (char == '`' && '`' in info) return null
    return fence to info.substringBefore(' ')
}

private fun isClosingFence(line: String, fence: String): Boolean {
    val trimmed = line.trim()
    return trimmed.length >= fence.length && trimmed.all { it == fence[0] }
}

private fun String.removeIndent(indent: Int): String {
    val spaces = takeWhile { it == ' ' }.length
    return substring(minOf(spaces, indent))
}

private fun isDelimiterRow(line: String): Boolean =
    '|' in line && cells(line).let { cells -> cells.isNotEmpty() && cells.all { DELIMITER_CELL.matches(it) } }

private fun cells(line: String): List<String> {
    var row = line.trim()
    if (row.startsWith("|")) row = row.drop(1)
    if (row.endsWith("|") && !row.endsWith("\\|")) row = row.dropLast(1)
    return row.split(Regex("""(?<!\\)\|""")).map { parseInlines(it.trim().replace("\\|", "|")).plainText() }
}
