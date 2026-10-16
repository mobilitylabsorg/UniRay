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

  /**
   * Whether the tunnel's process exists; it may have been killed without a
   * word. Both tunnel services (the VPN service and the stealth service) run
   * in the `:vpn` process. Independent of the state file on purpose: while a
   * service starts, the file still holds the previous run's `running=false`,
   * and the start must not be taken for a failure then.
   */
  fun serviceAlive(context: Context): Boolean {
    val manager = context.getSystemService(ActivityManager::class.java) ?: return true
    val vpnProcess = "${context.packageName}:vpn"
    return try {
      // Lists this app's own processes; null when there are none to list.
      manager.runningAppProcesses?.any { it.processName == vpnProcess } ?: aliveTunnels(context).isNotEmpty()
    } catch (e: Exception) {
      // Cannot tell: rather wait for the service's answer than give up on it.
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
    return listOfNotNull(Stealth.service, UniRayVpnService::class.java).filter { it.name in running }
  }

  /**
   * The services a stop must reach: those started, or, when the system lists
   * none although the tunnel's process is up (some ROMs answer
   * getRunningServices with nothing), both. Nothing when the process is gone:
   * starting the stealth service only to stop it would run its privileged
   * teardown for a tunnel that is not there.
   */
  fun tunnelsToStop(context: Context): List<Class<out android.app.Service>> {
    val started = aliveTunnels(context)
    if (started.isNotEmpty()) return started
    return if (serviceAlive(context)) listOfNotNull(Stealth.service, UniRayVpnService::class.java) else emptyList()
  }

  /** Which tunnel service is currently running, if any. */
  fun aliveTunnel(context: Context): Class<out android.app.Service>? = aliveTunnels(context).firstOrNull()
}
