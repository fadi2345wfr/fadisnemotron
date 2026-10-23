package de.fadi.nemotron.data

import org.junit.Assert.assertEquals
import org.junit.Test

class DiffsTest {
    @Test
    fun `an undo shows the change backwards`() {
        val change = """
            --- a/lib.rs
            +++ b/lib.rs
            @@ -1,3 +1,3 @@
             eins
            -alt
            +zwei
             drei
        """.trimIndent()

        val undone = invertDiff(change)

        assertEquals(
            """
            --- b/lib.rs
            +++ a/lib.rs
            @@ -1,3 +1,3 @@
             eins
            +alt
            -zwei
             drei
            """.trimIndent(),
            undone,
        )
        // Twice is the change again.
        assertEquals(change, invertDiff(undone))
    }

    @Test
    fun `hunk ranges swap and lines that look like headers are content`() {
        val change = """
            --- a/x.txt
            +++ b/x.txt
            @@ -3,2 +3,4 @@ fn main() {
             a
            +-- kein Kopf
            +++ auch keiner
             b
        """.trimIndent()

        val undone = invertDiff(change).lines()

        assertEquals("@@ -3,4 +3,2 @@ fn main() {", undone[2])
        // Content that looks like a header is content: +-- becomes ---, +++ becomes -++.
        assertEquals("--- kein Kopf", undone[4])
        assertEquals("-++ auch keiner", undone[5])
        assertEquals("", invertDiff(""))
    }
}
