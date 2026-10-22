package de.fadi.nemotron.data

import kotlinx.serialization.Serializable
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.OkHttpClient

/** A paired Mac: the address that answered, its key and its name. */
data class Pairing(val url: String, val token: String, val name: String)

/** The content of the QR code the Mac app shows. */
@Serializable
data class PairingCode(
    val fadis: Int = 0,
    val name: String = "",
    val urls: List<String> = emptyList(),
    val token: String = "",
)

const val DEFAULT_PORT = 7878

/** Reads a scanned QR code; null if it is not a Fadis Nemotron pairing code. */
fun parsePairingCode(raw: String): PairingCode? {
    val code = try {
        remoteJson.decodeFromString<PairingCode>(raw.trim())
    } catch (e: IllegalArgumentException) {
        // Also covers SerializationException: not JSON, or not an object.
        return null
    }
    return code.takeIf { it.fadis >= 1 && it.token.isNotBlank() && it.urls.isNotEmpty() }
}

/**
 * Accepts what people type: `192.168.1.20`, `macbook:7878` or a full
 * `http://…` address. Without a port the remote-access default is used.
 */
fun normalizeAddress(input: String): String? {
    val trimmed = input.trim()
    if (trimmed.isEmpty()) return null
    val withScheme = if ("://" in trimmed) trimmed else "http://$trimmed"
    val url = withScheme.toHttpUrlOrNull() ?: return null
    val hasPort = withScheme.substringAfter("://").substringBefore('/').substringAfterLast(']').contains(':')
    return HttpUrl.Builder()
        .scheme(url.scheme)
        .host(url.host)
        .port(if (hasPort) url.port else DEFAULT_PORT)
        .build()
        .toString()
        .trimEnd('/')
}

/**
 * Tries the Mac's addresses in order and keeps the first that answers.
 * A wrong key or a Mac app that is too old stops at once, since every
 * address would give the same answer.
 */
suspend fun connect(code: PairingCode, http: OkHttpClient): Pairing {
    if (!code.token.isHeaderSafe()) throw RemoteException(null, "Der Schlüssel enthält ungültige Zeichen.")
    var lastError: RemoteException? = null
    for (url in code.urls.mapNotNull(::normalizeAddress)) {
        val ping = try {
            RemoteApi(url, code.token, http).ping()
        } catch (e: RemoteException) {
            if (e.status == 401) throw e
            lastError = e
            continue
        }
        if (!ping.supported) throw RemoteException(null, RemoteApi.MAC_TOO_OLD)
        return Pairing(url, code.token, ping.name.ifBlank { code.name.ifBlank { "Mac" } })
    }
    throw lastError ?: RemoteException(null, "Der QR-Code enthält keine gültige Adresse.")
}
