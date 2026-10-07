package dev.uniray.app

import libXray.LibXray
import org.json.JSONArray
import org.json.JSONObject

/**
 * JSON wrapper around libXray's single `invoke` entry point.
 *
 * The core is process-wide: one instance runs at a time, and `pingBatch`
 * refuses to work while it does.
 */
object XrayCore {
  private const val API_VERSION = 3

  class Response(val success: Boolean, val data: JSONObject?, val error: String)

  private fun invoke(method: String, payload: JSONObject? = null): Response {
    val request = JSONObject().put("apiVersion", API_VERSION).put("method", method)
    if (payload != null) request.put("payload", payload)
    val response = JSONObject(LibXray.invoke(request.toString()))
    return Response(
      response.optBoolean("success"),
      response.optJSONObject("data"),
      response.optString("error")
    )
  }

  /** Starts the core with [config]; returns the error, or null on success. */
  fun run(config: String): String? {
    val response = invoke("runXray", JSONObject().put("xrayJson", config))
    return if (response.success) null else response.error.ifEmpty { "the core failed to start" }
  }

  fun stop() {
    invoke("stopXray")
  }

  /**
   * Delay of each config in milliseconds, -1 where the test failed. Throws
   * [IllegalStateException] when the whole batch is refused.
   */
  fun pingBatch(configs: List<String>, url: String, timeoutSeconds: Int): List<Long> {
    val items = JSONArray()
    for (config in configs) items.put(JSONObject().put("xrayJson", config))
    val payload = JSONObject().put("configs", items).put("url", url).put("timeout", timeoutSeconds)
    val response = invoke("pingBatch", payload)
    if (!response.success) throw IllegalStateException(response.error)
    // libXray's response types carry no JSON tags, so accept either casing.
    val results = response.data?.let { it.optJSONArray("results") ?: it.optJSONArray("Results") }
    return configs.indices.map { index ->
      val result = results?.optJSONObject(index)
      val ok = result != null && (result.optBoolean("success") || result.optBoolean("Success"))
      if (ok) result!!.optLong("delay", result.optLong("Delay", -1)) else -1L
    }
  }
}
