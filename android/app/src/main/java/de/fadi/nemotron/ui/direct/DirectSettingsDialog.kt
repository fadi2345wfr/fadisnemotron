package de.fadi.nemotron.ui.direct

import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.unit.dp
import de.fadi.nemotron.R
import de.fadi.nemotron.data.DirectSettings
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/** API key, server, model and sampling for the direct mode. */
@Composable
fun DirectSettingsDialog(settings: DirectSettings, onSave: (DirectSettings) -> Unit, onDismiss: () -> Unit) {
    var apiKey by rememberSaveable { mutableStateOf(settings.apiKey) }
    var showKey by rememberSaveable { mutableStateOf(false) }
    var baseUrl by rememberSaveable { mutableStateOf(settings.baseUrl) }
    var model by rememberSaveable { mutableStateOf(settings.model) }
    var thinking by rememberSaveable { mutableStateOf(settings.thinking) }
    var temperature by rememberSaveable { mutableStateOf(settings.temperature.toString()) }
    var topP by rememberSaveable { mutableStateOf(settings.topP.toString()) }
    var maxTokens by rememberSaveable { mutableStateOf(settings.maxTokens.toString()) }
    var systemPrompt by rememberSaveable { mutableStateOf(settings.systemPrompt) }

    val temperatureValue = decimal(temperature)?.takeIf { it in 0.0..2.0 }
    val topPValue = decimal(topP)?.takeIf { it > 0.0 && it <= 1.0 }
    val maxTokensValue = maxTokens.trim().toIntOrNull()?.takeIf { it > 0 }
    val urlValid = baseUrl.trim().toHttpUrlOrNull() != null
    val result = if (temperatureValue != null && topPValue != null && maxTokensValue != null && urlValid && model.isNotBlank()) {
        DirectSettings(
            apiKey = apiKey.trim(),
            baseUrl = baseUrl.trim().trimEnd('/'),
            model = model.trim(),
            thinking = thinking,
            temperature = temperatureValue,
            topP = topPValue,
            maxTokens = maxTokensValue,
            systemPrompt = systemPrompt.trim(),
        )
    } else {
        null
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.settings_title)) },
        text = {
            Column(
                Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                OutlinedTextField(
                    value = apiKey,
                    onValueChange = { apiKey = it },
                    label = { Text(stringResource(R.string.api_key)) },
                    singleLine = true,
                    visualTransformation = if (showKey) VisualTransformation.None else PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                    trailingIcon = {
                        IconButton(onClick = { showKey = !showKey }) {
                            Icon(
                                painterResource(if (showKey) R.drawable.ic_visibility_off else R.drawable.ic_visibility),
                                contentDescription = stringResource(if (showKey) R.string.hide_key else R.string.show_key),
                            )
                        }
                    },
                    modifier = Modifier.fillMaxWidth(),
                )
                KeyLink()
                OutlinedTextField(
                    value = baseUrl,
                    onValueChange = { baseUrl = it },
                    label = { Text(stringResource(R.string.base_url)) },
                    singleLine = true,
                    isError = !urlValid,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = model,
                    onValueChange = { model = it },
                    label = { Text(stringResource(R.string.model)) },
                    singleLine = true,
                    isError = model.isBlank(),
                    modifier = Modifier.fillMaxWidth(),
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(stringResource(R.string.thinking_mode))
                        Text(
                            stringResource(R.string.thinking_mode_hint),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    Switch(checked = thinking, onCheckedChange = { thinking = it })
                }
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    NumberField(temperature, { temperature = it }, R.string.temperature, temperatureValue == null, Modifier.weight(1f))
                    NumberField(topP, { topP = it }, R.string.top_p, topPValue == null, Modifier.weight(1f))
                }
                NumberField(
                    maxTokens,
                    { maxTokens = it },
                    R.string.max_tokens,
                    maxTokensValue == null,
                    Modifier.fillMaxWidth(),
                    keyboardType = KeyboardType.Number,
                )
                OutlinedTextField(
                    value = systemPrompt,
                    onValueChange = { systemPrompt = it },
                    label = { Text(stringResource(R.string.system_prompt)) },
                    minLines = 2,
                    maxLines = 6,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        },
        confirmButton = {
            TextButton(onClick = { result?.let(onSave) }, enabled = result != null) {
                Text(stringResource(R.string.save))
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

@Composable
private fun NumberField(
    value: String,
    onValueChange: (String) -> Unit,
    @StringRes label: Int,
    isError: Boolean,
    modifier: Modifier,
    keyboardType: KeyboardType = KeyboardType.Decimal,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onValueChange,
        label = { Text(stringResource(label)) },
        singleLine = true,
        isError = isError,
        keyboardOptions = KeyboardOptions(keyboardType = keyboardType),
        modifier = modifier,
    )
}

@Composable
private fun KeyLink() {
    val linkColor = MaterialTheme.colorScheme.primary
    val prefix = stringResource(R.string.api_key_link_prefix)
    Text(
        buildAnnotatedString {
            append(prefix)
            append(' ')
            withLink(
                LinkAnnotation.Url(
                    KEY_URL,
                    TextLinkStyles(SpanStyle(color = linkColor, textDecoration = TextDecoration.Underline)),
                ),
            ) { append(KEY_URL.removePrefix("https://")) }
        },
        style = MaterialTheme.typography.bodySmall,
    )
}

/** German keyboards type a decimal comma. */
private fun decimal(text: String): Double? = text.trim().replace(',', '.').toDoubleOrNull()

private const val KEY_URL = "https://build.nvidia.com/settings/api-keys"
