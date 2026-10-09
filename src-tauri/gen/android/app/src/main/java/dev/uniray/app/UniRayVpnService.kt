package dev.uniray.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.net.VpnService
import android.os.Build
import android.os.ParcelFileDescriptor
import android.text.format.Formatter
import android.util.Log
import androidx.core.app.NotificationCompat
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import libXray.DialerController
import libXray.LibXray
import org.json.JSONObject

/**
 * Owns the TUN interface and runs the Xray core on it.
 *
 * It lives in its own process (`:vpn`, see the manifest): the app's UI can be
 * swept away or crash and the tunnel stays up. The app writes what to run into
 * [specFile] and sends [ACTION_START] with a request id; the outcome comes back
 * through [VpnState]. The core's metrics endpoint and log files are reachable
 * from the app as before (localhost and shared files).
 *
 * The notification keeps itself current (speed every 5 s, latency every 5 min)
 * so that it stays live without the app; while the app runs, it sends the
 * server it found the traffic going through ([ACTION_NOTIFY]).
 */
class UniRayVpnService : VpnService() {
  companion object {
    const val ACTION_START = "dev.uniray.app.action.START"
    const val ACTION_STOP = "dev.uniray.app.action.STOP"
    const val ACTION_NOTIFY = "dev.uniray.app.action.NOTIFY"
    /** Request id echoed in [VpnState], so the app knows its answer. */
    const val EXTRA_REQUEST = "request"
    const val EXTRA_SERVER = "server"
    const val EXTRA_LATENCY = "latencyMs"
    const val EXTRA_OK = "ok"

    private const val TAG = "UniRayVpn"
    private const val CHANNEL_ID = "vpn"
    private const val NOTIFICATION_ID = 1

    // The tunnel's own gateway address and subnet.
    private const val ADDRESS_V4 = "172.19.0.1"
    private const val PREFIX_V4 = 30
    // The DNS server advertised to the system. A public resolver, not an
    // address in the tunnel's private subnet: a private DNS server stands out
    // to on-device VPN scanners ("DNS in a private subnet"). The address is
    // only cosmetic - every port-53 query entering the tunnel is answered by
    // the core regardless of which server it was aimed at (see the config's
    // DNS-hijack rule) - so a well-known public resolver blends in best.
    private const val DNS_V4 = "1.1.1.1"
    private const val ADDRESS_V6 = "fdfe:dcba:9876::1"
    private const val PREFIX_V6 = 126

    private const val REFRESH_SECONDS = 5L
    private const val CHECK_INTERVAL_MS = 5 * 60 * 1000L
    private const val CHECK_URL = "https://www.cloudflare.com/cdn-cgi/trace"

    fun specFile(context: Context) = File(context.filesDir, "run/vpn.json")

    fun assetDir(context: Context) = File(context.filesDir, "geo")
  }

  // Start and stop are serialized: libXray keeps process-wide state.
  private val worker = Executors.newSingleThreadExecutor()
  private val ticker = Executors.newSingleThreadScheduledExecutor()
  private var tick: ScheduledFuture<*>? = null
  @Volatile private var running = false
  // Set once we begin stopping, so no late refresh re-posts the notification
  // after it has been removed (which would leave it stuck, with us gone).
  @Volatile private var stopped = false
  private var tun: ParcelFileDescriptor? = null
  // For a WireGuard/AmneziaWG or Naive server: the engine Xray hands its traffic to.
  private var engine: Engine.Child? = null
  private var notification: NotificationCompat.Builder? = null

  // Notification content.
  @Volatile private var serverName = ""
  @Volatile private var latencyMs: Long? = null
  @Volatile private var checkOk: Boolean? = null
  @Volatile private var lastCheckAt = 0L
  private var metricsSocket = ""
  private var proxyTags: List<String> = emptyList()
  private var lastTraffic: Triple<Long, Long, Long>? = null // time, up, down

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    val request = intent?.getStringExtra(EXTRA_REQUEST)
    when (intent?.action) {
      ACTION_STOP -> {
        worker.execute { shutdown(request) }
        return START_NOT_STICKY
      }
      ACTION_NOTIFY -> {
        intent.getStringExtra(EXTRA_SERVER)?.takeIf { it.isNotEmpty() }?.let { serverName = it }
        if (intent.hasExtra(EXTRA_OK)) {
          checkOk = intent.getBooleanExtra(EXTRA_OK, false)
          latencyMs = if (intent.hasExtra(EXTRA_LATENCY)) intent.getLongExtra(EXTRA_LATENCY, 0) else null
          lastCheckAt = System.currentTimeMillis()
        }
        submitRefresh()
        return START_NOT_STICKY
      }
    }
    // A fresh start: this instance may be reused after a previous stop.
    stopped = false
    // Must happen within seconds of startForegroundService().
    showNotification()
    worker.execute {
      val error =
        try {
          startCore()
        } catch (e: Exception) {
          Log.e(TAG, "start failed", e)
          e.message ?: e.toString()
        }
      if (error != null) {
        shutdown(request, error)
      } else {
        startTicker()
        VpnState.write(this, true, request, null)
      }
    }
    return START_NOT_STICKY
  }

  /** The user turned the VPN off in system settings, or another VPN took over. */
  override fun onRevoke() {
    worker.execute { shutdown(null, "the VPN was turned off by the system or another VPN app") }
  }

  override fun onDestroy() {
    stopped = true
    tick?.cancel(false)
    ticker.shutdown()
    worker.execute {
      // Only when nothing has reported the stop yet: overwriting the answer
      // to a stop request would leave the app waiting for it.
      if (running) {
        stopCore()
        VpnState.write(this, false, null, null)
      }
    }
    worker.shutdown()
    super.onDestroy()
  }

  private fun startCore(): String? {
    if (running) stopCore()
    val spec = JSONObject(specFile(this).readText())
    val config = JSONObject(spec.getString("config"))
    serverName = spec.optString("serverName")
    metricsSocket = spec.optString("metricsSocket")
    proxyTags = spec.optJSONArray("proxyTags")?.let { tags -> (0 until tags.length()).map { tags.getString(it) } }.orEmpty()
    latencyMs = null
    checkOk = null
    lastCheckAt = 0L
    lastTraffic = null
    installAssets()

    val builder =
      Builder()
        // The label Android shows for the VPN in system settings. A neutral
        // name is less conspicuous than the app's own name.
        .setSession(spec.optString("sessionName", getString(R.string.app_name)))
        .setMtu(spec.optInt("mtu", 1500))
        .addAddress(ADDRESS_V4, PREFIX_V4)
        .addDnsServer(DNS_V4)
    // Everything but the engine's server (see the core's tunnel_routes).
    val routes = spec.optJSONArray("routes")?.let { list -> (0 until list.length()).map { list.getString(it) } }
      ?: listOf("0.0.0.0/0", "::/0")
    val (routesV6, routesV4) = routes.partition { it.contains(':') }
    for (route in routesV4) {
      val prefix = route.substringAfter('/', "").toIntOrNull() ?: continue
      try {
        builder.addRoute(route.substringBefore('/'), prefix)
      } catch (e: Exception) {
        Log.w(TAG, "Failed to add route: $route", e)
      }
    }
    try {
      // Also capture IPv6, so that it cannot bypass the tunnel.
      builder.addAddress(ADDRESS_V6, PREFIX_V6)
      for (route in routesV6) {
        val prefix = route.substringAfter('/', "").toIntOrNull() ?: continue
        try {
          builder.addRoute(route.substringBefore('/'), prefix)
        } catch (e: Exception) {
          Log.w(TAG, "Failed to add IPv6 route: $route", e)
        }
      }
    } catch (e: Exception) {
      Log.w(TAG, "IPv6 is not available for the tunnel", e)
    }
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) builder.setMetered(false)

    val packages = spec.optJSONArray("packages")
    val mode = spec.optString("perAppMode", "off")
    if (packages != null && mode != "off") {
      // The app's own requests (connection check, subscription updates) go
      // through the tunnel in every mode; the core's sockets are protected.
      if (mode == "allow") builder.addAllowedApplication(packageName)
      for (index in 0 until packages.length()) {
        val name = packages.getString(index)
        if (name == packageName) continue
        try {
          if (mode == "allow") builder.addAllowedApplication(name) else builder.addDisallowedApplication(name)
        } catch (e: PackageManager.NameNotFoundException) {
          // Uninstalled since it was selected.
        }
      }
    }

    val descriptor = builder.establish() ?: return "the VPN permission was revoked"
    tun = descriptor

    // The core's own sockets must leave through the real network, not the tunnel.
    val controller = DialerController { fd -> protect(fd.toInt()) }
    LibXray.registerDialerController(controller)
    LibXray.setDNS(controller, spec.optString("dns", "8.8.8.8:53"))

    val env = config.optJSONObject("env") ?: JSONObject()
    env.put("xray.tun.fd", descriptor.fd.toString())
    env.put("xray.location.asset", assetDir(this).absolutePath)
    config.put("env", env)

    val error = XrayCore.run(config.toString())
    running = error == null
    if (error != null) return error

    // After Xray, so the first handshake can already leave. The engine's
    // packets to its server leave by the system's routes (the VPN leaves the
    // server out); should they enter the tunnel all the same, the config
    // routes the server's endpoint direct.
    val engineMode = spec.optString("engineMode")
    if (engineMode.isNotEmpty()) {
      val log = Engine.logFile(spec.getString("config"), File(filesDir, "run/engine.log"))
      val child =
        Engine.Child(this, engineMode, spec.getString("engineConfig"), log) { reason ->
          worker.execute { if (running) shutdown(null, reason) }
        }
      engine = child
      child.start()?.let { return it }
    }
    return null
  }

  private fun stopCore() {
    try {
      XrayCore.stop()
      LibXray.resetDNS()
    } catch (e: Exception) {
      Log.w(TAG, "stopping the core failed", e)
    }
    // After Xray, which has nothing to send it any more.
    engine?.stop()
    engine = null
    // Closing the descriptor is what takes the interface down.
    try {
      tun?.close()
    } catch (e: Exception) {
      Log.w(TAG, "closing the tunnel failed", e)
    }
    tun = null
    running = false
  }

  /** Stops everything; [request] is the stop request being answered, if any. */
  /** Stops everything; [request] is the stop request being answered, if any,
   * and [reason] why the tunnel went down when nobody asked. */
  private fun shutdown(request: String?, reason: String? = null) {
    stopped = true
    tick?.cancel(false)
    stopCore()
    clearNotification()
    VpnState.write(this, false, request, reason)
    stopSelf()
  }

  /**
   * Removes the notification for good.
   */
  private fun clearNotification() {
    stopped = true
    notification = null
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
      stopForeground(STOP_FOREGROUND_REMOVE)
    } else {
      @Suppress("DEPRECATION") stopForeground(true)
    }
    (getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager).cancel(NOTIFICATION_ID)
  }

  /** Queues a notification refresh unless we are stopping. */
  private fun submitRefresh() {
    if (stopped) return
    try {
      ticker.execute { refreshNotification() }
    } catch (e: java.util.concurrent.RejectedExecutionException) {
      // The ticker is gone; we are stopping anyway.
    }
  }

  private fun startTicker() {
    tick?.cancel(false)
    tick =
      ticker.scheduleWithFixedDelay(
        {
          try {
            if (System.currentTimeMillis() - lastCheckAt >= CHECK_INTERVAL_MS) {
              lastCheckAt = System.currentTimeMillis()
              kotlin.concurrent.thread(name = "vpn-check", isDaemon = true) {
                checkConnection()
                submitRefresh()
              }
            }
            refreshNotification()
          } catch (e: Exception) {
            Log.w(TAG, "notification refresh failed", e)
          }
        },
        1,
        REFRESH_SECONDS,
        TimeUnit.SECONDS,
      )
  }

  /** One HTTPS request through the tunnel: the latency shown in the notification. */
  private fun checkConnection() {
    val started = System.nanoTime()
    val ok =
      try {
        val connection = URL(CHECK_URL).openConnection() as HttpURLConnection
        connection.connectTimeout = 10_000
        connection.readTimeout = 10_000
        try {
          connection.responseCode == 200
        } finally {
          connection.disconnect()
        }
      } catch (e: Exception) {
        false
      }
    checkOk = ok
    latencyMs = if (ok) (System.nanoTime() - started) / 1_000_000 else null
    lastCheckAt = System.currentTimeMillis()
  }

  /** Bytes sent and received through the proxy, from the core's counters. */
  private fun readTraffic(): Pair<Long, Long>? = TunnelMetrics.readTraffic(metricsSocket, proxyTags)

  /**
   * Unpacks geoip.dat and geosite.dat from the APK, once per installed
   * version: the core reads them as plain files.
   */
  private fun installAssets() {
    val dir = assetDir(this).apply { mkdirs() }
    val stamp = File(dir, ".version")
    val version = packageManager.getPackageInfo(packageName, 0).lastUpdateTime.toString()
    if (stamp.exists() && stamp.readText() == version) return
    for (name in listOf("geoip.dat", "geosite.dat")) {
      assets.open(name).use { input ->
        File(dir, name).outputStream().use { output -> input.copyTo(output) }
      }
    }
    stamp.writeText(version)
  }

  private fun showNotification() {
    val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      manager.createNotificationChannel(
        NotificationChannel(CHANNEL_ID, getString(R.string.vpn_channel), NotificationManager.IMPORTANCE_LOW)
      )
    }
    val immutable = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0
    val open =
      PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), immutable)
    val stop =
      PendingIntent.getService(
        this,
        1,
        Intent(this, UniRayVpnService::class.java).setAction(ACTION_STOP),
        immutable
      )
    val builder =
      NotificationCompat.Builder(this, CHANNEL_ID)
        .setSmallIcon(R.drawable.ic_vpn_key)
        .setContentTitle(getString(R.string.vpn_connected))
        .setContentIntent(open)
        .setOngoing(true)
        .setShowWhen(false)
        // Refreshed every 5 seconds: never sound or pop up again.
        .setOnlyAlertOnce(true)
        .setSilent(true)
        .addAction(0, getString(R.string.vpn_disconnect), stop)
    notification = builder
    val built: Notification = builder.build()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      startForeground(NOTIFICATION_ID, built, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
    } else {
      startForeground(NOTIFICATION_ID, built)
    }
  }

  /** Server in the title; speed and the latency of the last check below. */
  private fun refreshNotification() {
    val builder = notification ?: return
    if (stopped || !running) return
    var (down, up) = 0L to 0L
    readTraffic()?.let { (totalUp, totalDown) ->
      val now = System.currentTimeMillis()
      lastTraffic?.let { (then, lastUp, lastDown) ->
        val seconds = ((now - then).coerceAtLeast(1)) / 1000.0
        up = ((totalUp - lastUp).coerceAtLeast(0) / seconds).toLong()
        down = ((totalDown - lastDown).coerceAtLeast(0) / seconds).toLong()
      }
      lastTraffic = Triple(now, totalUp, totalDown)
    }
    var text =
      getString(
        R.string.vpn_speed,
        Formatter.formatShortFileSize(this, down),
        Formatter.formatShortFileSize(this, up),
      )
    val latency = latencyMs
    when {
      checkOk == true && latency != null -> text += " · " + getString(R.string.vpn_latency, latency)
      checkOk == false -> text += " · " + getString(R.string.vpn_no_connection)
    }
    builder.setContentTitle(serverName.ifEmpty { getString(R.string.vpn_connected) }).setContentText(text)
    val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    manager.notify(NOTIFICATION_ID, builder.build())
  }
}
