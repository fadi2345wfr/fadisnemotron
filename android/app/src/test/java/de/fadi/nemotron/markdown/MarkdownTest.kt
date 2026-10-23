package de.fadi.nemotron.markdown

import de.fadi.nemotron.markdown.Inline.Bold
import de.fadi.nemotron.markdown.Inline.Code
import de.fadi.nemotron.markdown.Inline.Italic
import de.fadi.nemotron.markdown.Inline.Link
import de.fadi.nemotron.markdown.Inline.Text
import org.junit.Assert.assertEquals
import org.junit.Test

class MarkdownTest {

    @Test
    fun `headings and paragraphs`() {
        val blocks = Markdown.parse("# Titel\n\nErste Zeile\nzweite Zeile\n\n### Klein ###\n#kein Titel")
        assertEquals(
            listOf(
                Block.Heading(1, listOf(Text("Titel"))),
                Block.Paragraph(listOf(Text("Erste Zeile\nzweite Zeile"))),
                Block.Heading(3, listOf(Text("Klein"))),
                Block.Paragraph(listOf(Text("#kein Titel"))),
            ),
            blocks,
        )
    }

    @Test
    fun `fenced code keeps its content verbatim`() {
        val blocks = Markdown.parse("Vorher:\n```kotlin\nval a = **b**\n\n  // eingerückt\n```\nNachher")
        assertEquals(
            listOf(
                Block.Paragraph(listOf(Text("Vorher:"))),
                Block.Code("kotlin", "val a = **b**\n\n  // eingerückt"),
                Block.Paragraph(listOf(Text("Nachher"))),
            ),
            blocks,
        )
        // While streaming the closing fence has not arrived yet.
        assertEquals(listOf(Block.Code("", "noch offen")), Markdown.parse("~~~\nnoch offen"))
        // Three backticks around a word on one line are inline code, not a fence.
        assertEquals(listOf(Block.Paragraph(listOf(Code("x")))), Markdown.parse("```x```"))
    }

    @Test
    fun `bullet and numbered lists`() {
        val blocks = Markdown.parse("Schritte:\n1. Bauen\n1. Testen\n   mit Details\n   - Unterpunkt\n2) Liefern\n\n- a\n* b\n\nEnde")
        assertEquals(
            listOf(
                Block.Paragraph(listOf(Text("Schritte:"))),
                Block.Bullets(
                    listOf(
                        ListItem("1.", 0, listOf(Text("Bauen"))),
                        ListItem("2.", 0, listOf(Text("Testen\nmit Details"))),
                        ListItem("◦", 1, listOf(Text("Unterpunkt"))),
                        ListItem("3.", 0, listOf(Text("Liefern"))),
                        ListItem("•", 0, listOf(Text("a"))),
                        ListItem("•", 0, listOf(Text("b"))),
                    ),
                ),
                Block.Paragraph(listOf(Text("Ende"))),
            ),
            blocks,
        )
        assertEquals("4.", (Markdown.parse("4. vier\n5. fünf")[0] as Block.Bullets).items[0].marker)
    }

    @Test
    fun `quotes, rules and tables`() {
        val blocks = Markdown.parse("> Zitat **fett**\n> weiter\n\n---\n\n| Datei | Zeilen |\n|:---|---:|\n| `a.rs` | 12 |\n| b.rs |")
        assertEquals(
            listOf(
                Block.Quote(listOf(Block.Paragraph(listOf(Text("Zitat "), Bold(listOf(Text("fett"))), Text("\nweiter"))))),
                Block.Rule,
                Block.Table(listOf("Datei", "Zeilen"), listOf(listOf("a.rs", "12"), listOf("b.rs", ""))),
            ),
            blocks,
        )
    }

    @Test
    fun `inline styles`() {
        assertEquals(
            listOf(
                Bold(listOf(Text("fett"))),
                Text(", "),
                Italic(listOf(Text("kursiv"))),
                Text(", "),
                Bold(listOf(Italic(listOf(Text("beides"))))),
                Text(", "),
                Code("a*b"),
                Text(" und "),
                Link(listOf(Text("Doku")), "https://example.com/x"),
            ),
            parseInlines("**fett**, *kursiv*, ***beides***, `a*b` und [Doku](https://example.com/x)"),
        )
        assertEquals(
            listOf(Bold(listOf(Text("außen "), Italic(listOf(Text("innen"))), Text(" außen")))),
            parseInlines("**außen *innen* außen**"),
        )
        assertEquals(listOf(Italic(listOf(Text("unter")))), parseInlines("_unter_"))
    }

    @Test
    fun `characters that only look like markup stay text`() {
        assertEquals(listOf(Text("snake_case_name und 2 * 3 * 4")), parseInlines("snake_case_name und 2 * 3 * 4"))
        assertEquals(listOf(Text("**offen und [kein](link")), parseInlines("**offen und [kein](link"))
        assertEquals(listOf(Text("*escaped*")), parseInlines("\\*escaped\\*"))
        assertEquals(listOf(Text("`einzeln")), parseInlines("`einzeln"))
    }

    @Test
    fun `bare web addresses become links`() {
        assertEquals(
            listOf(Text("Siehe "), Link(listOf(Text("https://build.nvidia.com/x")), "https://build.nvidia.com/x"), Text(".")),
            parseInlines("Siehe https://build.nvidia.com/x."),
        )
    }
}
