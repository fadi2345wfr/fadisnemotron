package de.fadi.nemotron.ui.common

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import de.fadi.nemotron.ui.theme.FadisTheme

/**
 * Monospace text in a box that scrolls sideways. It never scrolls vertically,
 * so it can sit inside the chat list without fighting its scrolling.
 */
@Composable
fun CodeBox(text: AnnotatedString, modifier: Modifier = Modifier) {
    Surface(
        color = MaterialTheme.colorScheme.surfaceContainerHighest,
        shape = RoundedCornerShape(8.dp),
        modifier = modifier.fillMaxWidth(),
    ) {
        Text(
            text = text,
            fontFamily = FontFamily.Monospace,
            fontSize = 12.5.sp,
            lineHeight = 17.sp,
            softWrap = false,
            modifier = Modifier
                .horizontalScroll(rememberScrollState())
                .padding(10.dp),
        )
    }
}

@Composable
fun CodeBox(text: String, modifier: Modifier = Modifier) = CodeBox(AnnotatedString(text), modifier)

/** A unified diff with added lines green and removed lines red. */
@Composable
fun DiffBox(diff: String, modifier: Modifier = Modifier) {
    val status = FadisTheme.status
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    val hunk = MaterialTheme.colorScheme.primary
    val text = remember(diff, status, muted, hunk) {
        buildAnnotatedString {
            diff.trimEnd('\n').lines().forEachIndexed { index, line ->
                if (index > 0) append('\n')
                val color = when {
                    line.startsWith("+++") || line.startsWith("---") -> muted
                    line.startsWith("@@") -> hunk
                    line.startsWith("+") -> status.added
                    line.startsWith("-") -> status.removed
                    else -> Color.Unspecified
                }
                withStyle(SpanStyle(color = color)) { append(line) }
            }
        }
    }
    CodeBox(text, modifier)
}
