package dev.uniray.app

import android.Manifest
import android.app.Activity
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.io.File
import java.util.UUID
import kotlin.concurrent.thread
import org.json.JSONArray
import org.json.JSONObject
import rikka.shizuku.Shizuku

@InvokeArg
class StealthArgs {
  var config: String = ""
  /** "tproxy" (iptables) or "roottun" (a root-owned TUN). */
  var mode: String = "tproxy"
  var port: Int = 12345
  /** Interface the root TUN mode creates. */
  var tunName: String = ""
  var ipv6: Boolean = false
  var serverName: String = ""
  var serverId: String = ""
  /** Hold a wake lock while connected (the "stay awake" setting). */
  var keepAwake: Boolean = false
  var metricsSocket: String = ""
  var proxyTags: Array<String> = arrayOf()
  var engineMode: String = ""
  var engineConfig: String = ""
  var enginePort: Int = 0
}

/**
 * Stealth mode, in the full (GitHub) edition: the tunnel as a root process
 * ([StealthService]) instead of a VpnService, through libsu or Shizuku
 * ([PrivilegedShell]). The Google Play edition builds src/play's Stealth
 * instead, which has none of it: Play allows a device-wide tunnel only
 * through VpnService.
 */
object Stealth {
  /** The stealth tunnel's service; null in the Google Play edition. */
  val service: Class<out Service>? = StealthService::class.java

  const val ACTION_STOP = StealthService.ACTION_STOP
  const val ACTION_NOTIFY = StealthService.ACTION_NOTIFY
  const val ACTION_KEEP_AWAKE = StealthService.ACTION_KEEP_AWAKE

  /** The spec the stealth tunnel was last started from (the tile reuses it). */
  fun specFile(context: Context): File? = StealthService.specFile(context)

  /** Starts the stealth tunnel from its saved spec (the Quick Settings tile). */
  fun startIntent(context: Context, request: String): Intent? =
    Intent(context, StealthService::class.java)
      .setAction(StealthService.ACTION_START)
      .putExtra(StealthService.EXTRA_REQUEST, request)

  private fun privilegeToken(level: Privilege) =
    when (level) {
      Privilege.NONE -> "none"
      Privilege.SHIZUKU_SHELL -> "shizukuShell"
      Privilege.SHIZUKU_ROOT -> "shizukuRoot"
      Privilege.ROOT -> "root"
    }

  /** The elevated access the device offers, for the UI. Never prompts. */
  fun privilegeState(activity: Activity, invoke: Invoke) {
    thread {
      val state = PrivilegedShell.detect(activity)
      invoke.resolve(
        JSObject()
          .put("level", privilegeToken(state.level))
          .put("shizukuAvailable", state.shizukuAvailable)
          .put("shizukuGranted", state.shizukuGranted)
          .put("hasTproxy", state.hasTproxy)
      )
    }
  }

  /** Asks Shizuku for permission (interactive); resolves the new state once answered. */
  fun requestShizuku(activity: Activity, invoke: Invoke) {
    try {
      if (Shizuku.pingBinder() && !Shizuku.isPreV11()) {
        if (Shizuku.checkSelfPermission() == PackageManager.PERMISSION_GRANTED) {
          privilegeState(activity, invoke)
          return
        }
        val responded = java.util.concurrent.atomic.AtomicBoolean(false)
        val listener = object : Shizuku.OnRequestPermissionResultListener {
          override fun onRequestPermissionResult(requestCode: Int, grantResult: Int) {
            if (requestCode == PrivilegedShell.SHIZUKU_PERMISSION_REQUEST) {
              if (responded.compareAndSet(false, true)) {
                try {
                  Shizuku.removeRequestPermissionResultListener(this)
                } catch (e: Exception) {
                  // Ignore
                }
                privilegeState(activity, invoke)
              }
            }
          }
        }
        Shizuku.addRequestPermissionResultListener(listener)
        thread {
          Thread.sleep(30_000L)
          if (responded.compareAndSet(false, true)) {
            try {
              Shizuku.removeRequestPermissionResultListener(listener)
            } catch (e: Exception) {
              // Ignore
            }
            privilegeState(activity, invoke)
          }
        }
        Shizuku.requestPermission(PrivilegedShell.SHIZUKU_PERMISSION_REQUEST)
        return
      }
    } catch (e: Exception) {
      // Nothing to request against; the state call below just reports "none".
    }
    privilegeState(activity, invoke)
  }

  /**
   * Starts the stealth tunnel. Verifies root (prompting for it if need be);
   * rejects with code `noRoot` so the caller can fall back to the VPN service.
   */
  fun start(plugin: VpnPlugin, activity: Activity, invoke: Invoke) {
    val args = invoke.parseArgs(StealthArgs::class.java)
    thread {
      var state = PrivilegedShell.detect(activity)
      if (!state.level.canStealth) {
        // The grant dialog may be what is missing; ask once.
        PrivilegedShell.requestRoot()
        state = PrivilegedShell.detect(activity)
      }
      if (!state.level.canStealth) {
        invoke.reject("stealth needs root, which this device does not grant", "noRoot")
        return@thread
      }

      if (
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
          !activity.isFinishing && !activity.isDestroyed &&
          ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
      ) {
        ActivityCompat.requestPermissions(activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
      }

      val spec =
        JSONObject()
          .put("config", args.config)
          .put("mode", args.mode)
          .put("port", args.port)
          .put("tunName", args.tunName)
          .put("ipv6", args.ipv6)
          .put("privilege", state.level.name)
          .put("serverName", args.serverName)
          .put("serverId", args.serverId)
          .put("keepAwake", args.keepAwake)
          .put("metricsSocket", args.metricsSocket)
          .put("proxyTags", JSONArray(args.proxyTags))
          .put("engineMode", args.engineMode)
          .put("engineConfig", args.engineConfig)
          .put("enginePort", args.enginePort)
      val file = StealthService.specFile(activity)
      file.parentFile?.mkdirs()
      file.writeText(spec.toString())

      val request = UUID.randomUUID().toString()
      try {
        ContextCompat.startForegroundService(activity, startIntent(activity, request)!!)
      } catch (e: Exception) {
        invoke.reject("cannot start the stealth service: ${e.message}")
        return@thread
      }
      val startTime = System.currentTimeMillis()
      val answer = plugin.awaitAnswer(request, START_TIMEOUT_MS) { state ->
        System.currentTimeMillis() - startTime > 4000L && !state.running && !VpnState.serviceAlive(activity)
      }
      when {
        answer == null -> invoke.reject("the stealth service did not answer")
        answer.running -> {
          if (!KeepAwake.batteryExempt(activity) && !activity.isFinishing && !activity.isDestroyed) {
            activity.runOnUiThread { KeepAwake.requestBatteryExemption(activity) }
          }
          invoke.resolve()
        }
        else -> invoke.reject(answer.error ?: "the stealth service failed to start")
      }
    }
  }
}
