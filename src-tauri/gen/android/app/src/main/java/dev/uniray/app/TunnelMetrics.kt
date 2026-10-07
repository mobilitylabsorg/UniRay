package dev.uniray.app

import android.net.LocalSocket
import android.net.LocalSocketAddress
import org.json.JSONObject

/**
 * Reads the Xray core's traffic counters, shared by the VPN and stealth
 * services. They are served over a Unix socket in the app's private directory
 * (not a port), so no other app can find or read them.
 */
object TunnelMetrics {
  /** Total up/down bytes across [proxyTags], or null when the socket cannot be
   * read (the core is not up yet, or gone). */
  fun readTraffic(socketPath: String, proxyTags: List<String>): Pair<Long, Long>? {
    if (socketPath.isEmpty()) return null
    val body =
      try {
        LocalSocket().use { socket ->
          socket.connect(LocalSocketAddress(socketPath, LocalSocketAddress.Namespace.FILESYSTEM))
          socket.soTimeout = 2000
          socket.outputStream.write("GET /debug/vars HTTP/1.0\r\nHost: localhost\r\n\r\n".toByteArray())
          socket.inputStream.readBytes().toString(Charsets.UTF_8)
        }
      } catch (e: Exception) {
        return null
      }
    val json = body.substringAfter("\r\n\r\n", "")
    val outbound =
      try {
        JSONObject(json).optJSONObject("stats")?.optJSONObject("outbound") ?: return 0L to 0L
      } catch (e: Exception) {
        return null
      }
    var up = 0L
    var down = 0L
    for (tag in proxyTags) {
      val counters = outbound.optJSONObject(tag) ?: continue
      up += counters.optLong("uplink")
      down += counters.optLong("downlink")
    }
    return up to down
  }
}
