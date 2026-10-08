package dev.uniray.app

import android.app.ActivityManager
import android.content.Context
import java.io.File
import org.json.JSONObject

/**
 * What the VPN service reports to the app across processes.
 *
 * The service runs in its own process (`:vpn`), so that the tunnel survives
 * the app's UI being swept away. The two share files, not memory: the service
 * writes its state here after every start and stop, tagged with the request
 * it answers, and the app polls it.
 */
object VpnState {
  class State(val running: Boolean, val request: String?, val error: String?)

  private fun file(context: Context) = File(context.filesDir, "run/vpn-state.json")

  fun write(context: Context, running: Boolean, request: String?, error: String?) {
    val target = file(context)
    target.parentFile?.mkdirs()
    val json = JSONObject().put("running", running).put("request", request ?: JSONObject.NULL).put("error", error ?: JSONObject.NULL)
    val tmp = File(target.parentFile, target.name + ".tmp")
    tmp.writeText(json.toString())
    // Atomic, so the app never reads half a file.
    tmp.renameTo(target)
  }

  fun read(context: Context): State {
    return try {
      val json = JSONObject(file(context).readText())
      State(
        json.optBoolean("running"),
        json.optString("request").takeUnless { json.isNull("request") },
        json.optString("error").takeUnless { json.isNull("error") },
      )
    } catch (e: Exception) {
      State(false, null, null)
    }
  }

  /**
   * Whether a tunnel service process exists; it may have been killed without a
   * word. Either the VPN service or the stealth service counts - they are the
   * two ways the tunnel runs and never run at once.
   */
  fun serviceAlive(context: Context): Boolean {
    val manager = context.getSystemService(ActivityManager::class.java) ?: return false
    val tunnels = setOf(UniRayVpnService::class.java.name, StealthService::class.java.name)
    // Deprecated for third-party services, still answers for the app's own.
    @Suppress("DEPRECATION")
    return manager.getRunningServices(Int.MAX_VALUE).any { it.service.className in tunnels && it.started }
  }

  fun isRunning(context: Context): Boolean = read(context).running && serviceAlive(context)

  /** The tunnel services currently started; normally one at most. */
  fun aliveTunnels(context: Context): List<Class<out android.app.Service>> {
    val manager = context.getSystemService(ActivityManager::class.java) ?: return emptyList()
    @Suppress("DEPRECATION")
    val running = manager.getRunningServices(Int.MAX_VALUE).filter { it.started }.map { it.service.className }.toSet()
    // The stealth service first: when both are somehow up, it is the one
    // that matters (the VPN service carries nothing without its interface).
    return listOf(StealthService::class.java, UniRayVpnService::class.java).filter { it.name in running }
  }

  /** Which tunnel service is currently running, if any. */
  fun aliveTunnel(context: Context): Class<out android.app.Service>? = aliveTunnels(context).firstOrNull()
}
