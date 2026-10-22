package de.fadi.nemotron.ui.theme

import android.content.res.Configuration
import android.content.res.Resources
import android.os.Build
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

val NvidiaGreen = Color(0xFF76B900)

/** Colours Material 3 has no slot for: diff lines and warnings. */
@Immutable
data class StatusColors(val added: Color, val removed: Color, val warning: Color)

private val DarkStatus = StatusColors(added = Color(0xFF7EE787), removed = Color(0xFFFF7B72), warning = Color(0xFFFFA657))
private val LightStatus = StatusColors(added = Color(0xFF1A7F37), removed = Color(0xFFCF222E), warning = Color(0xFFB35900))

private val LocalStatusColors = staticCompositionLocalOf { DarkStatus }

private val DarkColors = darkColorScheme(
    primary = NvidiaGreen,
    onPrimary = Color.Black,
    primaryContainer = Color(0xFF2E4A00),
    onPrimaryContainer = Color(0xFFC6F18A),
    secondary = Color(0xFFBFCBAD),
    onSecondary = Color(0xFF2A331E),
    secondaryContainer = Color(0xFF3F4A33),
    onSecondaryContainer = Color(0xFFDBE7C8),
    background = Color(0xFF111410),
    onBackground = Color(0xFFE1E4D9),
    surface = Color(0xFF111410),
    onSurface = Color(0xFFE1E4D9),
    surfaceVariant = Color(0xFF44483D),
    onSurfaceVariant = Color(0xFFC4C8BA),
    surfaceContainerLowest = Color(0xFF0C0F0A),
    surfaceContainerLow = Color(0xFF191C17),
    surfaceContainer = Color(0xFF1D201B),
    surfaceContainerHigh = Color(0xFF272B25),
    surfaceContainerHighest = Color(0xFF32362F),
    outline = Color(0xFF8E9286),
    outlineVariant = Color(0xFF44483D),
)

private val LightColors = lightColorScheme(
    // The NVIDIA green itself is too light for text on white; this is the same hue, darker.
    primary = Color(0xFF4A7C00),
    onPrimary = Color.White,
    primaryContainer = Color(0xFFC6F18A),
    onPrimaryContainer = Color(0xFF142000),
    secondary = Color(0xFF586249),
    onSecondary = Color.White,
    secondaryContainer = Color(0xFFDCE7C8),
    onSecondaryContainer = Color(0xFF161E0B),
    background = Color(0xFFF9FAF0),
    onBackground = Color(0xFF1A1C17),
    surface = Color(0xFFF9FAF0),
    onSurface = Color(0xFF1A1C17),
    surfaceVariant = Color(0xFFE1E4D5),
    onSurfaceVariant = Color(0xFF44483D),
    surfaceContainerLowest = Color.White,
    surfaceContainerLow = Color(0xFFF3F5EA),
    surfaceContainer = Color(0xFFEDEFE4),
    surfaceContainerHigh = Color(0xFFE7E9DF),
    surfaceContainerHighest = Color(0xFFE2E3D9),
    outline = Color(0xFF75796C),
    outlineVariant = Color(0xFFC5C8BA),
)

/**
 * Dark unless the system says otherwise. Android 8 and 9 have no system-wide
 * dark mode, so they always get the dark theme.
 */
fun usesDarkTheme(resources: Resources): Boolean =
    Build.VERSION.SDK_INT < Build.VERSION_CODES.Q ||
        resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES

@Composable
fun FadisTheme(dark: Boolean, content: @Composable () -> Unit) {
    CompositionLocalProvider(LocalStatusColors provides if (dark) DarkStatus else LightStatus) {
        MaterialTheme(colorScheme = if (dark) DarkColors else LightColors, content = content)
    }
}

object FadisTheme {
    val status: StatusColors
        @Composable @ReadOnlyComposable
        get() = LocalStatusColors.current
}
