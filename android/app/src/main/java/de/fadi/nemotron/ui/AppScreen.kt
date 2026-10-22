package de.fadi.nemotron.ui

import androidx.annotation.DrawableRes
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Icon
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.lifecycle.viewmodel.compose.viewModel
import de.fadi.nemotron.R
import de.fadi.nemotron.ui.direct.DirectScreen
import de.fadi.nemotron.ui.direct.DirectViewModel
import de.fadi.nemotron.ui.mac.MacScreen
import de.fadi.nemotron.ui.mac.MacViewModel

enum class Tab(@StringRes val label: Int, @DrawableRes val icon: Int) {
    Mac(R.string.tab_mac, R.drawable.ic_laptop),
    Direct(R.string.tab_direct, R.drawable.ic_cloud),
}

/** The two modes behind a bottom navigation bar. */
@Composable
fun AppScreen(initialTab: Tab, onTabChange: (Tab) -> Unit) {
    var tab by rememberSaveable { mutableStateOf(initialTab) }
    // Both live as long as the activity, so switching tabs keeps chats and the pairing.
    val mac: MacViewModel = viewModel(factory = MacViewModel.Factory)
    val direct: DirectViewModel = viewModel(factory = DirectViewModel.Factory)

    Scaffold(
        bottomBar = {
            NavigationBar {
                for (entry in Tab.entries) {
                    NavigationBarItem(
                        selected = tab == entry,
                        onClick = {
                            tab = entry
                            onTabChange(entry)
                        },
                        icon = { Icon(painterResource(entry.icon), contentDescription = null) },
                        label = { Text(stringResource(entry.label)) },
                    )
                }
            }
        },
        contentWindowInsets = WindowInsets(0),
    ) { padding ->
        Box(Modifier.padding(padding).consumeWindowInsets(padding)) {
            when (tab) {
                Tab.Mac -> MacScreen(mac)
                Tab.Direct -> DirectScreen(direct)
            }
        }
    }
}
