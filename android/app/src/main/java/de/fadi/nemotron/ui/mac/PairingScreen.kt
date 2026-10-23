package de.fadi.nemotron.ui.mac

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import de.fadi.nemotron.R

/** Explains how to connect to the Mac and pairs by QR code or by hand. */
@Composable
fun PairingScreen(
    inProgress: Boolean,
    error: String?,
    onScan: () -> Unit,
    onConnect: (address: String, token: String) -> Unit,
) {
    var address by rememberSaveable { mutableStateOf("") }
    var token by rememberSaveable { mutableStateOf("") }

    Column(
        Modifier
            .fillMaxSize()
            .safeDrawingPadding()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 24.dp, vertical = 24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text(stringResource(R.string.pair_title), style = MaterialTheme.typography.headlineSmall)
        PairingStep(1, stringResource(R.string.pair_step_remote))
        PairingStep(2, stringResource(R.string.pair_step_scan))
        PairingStep(3, stringResource(R.string.pair_step_tailscale))

        Button(onClick = onScan, enabled = !inProgress, modifier = Modifier.fillMaxWidth()) {
            Icon(painterResource(R.drawable.ic_qr_code), contentDescription = null, modifier = Modifier.size(20.dp))
            Text(stringResource(R.string.scan_qr), Modifier.padding(start = 8.dp))
        }
        if (inProgress) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                Text(stringResource(R.string.connecting))
            }
        }
        if (error != null) {
            Text(error, color = MaterialTheme.colorScheme.error)
        }

        HorizontalDivider(Modifier.padding(vertical = 8.dp))
        Text(stringResource(R.string.pair_manual_title), style = MaterialTheme.typography.titleMedium)
        OutlinedTextField(
            value = address,
            onValueChange = { address = it },
            label = { Text(stringResource(R.string.pair_address)) },
            placeholder = { Text("http://192.168.1.20:7878") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Next),
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = token,
            onValueChange = { token = it },
            label = { Text(stringResource(R.string.pair_token)) },
            singleLine = true,
            textStyle = MaterialTheme.typography.bodyLarge.copy(fontFamily = FontFamily.Monospace),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedButton(
            onClick = { onConnect(address, token) },
            enabled = !inProgress && address.isNotBlank() && token.isNotBlank(),
            modifier = Modifier.align(Alignment.End),
        ) {
            Text(stringResource(R.string.connect))
        }
    }
}

@Composable
private fun PairingStep(number: Int, text: String) {
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Surface(shape = CircleShape, color = MaterialTheme.colorScheme.primary, modifier = Modifier.size(28.dp)) {
            Box(contentAlignment = Alignment.Center) {
                Text(number.toString(), color = MaterialTheme.colorScheme.onPrimary, fontWeight = FontWeight.Bold)
            }
        }
        Text(text, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.widthIn(max = 560.dp))
    }
}
