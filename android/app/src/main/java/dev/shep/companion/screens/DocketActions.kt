package dev.shep.companion.screens

import dev.shep.companion.BridgeClient
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * The docket's mutating verbs, in one place.
 *
 * Two screens now dispose of the same rows — the docket tab, where you work
 * the whole list, and the board's proposals region, where the overseer's
 * suggestions get kept or dropped without leaving the board. They have to
 * behave identically: the call goes off the main thread, the `{item}` that
 * comes back is handed to the caller so the row can move lane before the next
 * poll lands, and a failure is *shown* rather than swallowed — a proposal that
 * silently stayed put would read as a tap that missed.
 *
 * The success and failure callbacks are separate because the two screens do
 * different things with them: the docket tab closes its sheet on success and
 * deliberately leaves it open on failure, so the typed date is still there to
 * try again with.
 */
class DocketActions(
    private val client: BridgeClient,
    private val scope: CoroutineScope,
) {
    fun mutate(
        method: String,
        params: JSONObject,
        label: String,
        onItem: (JSONObject) -> Unit = {},
        onSuccess: (String) -> Unit,
        onFailure: (String) -> Unit,
    ) {
        scope.launch {
            withContext(Dispatchers.IO) { runCatching { client.call(method, params) } }
                .onSuccess { result ->
                    result.optJSONObject("item")?.let(onItem)
                    onSuccess(label)
                }
                .onFailure { onFailure("$method failed: ${it.message}") }
        }
    }

    /** Keep a proposal: promote it out of the inbox onto the slated lane. */
    fun keep(
        id: Long,
        onItem: (JSONObject) -> Unit = {},
        onSuccess: (String) -> Unit,
        onFailure: (String) -> Unit,
    ) = mutate(
        "docket.promote",
        JSONObject().put("id", id).put("kind", "slated"),
        "#$id slated",
        onItem,
        onSuccess,
        onFailure,
    )

    /** Drop a proposal. The overseer proposes; you dispose. */
    fun drop(
        id: Long,
        onItem: (JSONObject) -> Unit = {},
        onSuccess: (String) -> Unit,
        onFailure: (String) -> Unit,
    ) = mutate(
        "docket.discard",
        JSONObject().put("id", id),
        "#$id discarded",
        onItem,
        onSuccess,
        onFailure,
    )
}
