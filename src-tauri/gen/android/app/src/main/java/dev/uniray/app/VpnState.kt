package dev.uniray.app

import android.app.ActivityManager
import android.content.Context
import android.content.Intent
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
    if (!tmp.renameTo(target)) {
      target.delete()
      if (!tmp.renameTo(target)) {
        target.writeText(json.toString())
        tmp.delete()
      }
    }
    lastServiceAliveCheck = 0L
    try {
      val broadcast = Intent(UniRayTileService.ACTION_VPN_STATE_CHANGED)
        .setPackage(context.packageName)
        .putExtra("running", running)
      context.sendBroadcast(broadcast)
    } catch (_: Exception) {}
    UniRayTileService.requestUpdate(context)
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

  @Volatile private var lastServiceAliveCheck = 0L
  @Volatile private var cachedServiceAlive = false

  /**
   * Whether a tunnel service process exists; it may have been killed without a
   * word. Either the VPN service or the stealth service counts - they are the
   * two ways the tunnel runs and never run at once.
   */
  fun serviceAlive(context: Context): Boolean {
    val state = read(context)
    if (!state.running) return false

    val manager = context.getSystemService(ActivityManager::class.java) ?: return true
    val vpnProcess = "${context.packageName}:vpn"
    return try {
      val procs = manager.runningAppProcesses
      if (procs != null) {
        procs.any { it.processName == vpnProcess }
      } else {
        true
      }
    } catch (e: Exception) {
      true
    }
  }

  fun isRunning(context: Context): Boolean = read(context).running && serviceAlive(context)

  /** The tunnel services currently started; normally one at most. */
  fun aliveTunnels(context: Context): List<Class<out android.app.Service>> {
    val manager = context.getSystemService(ActivityManager::class.java) ?: return emptyList()
    @Suppress("DEPRECATION")
    val running =
      try {
        manager.getRunningServices(Int.MAX_VALUE)?.filter { it.started }?.map { it.service.className }?.toSet() ?: emptySet()
      } catch (e: Exception) {
        emptySet()
      }
    // The stealth service first: when both are somehow up, it is the one
    // that matters (the VPN service carries nothing without its interface).
    return listOf(StealthService::class.java, UniRayVpnService::class.java).filter { it.name in running }
  }

  /** Which tunnel service is currently running, if any. */
  fun aliveTunnel(context: Context): Class<out android.app.Service>? = aliveTunnels(context).firstOrNull()
}
