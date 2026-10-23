package de.fadi.nemotron.data

private val HUNK = Regex("""^@@ -(\S+) \+(\S+) @@(.*)$""")

/**
 * A unified diff run backwards: what the diff added becomes removed and the
 * other way round. It shows what an undo did, from the diff of the change.
 * The two file header lines swap their names, hunk ranges swap, `+` and `-`
 * lines swap; context lines stay.
 */
fun invertDiff(diff: String): String {
    val lines = diff.lines()
    val out = ArrayList<String>(lines.size)
    var inHunks = false
    var index = 0
    while (index < lines.size) {
        val line = lines[index]
        val next = lines.getOrNull(index + 1)
        when {
            // The two header lines come before the first hunk.
            !inHunks && line.startsWith("--- ") && next != null && next.startsWith("+++ ") -> {
                out += "--- " + next.removePrefix("+++ ")
                out += "+++ " + line.removePrefix("--- ")
                index++
            }
            line.startsWith("@@") -> {
                inHunks = true
                val match = HUNK.matchEntire(line)
                out += if (match != null) {
                    val (old, new, rest) = match.destructured
                    "@@ -$new +$old @@$rest"
                } else {
                    line
                }
            }
            inHunks && line.startsWith("+") -> out += "-" + line.substring(1)
            inHunks && line.startsWith("-") -> out += "+" + line.substring(1)
            else -> out += line
        }
        index++
    }
    return out.joinToString("\n")
}
