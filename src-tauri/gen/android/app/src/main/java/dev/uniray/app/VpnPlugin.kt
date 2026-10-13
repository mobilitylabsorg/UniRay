package dev.uniray.app

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.Context
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
  var mtu: Int = 1360
  /** What the VPN routes into itself, `address/length`: all but the engine's server. */
  var routes: Array<String> = arrayOf("0.0.0.0/0", "::/0")
  var sessionName: String = "VPN"
  var serverName: String = ""
  var serverId: String = ""
  /** Hold a wake lock while connected (the "stay awake" setting). */
  var keepAwake: Boolean = false
  var metricsSocket: String = ""
  var proxyTags: Array<String> = arrayOf()
  /** The engine for a WireGuard/AmneziaWG or Naive server; empty for the others. */
  var engineMode: String = ""
  var engineConfig: String = ""
  var enginePort: Int = 0
}

@InvokeArg
class KeepAwakeArgs {
  var enabled: Boolean = false
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
class ProbeArgs {
  var mode: String = ""
  var config: String = ""
  var url: String = ""
  var timeout: Int = 8
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
 * The native half of the Rust `core::mobile` module: starts and stops
 * [UniRayVpnService] and answers what only Android knows.
 *
 * The service lives in another process (`:vpn`): requests go out as intents
 * carrying a request id, and answers come back through [VpnState].
 */
/** Android TV / Google TV: a television UI mode, or the leanback feature. */
fun isTv(context: Context): Boolean {
  val ui = context.getSystemService(android.app.UiModeManager::class.java)
  return ui?.currentModeType == android.content.res.Configuration.UI_MODE_TYPE_TELEVISION ||
    context.packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)
}

@TauriPlugin
class VpnPlugin(private val activity: Activity) : Plugin(activity) {
  companion object {
    @Volatile
    private var pendingStart: StartArgs? = null
  }

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
        !activity.isFinishing && !activity.isDestroyed &&
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
        .put("routes", JSONArray(args.routes))
        .put("sessionName", args.sessionName)
        .put("serverName", args.serverName)
        .put("serverId", args.serverId)
        .put("keepAwake", args.keepAwake)
        .put("metricsSocket", args.metricsSocket)
        .put("proxyTags", JSONArray(args.proxyTags))
        .put("engineMode", args.engineMode)
        .put("engineConfig", args.engineConfig)
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
    val startTime = System.currentTimeMillis()
    thread {
      val answer = awaitAnswer(request, START_TIMEOUT_MS) { state ->
        System.currentTimeMillis() - startTime > 4000L && !state.running && !VpnState.serviceAlive(activity)
      }
      when {
        answer == null -> invoke.reject("the VPN service did not answer")
        answer.running -> {
          if (!KeepAwake.batteryExempt(activity) && !activity.isFinishing && !activity.isDestroyed) {
            activity.runOnUiThread { KeepAwake.requestBatteryExemption(activity) }
          }
          invoke.resolve()
        }
        else -> invoke.reject(answer.error ?: "the VPN service failed to start")
      }
    }
  }

  @Command
  fun stop(invoke: Invoke) {
    // Every tunnel service that is up, not just the first found: stopping
    // the wrong one leaves the other carrying the traffic.
    val targets = VpnState.tunnelsToStop(activity)
    if (targets.isEmpty()) {
      invoke.resolve()
      return
    }
    val stopIntents =
      targets.map { service ->
        when (service) {
          StealthService::class.java -> Intent(activity, service).setAction(StealthService.ACTION_STOP)
          else -> Intent(activity, service).setAction(UniRayVpnService.ACTION_STOP)
        }
      }
    val request = UUID.randomUUID().toString()
    try {
      for (intent in stopIntents) activity.startService(intent.putExtra(UniRayVpnService.EXTRA_REQUEST, request))
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
    // Only to the tunnel service that is running: startService would start
    // the other one, and an idle VPN service left standing took the next
    // disconnect for itself while the stealth tunnel kept running.
    val action =
      when (VpnState.aliveTunnel(activity)) {
        UniRayVpnService::class.java -> UniRayVpnService::class.java to UniRayVpnService.ACTION_NOTIFY
        StealthService::class.java -> StealthService::class.java to StealthService.ACTION_NOTIFY
        else -> null
      }
    if (action != null) {
      // Both services read the same extra names.
      val intent =
        Intent(activity, action.first)
          .setAction(action.second)
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

  /**
   * The "stay awake" setting changed: applies it to the running tunnel, and
   * to the saved specs the Quick Settings tile starts from. Turning it on
   * also asks for the battery optimization exemption (a system dialog).
   * Resolves whether the app is exempt now.
   */
  @Command
  fun setKeepAwake(invoke: Invoke) {
    val args = invoke.parseArgs(KeepAwakeArgs::class.java)
    for (spec in listOf(UniRayVpnService.specFile(activity), StealthService.specFile(activity))) {
      try {
        if (spec.exists()) spec.writeText(JSONObject(spec.readText()).put("keepAwake", args.enabled).toString())
      } catch (e: Exception) {
        // The next connection writes it afresh.
      }
    }
    val action =
      when (VpnState.aliveTunnel(activity)) {
        UniRayVpnService::class.java -> UniRayVpnService::class.java to UniRayVpnService.ACTION_KEEP_AWAKE
        StealthService::class.java -> StealthService::class.java to StealthService.ACTION_KEEP_AWAKE
        else -> null
      }
    if (action != null) {
      try {
        activity.startService(
          Intent(activity, action.first).setAction(action.second).putExtra(UniRayVpnService.EXTRA_ENABLED, args.enabled)
        )
      } catch (e: Exception) {
        // Applied on the next connection.
      }
    }
    if (args.enabled) activity.runOnUiThread { KeepAwake.requestBatteryExemption(activity) }
    invoke.resolve(JSObject().put("batteryExempt", KeepAwake.batteryExempt(activity)))
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
    val id =
      try {
        Settings.Secure.getString(activity.contentResolver, Settings.Secure.ANDROID_ID)
      } catch (e: Exception) {
        ""
      }
    invoke.resolve(
      JSObject()
        .put("deviceId", id ?: "")
        .put("model", "${Build.MANUFACTURER} ${Build.MODEL}")
        .put("osVersion", Build.VERSION.RELEASE ?: "")
        .put("tv", isTv(activity))
    )
  }

  /**
   * This device's IPv4 address in the local network (Wi-Fi or Ethernet, not
   * the tunnel's), for the TV's import page; empty when there is none.
   */
  @Command
  fun lanAddress(invoke: Invoke) {
    val info = UnderlyingNetwork.current(activity)
    val local = info.address.takeIf { info.type == "wifi" || info.type == "ethernet" }
    invoke.resolve(JSObject().put("address", local ?: ""))
  }

  /** Launchable apps other than this one, for the per-app proxy list. */
  @Command
  fun listApps(invoke: Invoke) {
    thread {
      try {
        val manager = activity.packageManager
        val apps = JSArray()
        val installed =
          manager.getInstalledApplications(0)
            .filter {
              it.packageName != activity.packageName &&
                // TV apps (YouTube for TV and the like) may only have a
                // leanback launcher entry.
                (manager.getLaunchIntentForPackage(it.packageName) != null ||
                  manager.getLeanbackLaunchIntentForPackage(it.packageName) != null)
            }
            .map { info ->
              Triple(
                info.packageName,
                manager.getApplicationLabel(info).toString(),
                info.flags and ApplicationInfo.FLAG_SYSTEM != 0
              )
            }
            .sortedBy { it.second.lowercase() }
        for ((pkg, label, isSystem) in installed) {
          apps.put(
            JSObject()
              .put("package", pkg)
              .put("label", label)
              .put("system", isSystem)
          )
        }
        invoke.resolve(JSObject().put("apps", apps))
      } catch (e: Exception) {
        invoke.reject("failed to list applications: ${e.message}")
      }
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

  /** The delay of a WireGuard/AmneziaWG or Naive server, see [Engine.probe]. */
  @Command
  fun probeEngine(invoke: Invoke) {
    val args = invoke.parseArgs(ProbeArgs::class.java)
    thread {
      val delay =
        try {
          Engine.probe(activity, args.mode, args.config, args.url, args.timeout)
        } catch (e: Exception) {
          -1L
        }
      invoke.resolve(JSObject().put("delay", delay))
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
          .put("hasTproxy", state.hasTproxy)
      )
    }
  }

  /** Asks Shizuku for permission (interactive); resolves the new state once answered. */
  @Command
  fun requestShizuku(invoke: Invoke) {
    try {
      if (Shizuku.pingBinder() && !Shizuku.isPreV11()) {
        if (Shizuku.checkSelfPermission() == PackageManager.PERMISSION_GRANTED) {
          privilegeState(invoke)
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
                privilegeState(invoke)
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
            privilegeState(invoke)
          }
        }
        Shizuku.requestPermission(PrivilegedShell.SHIZUKU_PERMISSION_REQUEST)
        return
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
      val startTime = System.currentTimeMillis()
      val answer = awaitAnswer(request, START_TIMEOUT_MS) { state ->
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
