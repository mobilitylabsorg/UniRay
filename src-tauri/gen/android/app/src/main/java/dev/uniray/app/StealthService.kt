package dev.uniray.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.text.format.Formatter
import android.util.Log
import androidx.core.app.NotificationCompat
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import org.json.JSONObject

/**
 * The stealth tunnel: runs the standalone Xray as a separate root process and
 * captures traffic without a VpnService.
 *
 * Capturing system-wide traffic needs CAP_NET_ADMIN, which the in-process
 * libXray cannot have - so this runs the bundled `xray` binary through a root
 * shell ([PrivilegedShell]). Two ways in:
 *
 *  - **tproxy** (primary): iptables TPROXY rules hand TCP+UDP to a
 *    dokodemo-door inbound. No interface at all, so `tun0`, `TRANSPORT_VPN`
 *    and the VpnService record an on-device scanner looks for are all absent.
 *  - **roottun** (fallback): Xray, being root, creates its own TUN and
 *    installs the routes itself (the desktop "managed" path). The interface
 *    exists but under a neutral name, and there is still no VpnService.
 *
 * Like [UniRayVpnService] it lives in the `:vpn` process so it outlives the
 * app's UI, keeps its own notification current, and reports across processes
 * through [VpnState]. The two tunnel services never run at once.
 */
class StealthService : Service() {
  companion object {
    const val ACTION_START = "dev.uniray.app.action.STEALTH_START"
    const val ACTION_STOP = "dev.uniray.app.action.STEALTH_STOP"
    const val ACTION_NOTIFY = "dev.uniray.app.action.STEALTH_NOTIFY"
    const val EXTRA_REQUEST = "request"
    const val EXTRA_SERVER = "server"
    const val EXTRA_LATENCY = "latencyMs"
    const val EXTRA_OK = "ok"

    const val MODE_TPROXY = "tproxy"
    const val MODE_ROOT_TUN = "roottun"

    private const val TAG = "UniRayStealth"
    private const val CHANNEL_ID = "vpn"
    private const val NOTIFICATION_ID = 1

    // The firewall mark and routing table the tproxy setup owns. The mark is a
    // single high bit, set and matched through a mask, so it does not disturb
    // the fwmark bits Android's netd uses for network selection (netId and
    // flags live in the low bits). The table id must be decimal: `ip` parses
    // `table`/`lookup` values in base 10, not hex.
    private const val MARK = "0x10000000/0x10000000"
    private const val TABLE = "7847"
    private const val CHAIN_PRE = "UNIRAY_PRE"
    private const val CHAIN_OUT = "UNIRAY_OUT"

    private const val REFRESH_SECONDS = 5L
    private const val CHECK_INTERVAL_MS = 5 * 60 * 1000L
    private const val CHECK_URL = "https://www.cloudflare.com/cdn-cgi/trace"
    // Reads of the counters socket that fail in a row before we conclude the
    // root core has died and take the tunnel down.
    private const val DEAD_AFTER_MISSES = 4

    fun specFile(context: Context) = File(context.filesDir, "run/stealth.json")

    fun stealthDir(context: Context) = File(context.filesDir, "stealth")
  }

  private val worker = Executors.newSingleThreadExecutor()
  private val ticker = Executors.newSingleThreadScheduledExecutor()
  private var tick: ScheduledFuture<*>? = null
  @Volatile private var running = false
  // Set once we begin stopping, so no late refresh re-posts the notification.
  @Volatile private var stopped = false
  private var notification: NotificationCompat.Builder? = null

  private var level = Privilege.NONE
  private var mode = MODE_TPROXY

  @Volatile private var serverName = ""
  @Volatile private var latencyMs: Long? = null
  @Volatile private var checkOk: Boolean? = null
  @Volatile private var lastCheckAt = 0L
  private var metricsSocket = ""
  private var proxyTags: List<String> = emptyList()
  private var lastTraffic: Triple<Long, Long, Long>? = null
  private var missedReads = 0

  override fun onBind(intent: Intent?): IBinder? = null

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
    stopped = false
    showNotification()
    worker.execute {
      val error =
        try {
          startCore()
        } catch (e: Exception) {
          Log.e(TAG, "stealth start failed", e)
          e.message ?: e.toString()
        }
      if (error != null) {
        shutdown(null)
      } else {
        startTicker()
      }
      VpnState.write(this, error == null, request, error)
    }
    return START_NOT_STICKY
  }

  override fun onDestroy() {
    stopped = true
    tick?.cancel(false)
    ticker.shutdown()
    worker.execute {
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
    val config = spec.getString("config")
    serverName = spec.optString("serverName")
    metricsSocket = spec.optString("metricsSocket")
    mode = spec.optString("mode", MODE_TPROXY)
    level = parseLevel(spec.optString("privilege"))
    proxyTags =
      spec.optJSONArray("proxyTags")?.let { tags -> (0 until tags.length()).map { tags.getString(it) } }.orEmpty()
    latencyMs = null
    checkOk = null
    lastCheckAt = 0L
    lastTraffic = null
    missedReads = 0

    if (!level.canStealth) return "stealth needs root; this device offers none"

    val dir = installAssets()
    val configFile = File(dir, "config.json").apply { writeText(config) }
    val port = spec.optInt("port", 12345)
    val ipv6 = spec.optBoolean("ipv6", false)

    val up =
      if (mode == MODE_ROOT_TUN) rootTunUpScript(dir, configFile)
      else tproxyUpScript(dir, configFile, port, ipv6)
    val result = PrivilegedShell.execPrivileged(level, up)
    if (!result.ok) {
      Log.e(TAG, "stealth up failed: ${result.errText()}")
      // Leave nothing half-set-up behind.
      PrivilegedShell.execPrivileged(level, downScript(ipv6))
      return "could not set up the stealth tunnel: ${result.errText().take(200)}"
    }
    running = true
    return null
  }

  private fun stopCore() {
    val spec = try {
      JSONObject(specFile(this).readText())
    } catch (e: Exception) {
      null
    }
    val ipv6 = spec?.optBoolean("ipv6", false) ?: false
    // The teardown must run even in a fresh process that never ran startCore
    // (so `level` is NONE) - otherwise the iptables rules, routes and the root
    // xray leak. Recover the level from the spec, or re-detect, as a fallback.
    var lvl = level
    if (!lvl.canStealth) lvl = parseLevel(spec?.optString("privilege").orEmpty())
    if (!lvl.canStealth) lvl = PrivilegedShell.detect(this).level
    val result = PrivilegedShell.execPrivileged(lvl, downScript(ipv6))
    if (!result.ok) Log.w(TAG, "stealth down reported: ${result.errText()}")
    running = false
  }

  private fun shutdown(request: String?) {
    stopped = true
    tick?.cancel(false)
    stopCore()
    clearNotification()
    VpnState.write(this, false, request, null)
    stopSelf()
  }

  /**
   * Removes the notification for good, on the ticker thread after any queued
   * refresh, so nothing re-posts it; [stopped] blocks later refreshes.
   */
  private fun clearNotification() {
    val drop = Runnable {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
        stopForeground(STOP_FOREGROUND_REMOVE)
      } else {
        @Suppress("DEPRECATION") stopForeground(true)
      }
      (getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager).cancel(NOTIFICATION_ID)
    }
    try {
      ticker.execute(drop)
      ticker.shutdown()
    } catch (e: java.util.concurrent.RejectedExecutionException) {
      drop.run()
    }
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

  /** Unpacks the standalone xray binary and the geo databases into the app's
   * private dir, once per installed version. Returns the stealth dir. */
  private fun installAssets(): File {
    val dir = stealthDir(this).apply { mkdirs() }
    val geo = File(filesDir, "geo").apply { mkdirs() }
    val stamp = File(dir, ".version")
    val version = packageManager.getPackageInfo(packageName, 0).lastUpdateTime.toString()
    if (stamp.exists() && stamp.readText() == version) return dir

    val xray = File(dir, "xray")
    assets.open("stealth/xray").use { input -> xray.outputStream().use { input.copyTo(it) } }
    xray.setExecutable(true, false)
    xray.setReadable(true, false)
    for (name in listOf("geoip.dat", "geosite.dat")) {
      assets.open(name).use { input -> File(geo, name).outputStream().use { input.copyTo(it) } }
    }
    stamp.writeText(version)
    return dir
  }

  // --- Privileged scripts ---------------------------------------------------
  //
  // Generated here, run as root by the shell. Paths are absolute; teardown is
  // idempotent (every delete tolerates "not there").

  private fun paths(dir: File) =
    Triple(File(dir, "xray").absolutePath, File(filesDir, "geo").absolutePath, File(dir, "xray.pid").absolutePath)

  /** Launch xray detached, wait for the counters socket, open it to the app. */
  private fun launch(dir: File, configFile: File): String {
    val (xray, geo, pid) = paths(dir)
    val log = File(dir, "xray.log").absolutePath
    val sock = metricsSocket
    return """
      cd ${dir.absolutePath}
      XRAY_LOCATION_ASSET=$geo setsid $xray run -c ${configFile.absolutePath} >$log 2>&1 &
      echo ${'$'}! > $pid
      for i in ${'$'}(seq 1 40); do [ -S "$sock" ] && chmod 666 "$sock" && break; sleep 0.1; done
    """.trimIndent()
  }

  private val privateNets =
    listOf("0.0.0.0/8", "10.0.0.0/8", "127.0.0.0/8", "169.254.0.0/16", "172.16.0.0/12", "192.168.0.0/16", "224.0.0.0/4", "240.0.0.0/4")

  private fun tproxyUpScript(dir: File, configFile: File, port: Int, ipv6: Boolean): String {
    val returns = privateNets.joinToString("\n") { "iptables -t mangle -A $CHAIN_PRE -d $it -j RETURN" }
    val returnsOut = privateNets.joinToString("\n") { "iptables -t mangle -A $CHAIN_OUT -d $it -j RETURN" }
    // Without IPv6 support, drop global unicast v6 egress so it cannot bypass
    // the v4 tunnel. (ULA/link-local stay for on-device and LAN use.)
    val v6 =
      if (ipv6) ""
      else
        """
        ip6tables -t filter -N $CHAIN_OUT 2>/dev/null || ip6tables -t filter -F $CHAIN_OUT
        ip6tables -t filter -A $CHAIN_OUT -o lo -j RETURN
        ip6tables -t filter -A $CHAIN_OUT -d fc00::/7 -j RETURN
        ip6tables -t filter -A $CHAIN_OUT -d fe80::/10 -j RETURN
        ip6tables -t filter -A $CHAIN_OUT -d 2000::/3 -j DROP
        ip6tables -t filter -A OUTPUT -j $CHAIN_OUT
        """.trimIndent()
    return """
      set -e
      ip rule add fwmark $MARK lookup $TABLE 2>/dev/null || true
      ip route add local 0.0.0.0/0 dev lo table $TABLE 2>/dev/null || true

      iptables -t mangle -N $CHAIN_PRE 2>/dev/null || iptables -t mangle -F $CHAIN_PRE
      iptables -t mangle -N $CHAIN_OUT 2>/dev/null || iptables -t mangle -F $CHAIN_OUT
      $returns
      iptables -t mangle -A $CHAIN_PRE -p tcp -j TPROXY --on-port $port --tproxy-mark $MARK
      iptables -t mangle -A $CHAIN_PRE -p udp -j TPROXY --on-port $port --tproxy-mark $MARK
      iptables -t mangle -A PREROUTING -j $CHAIN_PRE

      iptables -t mangle -A $CHAIN_OUT -m owner --uid-owner 0 -j RETURN
      $returnsOut
      iptables -t mangle -A $CHAIN_OUT -p tcp -j MARK --set-mark $MARK
      iptables -t mangle -A $CHAIN_OUT -p udp -j MARK --set-mark $MARK
      iptables -t mangle -A OUTPUT -j $CHAIN_OUT

      $v6

      ${launch(dir, configFile)}
    """.trimIndent()
  }

  private fun rootTunUpScript(dir: File, configFile: File): String {
    // Xray creates and configures its own TUN (it has CAP_NET_ADMIN as root):
    // nothing to set up here beyond making /dev/net/tun usable and launching.
    return """
      set -e
      [ -e /dev/net/tun ] || { mkdir -p /dev/net; mknod /dev/net/tun c 10 200; }
      ${launch(dir, configFile)}
    """.trimIndent()
  }

  private fun downScript(ipv6: Boolean): String {
    val pid = File(stealthDir(this), "xray.pid").absolutePath
    val xray = File(stealthDir(this), "xray").absolutePath
    val v6 =
      if (ipv6) ""
      else
        """
        ip6tables -t filter -D OUTPUT -j $CHAIN_OUT 2>/dev/null || true
        ip6tables -t filter -F $CHAIN_OUT 2>/dev/null || true
        ip6tables -t filter -X $CHAIN_OUT 2>/dev/null || true
        """.trimIndent()
    return """
      [ -f $pid ] && kill ${'$'}(cat $pid) 2>/dev/null || true
      pkill -f "$xray run" 2>/dev/null || true
      rm -f $pid 2>/dev/null || true
      iptables -t mangle -D OUTPUT -j $CHAIN_OUT 2>/dev/null || true
      iptables -t mangle -D PREROUTING -j $CHAIN_PRE 2>/dev/null || true
      iptables -t mangle -F $CHAIN_OUT 2>/dev/null || true
      iptables -t mangle -F $CHAIN_PRE 2>/dev/null || true
      iptables -t mangle -X $CHAIN_OUT 2>/dev/null || true
      iptables -t mangle -X $CHAIN_PRE 2>/dev/null || true
      ip rule del fwmark $MARK lookup $TABLE 2>/dev/null || true
      ip route flush table $TABLE 2>/dev/null || true
      $v6
    """.trimIndent()
  }

  private fun parseLevel(name: String): Privilege =
    try {
      Privilege.valueOf(name)
    } catch (e: Exception) {
      Privilege.NONE
    }

  // --- Notification (mirrors the VPN service) -------------------------------

  private fun startTicker() {
    tick?.cancel(false)
    tick =
      ticker.scheduleWithFixedDelay(
        {
          try {
            if (System.currentTimeMillis() - lastCheckAt >= CHECK_INTERVAL_MS) checkConnection()
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

  private fun showNotification() {
    val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      manager.createNotificationChannel(
        NotificationChannel(CHANNEL_ID, getString(R.string.vpn_channel), NotificationManager.IMPORTANCE_LOW)
      )
    }
    val immutable = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0
    val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), immutable)
    val stop =
      PendingIntent.getService(
        this,
        2,
        Intent(this, StealthService::class.java).setAction(ACTION_STOP),
        immutable,
      )
    val builder =
      NotificationCompat.Builder(this, CHANNEL_ID)
        .setSmallIcon(R.drawable.ic_vpn_key)
        .setContentTitle(getString(R.string.vpn_connected))
        .setContentIntent(open)
        .setOngoing(true)
        .setShowWhen(false)
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

  private fun refreshNotification() {
    val builder = notification ?: return
    if (stopped || !running) return
    var (down, up) = 0L to 0L
    val traffic = TunnelMetrics.readTraffic(metricsSocket, proxyTags)
    if (traffic == null) {
      // The root core answers on the socket; several misses mean it is gone.
      if (++missedReads >= DEAD_AFTER_MISSES) {
        Log.w(TAG, "stealth core stopped answering; shutting down")
        worker.execute { shutdown(null) }
      }
    } else {
      missedReads = 0
      val (totalUp, totalDown) = traffic
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
