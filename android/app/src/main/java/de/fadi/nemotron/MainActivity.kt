package de.fadi.nemotron

import android.graphics.Color
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import de.fadi.nemotron.ui.AppScreen
import de.fadi.nemotron.ui.Tab
import de.fadi.nemotron.ui.theme.FadisTheme
import de.fadi.nemotron.ui.theme.usesDarkTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        // The default style asks the system for dark mode, which Android 8 and 9 do not have.
        val barStyle = SystemBarStyle.auto(Color.TRANSPARENT, Color.TRANSPARENT, ::usesDarkTheme)
        enableEdgeToEdge(barStyle, barStyle)
        super.onCreate(savedInstanceState)
        val prefs = (application as FadisApplication).prefs
        val initialTab = Tab.entries.find { it.name == prefs.lastTab } ?: Tab.Mac
        setContent {
            FadisTheme(dark = usesDarkTheme(resources)) {
                AppScreen(initialTab, onTabChange = { prefs.lastTab = it.name })
            }
        }
    }
}
