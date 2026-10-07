package dev.uniray.app

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.net.VpnService
import android.os.Build
import android.provider.Settings
import androidx.activity.result.ActivityResult
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import kotlin.concurrent.thread
import org.json.JSONArray
import org.json.JSONObject

@InvokeArg
class StartArgs {
  var config: String = ""
  var perAppMode: String = "off"
  var packages: Array<String> = arrayOf()
  var dns: String = "8.8.8.8:53"
  var mtu: Int = 1500
  var sessionName: String = "VPN"
}

@InvokeArg
class NotificationArgs {
  var server: String = ""
  var down: Long = 0
  var up: Long = 0
  var latencyMs: Long? = null
  var ok: Boolean? = null
}

@InvokeArg
class DelayArgs {
  var configs: Array<String> = arrayOf()
  var url: String = ""
  var timeout: Int = 6
}

/**
 * The native half of the Rust `core::mobile` module: starts and stops
 * [UniRayVpnService] and answers what only Android knows.
 */
@TauriPlugin
class VpnPlugin(private val activity: Activity) : Plugin(activity) {
  private var pendingStart: StartArgs? = null

  @Command
  fun start(invoke: Invoke) {
    val args = invoke.parseArgs(StartArgs::class.java)
    // Null when the user has already agreed to this app being a VPN.
    val consent = VpnService.prepare(activity)
    if (consent != null) {
      pendingStart = args
      startActivityForResult(invoke, consent, "onVpnConsent")
    } else {
      launch(invoke, args)
    }
  }

  @ActivityCallback
  fun onVpnConsent(invoke: Invoke, result: ActivityResult) {
    val args = pendingStart
    pendingStart = null
    if (result.resultCode == Activity.RESULT_OK && args != null) {
      launch(invoke, args)
    } else {
      invoke.reject("the VPN permission was not granted", "vpnDenied")
    }
  }

  private fun launch(invoke: Invoke, args: StartArgs) {
    // Without it the service still runs, only its notification is hidden.
    if (
      Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
        ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) !=
          PackageManager.PERMISSION_GRANTED
    ) {
      ActivityCompat.requestPermissions(activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
    }

    val spec =
      JSONObject()
        .put("config", args.config)
        .put("perAppMode", args.perAppMode)
        .put("packages", JSONArray(args.packages))
        .put("dns", args.dns)
        .put("mtu", args.mtu)
        .put("sessionName", args.sessionName)
    val file = UniRayVpnService.specFile(activity)
    file.parentFile?.mkdirs()
    file.writeText(spec.toString())

    UniRayVpnService.onStarted = { error ->
      if (error == null) invoke.resolve() else invoke.reject(error)
    }
    val intent = Intent(activity, UniRayVpnService::class.java).setAction(UniRayVpnService.ACTION_START)
    try {
      ContextCompat.startForegroundService(activity, intent)
    } catch (e: Exception) {
      UniRayVpnService.onStarted = null
      invoke.reject("cannot start the VPN service: ${e.message}")
    }
  }

  @Command
  fun stop(invoke: Invoke) {
    if (!UniRayVpnService.running) {
      invoke.resolve()
      return
    }
    UniRayVpnService.onStopped = { invoke.resolve() }
    activity.startService(
      Intent(activity, UniRayVpnService::class.java).setAction(UniRayVpnService.ACTION_STOP)
    )
  }

  @Command
  fun updateNotification(invoke: Invoke) {
    val args = invoke.parseArgs(NotificationArgs::class.java)
    UniRayVpnService.instance?.updateNotification(args.server, args.down, args.up, args.latencyMs, args.ok)
    invoke.resolve()
  }

  @Command
  fun state(invoke: Invoke) {
    invoke.resolve(JSObject().put("running", UniRayVpnService.running))
  }

  /** The Material You palette as UI tokens for both themes; empty before Android 12. */
  @Command
  fun systemColors(invoke: Invoke) {
    val light = SystemTheme.palette(activity, dark = false)
    val dark = SystemTheme.palette(activity, dark = true)
    if (light == null || dark == null) {
      invoke.resolve(JSObject())
      return
    }
    fun tokens(palette: Map<String, Int>) =
      JSObject().apply { for ((name, color) in palette) put(name, SystemTheme.hex(color)) }
    invoke.resolve(JSObject().put("light", tokens(light)).put("dark", tokens(dark)))
  }

  /** The real network under the VPN and its DNS servers. */
  @Command
  fun networkInfo(invoke: Invoke) {
    val info = UnderlyingNetwork.current(activity)
    invoke.resolve(JSObject().put("type", info.type).put("dns", JSArray(info.dns)))
  }

  @Command
  fun deviceInfo(invoke: Invoke) {
    val id = Settings.Secure.getString(activity.contentResolver, Settings.Secure.ANDROID_ID)
    invoke.resolve(
      JSObject()
        .put("deviceId", id ?: "")
        .put("model", "${Build.MANUFACTURER} ${Build.MODEL}")
        .put("osVersion", Build.VERSION.RELEASE ?: "")
    )
  }

  /** Launchable apps other than this one, for the per-app proxy list. */
  @Command
  fun listApps(invoke: Invoke) {
    thread {
      val manager = activity.packageManager
      val apps = JSArray()
      val installed =
        manager.getInstalledApplications(0).filter {
          it.packageName != activity.packageName &&
            manager.getLaunchIntentForPackage(it.packageName) != null
        }
      for (info in installed.sortedBy { manager.getApplicationLabel(it).toString().lowercase() }) {
        apps.put(
          JSObject()
            .put("package", info.packageName)
            .put("label", manager.getApplicationLabel(info).toString())
            .put("system", info.flags and ApplicationInfo.FLAG_SYSTEM != 0)
        )
      }
      invoke.resolve(JSObject().put("apps", apps))
    }
  }

  @Command
  fun measureDelays(invoke: Invoke) {
    val args = invoke.parseArgs(DelayArgs::class.java)
    if (UniRayVpnService.running) {
      invoke.reject("delays cannot be tested while connected", "busy")
      return
    }
    thread {
      try {
        val delays = XrayCore.pingBatch(args.configs.toList(), args.url, args.timeout)
        invoke.resolve(JSObject().put("delays", JSArray(delays)))
      } catch (e: Exception) {
        invoke.reject(e.message ?: e.toString())
      }
    }
  }
}
