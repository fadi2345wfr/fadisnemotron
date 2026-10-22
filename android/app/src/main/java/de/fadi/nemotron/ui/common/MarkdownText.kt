package de.fadi.nemotron.ui.common

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import de.fadi.nemotron.markdown.Block
import de.fadi.nemotron.markdown.Inline
import de.fadi.nemotron.markdown.Markdown

/** Renders a chat answer written in Markdown; the text can be selected and copied in parts. */
@Composable
fun MarkdownText(markdown: String, modifier: Modifier = Modifier) {
    val blocks = remember(markdown) { Markdown.parse(markdown) }
    SelectionContainer(modifier) { Blocks(blocks) }
}

@Composable
private fun Blocks(blocks: List<Block>, modifier: Modifier = Modifier) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (block in blocks) BlockView(block)
    }
}

@Composable
private fun BlockView(block: Block) {
    val typography = MaterialTheme.typography
    when (block) {
        is Block.Heading -> InlineText(
            block.text,
            when (block.level) {
                1 -> typography.titleLarge
                2 -> typography.titleMedium
                else -> typography.titleSmall
            },
        )
        is Block.Paragraph -> InlineText(block.text, typography.bodyLarge)
        is Block.Code -> CodeBox(block.code.ifEmpty { " " })
        is Block.Bullets -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            for (item in block.items) {
                Row(Modifier.padding(start = 18.dp * item.level)) {
                    Text(
                        item.marker,
                        style = typography.bodyLarge,
                        modifier = Modifier.widthIn(min = 22.dp).padding(end = 6.dp),
                    )
                    InlineText(item.text, typography.bodyLarge)
                }
            }
        }
        is Block.Quote -> Row(Modifier.height(IntrinsicSize.Min)) {
            Spacer(
                Modifier
                    .width(3.dp)
                    .fillMaxHeight()
                    .background(MaterialTheme.colorScheme.outline),
            )
            Blocks(block.blocks, Modifier.padding(start = 10.dp))
        }
        is Block.Table -> CodeBox(tableText(block))
        Block.Rule -> HorizontalDivider()
    }
}

@Composable
private fun InlineText(inlines: List<Inline>, style: TextStyle) {
    val colors = MaterialTheme.colorScheme
    val text = remember(inlines, colors) {
        buildAnnotatedString { appendInlines(inlines, colors.primary, colors.surfaceContainerHighest) }
    }
    Text(text, style = style)
}

private fun AnnotatedString.Builder.appendInlines(inlines: List<Inline>, link: Color, codeBackground: Color) {
    for (inline in inlines) {
        when (inline) {
            is Inline.Text -> append(inline.text)
            is Inline.Bold -> withStyle(SpanStyle(fontWeight = FontWeight.Bold)) {
                appendInlines(inline.children, link, codeBackground)
            }
            is Inline.Italic -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) {
                appendInlines(inline.children, link, codeBackground)
            }
            is Inline.Code -> withStyle(SpanStyle(fontFamily = FontFamily.Monospace, background = codeBackground)) {
                append(inline.code)
            }
            is Inline.Link -> {
                val style = SpanStyle(color = link, textDecoration = TextDecoration.Underline)
                // Only web links open; anything else the model wrote is just shown.
                if (inline.url.startsWith("https://") || inline.url.startsWith("http://")) {
                    withLink(LinkAnnotation.Url(inline.url, TextLinkStyles(style))) {
                        appendInlines(inline.children, link, codeBackground)
                    }
                } else {
                    withStyle(style) { appendInlines(inline.children, link, codeBackground) }
                }
            }
        }
    }
}

/** Lays a table out as aligned monospace columns with a bold header. */
private fun tableText(table: Block.Table): AnnotatedString {
    val rows = listOf(table.header) + table.rows
    val widths = table.header.indices.map { column -> rows.maxOf { it.getOrElse(column) { "" }.length } }
    fun line(row: List<String>) = row.mapIndexed { column, cell -> cell.padEnd(widths[column]) }.joinToString("  ").trimEnd()
    return buildAnnotatedString {
        withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(line(table.header)) }
        append('\n')
        append(widths.joinToString("  ") { "─".repeat(it) })
        for (row in table.rows) {
            append('\n')
            append(line(row))
        }
    }
}
