package de.fadi.nemotron.data

import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import okhttp3.Call
import okhttp3.Response

/**
 * Executes the call on the IO dispatcher and hands the response to [read].
 *
 * Cancelling the coroutine cancels the call. That also aborts a body that is
 * still being read, such as a long-poll or a token stream, which a plain
 * blocking `execute()` would not notice.
 */
internal suspend fun <T> Call.await(read: (Response) -> T): T = coroutineScope {
    val call = this@await
    val canceller = launch(start = CoroutineStart.UNDISPATCHED) {
        try {
            awaitCancellation()
        } finally {
            call.cancel()
        }
    }
    try {
        withContext(Dispatchers.IO) { call.execute().use(read) }
    } finally {
        canceller.cancel()
    }
}

/** HTTP headers take printable ASCII only; a pasted key with a stray character would make OkHttp throw. */
internal fun String.isHeaderSafe(): Boolean = all { it in ' '..'~' }
