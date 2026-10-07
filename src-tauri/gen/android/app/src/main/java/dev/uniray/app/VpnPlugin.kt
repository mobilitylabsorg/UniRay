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
import java.util.UUID
import kotlin.concurrent.thread
import org.json.JSONArray
import org.json.JSONObject
import rikka.shizuku.Shizuku

/** How long to wait for the service to answer a start or a stop. */
private const val START_TIMEOUT_MS = 60_000L
private const val STOP_TIMEOUT_MS = 15_000L

@InvokeArg
class StartArgs {
  var config: String = ""
  var perAppMode: String = "off"
  var packages: Array<String> = arrayOf()
  var dns: String = "8.8.8.8:53"
  var mtu: Int = 1500
  var sessionName: String = "VPN"
  var serverName: String = ""
  var metricsSocket: String = ""
  var proxyTags: Array<String> = arrayOf()
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
  var metricsSocket: String = ""
  var proxyTags: Array<String> = arrayOf()
}

/**
 * The native half of the Rust `core::mobile` module: starts and stops
 * [UniRayVpnService] and answers what only Android knows.
 *
 * The service lives in another process (`:vpn`): requests go out as intents
 * carrying a request id, and answers come back through [VpnState].
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
        .put("serverName", args.serverName)
        .put("metricsSocket", args.metricsSocket)
        .put("proxyTags", JSONArray(args.proxyTags))
    val file = UniRayVpnService.specFile(activity)
    file.parentFile?.mkdirs()
    file.writeText(spec.toString())

    val request = UUID.randomUUID().toString()
    val intent =
      Intent(activity, UniRayVpnService::class.java)
        .setAction(UniRayVpnService.ACTION_START)
        .putExtra(UniRayVpnService.EXTRA_REQUEST, request)
    try {
      ContextCompat.startForegroundService(activity, intent)
    } catch (e: Exception) {
      invoke.reject("cannot start the VPN service: ${e.message}")
      return
    }
    thread {
      val answer = awaitAnswer(request, START_TIMEOUT_MS)
      when {
        answer == null -> invoke.reject("the VPN service did not answer")
        answer.running -> invoke.resolve()
        else -> invoke.reject(answer.error ?: "the VPN service failed to start")
      }
    }
  }

  @Command
  fun stop(invoke: Invoke) {
    // Either tunnel service may be the one running; stop whichever it is.
    val stopIntent =
      when (VpnState.aliveTunnel(activity)) {
        UniRayVpnService::class.java ->
          Intent(activity, UniRayVpnService::class.java).setAction(UniRayVpnService.ACTION_STOP)
        StealthService::class.java ->
          Intent(activity, StealthService::class.java).setAction(StealthService.ACTION_STOP)
        else -> null
      }
    if (stopIntent == null) {
      invoke.resolve()
      return
    }
    val request = UUID.randomUUID().toString()
    try {
      activity.startService(stopIntent.putExtra(UniRayVpnService.EXTRA_REQUEST, request))
    } catch (e: Exception) {
      invoke.reject("cannot reach the VPN service: ${e.message}")
      return
    }
    // Resolves either way: a service that does not answer is gone or going.
    // What it could not tear down comes back as `error`.
    thread {
      val answer = awaitAnswer(request, STOP_TIMEOUT_MS) { !it.running && !VpnState.serviceAlive(activity) }
      invoke.resolve(JSObject().put("error", answer?.error))
    }
  }

  /**
   * The service's answer to [request], or null when it does not come in time.
   * [settled] accepts a state that answers the request by other means (the
   * service is simply gone).
   */
  private fun awaitAnswer(
    request: String,
    timeoutMs: Long,
    settled: (VpnState.State) -> Boolean = { false },
  ): VpnState.State? {
    val deadline = System.currentTimeMillis() + timeoutMs
    while (System.currentTimeMillis() < deadline) {
      val state = VpnState.read(activity)
      if (state.request == request || settled(state)) return state
      Thread.sleep(50)
    }
    return null
  }

  @Command
  fun updateNotification(invoke: Invoke) {
    val args = invoke.parseArgs(NotificationArgs::class.java)
    if (VpnState.isRunning(activity)) {
      val intent =
        Intent(activity, UniRayVpnService::class.java)
          .setAction(UniRayVpnService.ACTION_NOTIFY)
          .putExtra(UniRayVpnService.EXTRA_SERVER, args.server)
      args.ok?.let { intent.putExtra(UniRayVpnService.EXTRA_OK, it) }
      args.latencyMs?.let { intent.putExtra(UniRayVpnService.EXTRA_LATENCY, it) }
      try {
        activity.startService(intent)
      } catch (e: Exception) {
        // The service keeps its own notification current anyway.
      }
    }
    invoke.resolve()
  }

  @Command
  fun state(invoke: Invoke) {
    val state = VpnState.read(activity)
    // `reported`: the service wrote its own stop (asked to, or with `error`
    // saying why); otherwise a tunnel that is not running was killed.
    invoke.resolve(
      JSObject()
        .put("running", state.running && VpnState.serviceAlive(activity))
        .put("reported", !state.running)
        .put("error", state.error)
    )
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
    // Probes from this process would go through the tunnel.
    if (VpnState.isRunning(activity)) {
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

  private fun privilegeToken(level: Privilege) =
    when (level) {
      Privilege.NONE -> "none"
      Privilege.SHIZUKU_SHELL -> "shizukuShell"
      Privilege.SHIZUKU_ROOT -> "shizukuRoot"
      Privilege.ROOT -> "root"
    }

  /** The elevated access the device offers, for the UI. Never prompts. */
  @Command
  fun privilegeState(invoke: Invoke) {
    thread {
      val state = PrivilegedShell.detect(activity)
      invoke.resolve(
        JSObject()
          .put("level", privilegeToken(state.level))
          .put("shizukuAvailable", state.shizukuAvailable)
          .put("shizukuGranted", state.shizukuGranted)
      )
    }
  }

  /** Asks Shizuku for permission (interactive); resolves the new state. */
  @Command
  fun requestShizuku(invoke: Invoke) {
    try {
      if (Shizuku.pingBinder() && !Shizuku.isPreV11()) {
        Shizuku.requestPermission(PrivilegedShell.SHIZUKU_PERMISSION_REQUEST)
      }
    } catch (e: Exception) {
      // Nothing to request against; the state call below just reports "none".
    }
    privilegeState(invoke)
  }

  /**
   * Starts the stealth tunnel. Verifies root (prompting for it if need be);
   * rejects with code `noRoot` so the caller can fall back to the VPN service.
   */
  @Command
  fun startStealth(invoke: Invoke) {
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

      val spec =
        JSONObject()
          .put("config", args.config)
          .put("mode", args.mode)
          .put("port", args.port)
          .put("tunName", args.tunName)
          .put("ipv6", args.ipv6)
          .put("privilege", state.level.name)
          .put("serverName", args.serverName)
          .put("metricsSocket", args.metricsSocket)
          .put("proxyTags", JSONArray(args.proxyTags))
      val file = StealthService.specFile(activity)
      file.parentFile?.mkdirs()
      file.writeText(spec.toString())

      val request = UUID.randomUUID().toString()
      val intent =
        Intent(activity, StealthService::class.java)
          .setAction(StealthService.ACTION_START)
          .putExtra(StealthService.EXTRA_REQUEST, request)
      try {
        ContextCompat.startForegroundService(activity, intent)
      } catch (e: Exception) {
        invoke.reject("cannot start the stealth service: ${e.message}")
        return@thread
      }
      val answer = awaitAnswer(request, START_TIMEOUT_MS)
      when {
        answer == null -> invoke.reject("the stealth service did not answer")
        answer.running -> invoke.resolve()
        else -> invoke.reject(answer.error ?: "the stealth service failed to start")
      }
    }
  }
}
