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
 *    dokodemo-door inbound on loopback, which nothing may connect to
 *    directly. No interface at all, so `tun0`, `TRANSPORT_VPN` and the
 *    VpnService record an on-device scanner looks for are all absent.
 *  - **roottun** (fallback): the Linux build of Xray, being root, creates
 *    its own TUN, and policy rules route into it (see [rootTunUpScript]).
 *    The interface exists but under a neutral name, and there is still no
 *    VpnService.
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
    // Refuses direct connections to the capture port (filter table).
    private const val CHAIN_GUARD = "UNIRAY_GUARD"
    // The root TUN mode's routing table, and the priority of the rule that
    // sends everything but root's traffic to it: ahead of Android's own rules,
    // which start at 10000 (only the local table's rule, 0, comes earlier).
    private const val TUN_TABLE = "7848"
    private const val TUN_RULE_PREF = "9000"
    // Root CAs for the Linux build of Xray, which looks only in the desktop
    // places: the updatable store (Android 14+), then the system one.
    private const val ANDROID_CA_DIRS = "/apex/com.android.conscrypt/cacerts:/system/etc/security/cacerts"

    private const val REFRESH_SECONDS = 5L
    private const val CHECK_INTERVAL_MS = 5 * 60 * 1000L
    private const val CHECK_URL = "https://www.cloudflare.com/cdn-cgi/trace"
    // Reads of the counters socket that fail in a row before we conclude the
    // root core has died and take the tunnel down.
    private const val DEAD_AFTER_MISSES = 4

    fun specFile(context: Context) = File(context.filesDir, "run/stealth.json")

    fun stealthDir(context: Context) = File(context.filesDir, "stealth")

    /** The Android build of Xray, for tproxy. */
    private const val XRAY_TPROXY = "xray"
    /** The Linux build, for the root TUN: only it creates a TUN itself. */
    private const val XRAY_TUN = "xray-tun"
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
        val left = stopCore()
        VpnState.write(this, false, null, left)
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
    // This service lives in :vpn, where Shizuku's binder does not arrive on its own.
    if (level == Privilege.SHIZUKU_ROOT) PrivilegedShell.ensureShizukuBinder(this)

    val dir = installAssets()
    val configFile = File(dir, "config.json").apply { writeText(config) }
    val port = spec.optInt("port", 12345)
    val ipv6 = spec.optBoolean("ipv6", false)
    val tunName = spec.optString("tunName").ifEmpty { "uniray0" }
    val engine = engineScript(dir, spec, config)

    // Whatever an earlier run left behind (a crash, a lost teardown) would
    // otherwise sit in front of the new rules.
    PrivilegedShell.execPrivileged(level, downScript())
    rules(level)?.let { rulesBeforeFile().writeText(it.joinToString("\n")) }

    val up =
      if (mode == MODE_ROOT_TUN) rootTunUpScript(dir, configFile, tunName, ipv6, engine)
      else tproxyUpScript(dir, configFile, port, ipv6, engine)
    val result = PrivilegedShell.execPrivileged(level, up)
    if (!result.ok) {
      Log.e(TAG, "stealth up failed: ${result.errText()}")
      // Leave nothing half-set-up behind.
      PrivilegedShell.execPrivileged(level, downScript())
      return "could not set up the stealth tunnel: ${result.errText().take(200)}"
    }
    running = true
    return null
  }

  /**
   * Tears the tunnel down and checks that nothing of it is left: a rule left
   * in place sends all traffic to a core that is gone, which takes the whole
   * connection with it. Returns what could not be removed, if anything.
   */
  private fun stopCore(): String? {
    val spec = try {
      JSONObject(specFile(this).readText())
    } catch (e: Exception) {
      null
    }
    // The teardown must run even in a fresh process that never ran startCore
    // (so `level` is NONE) - otherwise the iptables rules, routes and the root
    // xray leak. Recover the level from the spec, or re-detect, as a fallback.
    var lvl = level
    if (!lvl.canStealth) lvl = parseLevel(spec?.optString("privilege").orEmpty())
    if (!lvl.canStealth || lvl == Privilege.SHIZUKU_ROOT) PrivilegedShell.ensureShizukuBinder(this)
    if (!lvl.canStealth) lvl = PrivilegedShell.detect(this).level
    running = false
    var left = ""
    for (attempt in 1..3) {
      val result = PrivilegedShell.execPrivileged(lvl, downScript())
      if (!result.ok) Log.w(TAG, "stealth down reported: ${result.errText()}")
      val check = PrivilegedShell.execPrivileged(lvl, leftoversScript())
      left = if (check.ok) check.outText().trim() else "unknown (${check.errText().take(100)})"
      if (left.isEmpty()) {
        logRuleChanges(lvl, spec)
        return null
      }
      Log.w(TAG, "stealth teardown left: $left (attempt $attempt)")
      Thread.sleep(500)
    }
    return "could not remove the stealth network setup (${left.replace('\n', ',')}); restart the phone to clear it"
  }

  /** Stops everything; [request] is the stop request being answered, if any,
   * and [reason] why the tunnel went down when nobody asked. */
  private fun shutdown(request: String?, reason: String? = null) {
    stopped = true
    tick?.cancel(false)
    val teardown = stopCore()
    clearNotification()
    VpnState.write(this, false, request, teardown ?: reason)
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

  /** Unpacks the standalone xray binaries and the geo databases into the app's
   * private dir, once per installed version. Returns the stealth dir. */
  private fun installAssets(): File {
    val dir = stealthDir(this).apply { mkdirs() }
    val geo = File(filesDir, "geo").apply { mkdirs() }
    val stamp = File(dir, ".version")
    val version = packageManager.getPackageInfo(packageName, 0).lastUpdateTime.toString()
    if (stamp.exists() && stamp.readText() == version) return dir

    for (name in listOf(XRAY_TPROXY, XRAY_TUN)) {
      val xray = File(dir, name)
      assets.open("stealth/$name").use { input -> xray.outputStream().use { input.copyTo(it) } }
      xray.setExecutable(true, false)
      xray.setReadable(true, false)
    }
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
    Pair(File(filesDir, "geo").absolutePath, File(dir, "xray.pid").absolutePath)

  /**
   * Launch [binary] (one of the xray builds in [dir]) detached with [env] in
   * front, wait for the counters socket, open it to the app.
   */
  private fun launch(dir: File, configFile: File, binary: String = XRAY_TPROXY, env: String = ""): String {
    val (geo, pid) = paths(dir)
    val xray = File(dir, binary).absolutePath
    val log = File(dir, "xray.log").absolutePath
    val sock = metricsSocket
    return """
      cd ${dir.absolutePath}
      # A socket or lock left by an earlier core, maybe not openable by this
      # one; root may remove it whoever made it.
      rm -f "$sock" "$sock.lock"
      $env XRAY_LOCATION_ASSET=$geo setsid $xray run -c ${configFile.absolutePath} >$log 2>&1 &
      echo ${'$'}! > $pid
      for i in ${'$'}(seq 1 40); do [ -S "$sock" ] && chmod 666 "$sock" && break; sleep 0.1; done
    """.trimIndent()
  }

  /**
   * For a WireGuard/AmneziaWG or Naive server: starts the engine as root, detached
   * (the teardown kills it), and lets only root - Xray - reach its port; the
   * engine itself also accepts only root's connections. Runs before Xray.
   * Empty for the other servers.
   */
  private fun engineScript(dir: File, spec: JSONObject, config: String): String {
    val mode = spec.optString("engineMode")
    if (mode.isEmpty()) return ""
    val port = spec.optInt("enginePort")
    val configFile = File(dir, "engine.json").apply { writeText(Engine.withNaive(this@StealthService, spec.getString("engineConfig"))) }
    val out = File(dir, "engine.out").absolutePath
    val log = Engine.logFile(config, File(dir, "engine.log")).absolutePath
    val pid = File(dir, "engine.pid").absolutePath
    val engine = Engine.binary(this).absolutePath
    return """
      iptables -w -t filter -N $CHAIN_GUARD 2>/dev/null || true
      iptables -w -t filter -C OUTPUT -j $CHAIN_GUARD 2>/dev/null || iptables -w -t filter -A OUTPUT -j $CHAIN_GUARD
      iptables -w -t filter -A $CHAIN_GUARD -d 127.0.0.0/8 -p tcp --dport $port -m owner ! --uid-owner 0 -j REJECT --reject-with tcp-reset
      setsid $engine $mode --config ${configFile.absolutePath} </dev/null >$out 2>>$log &
      echo ${'$'}! > $pid
      for i in ${'$'}(seq 1 100); do grep -q '^ready ' $out 2>/dev/null && break; sleep 0.1; done
      if ! grep -q '^ready ' $out 2>/dev/null; then
        echo "the engine did not start:" >&2
        tail -n 5 $log >&2
        exit 1
      fi
    """.trimIndent()
  }

  private val privateNets =
    listOf("0.0.0.0/8", "10.0.0.0/8", "127.0.0.0/8", "169.254.0.0/16", "172.16.0.0/12", "192.168.0.0/16", "224.0.0.0/4", "240.0.0.0/4")

  /** Without IPv6 support, drops global unicast v6 egress so it cannot bypass
   * the v4 tunnel. (ULA/link-local stay for on-device and LAN use.) */
  private fun blockIpv6Script(ipv6: Boolean): String =
    if (ipv6) ""
    else
      """
      ip6tables -w -t filter -N $CHAIN_OUT 2>/dev/null || ip6tables -w -t filter -F $CHAIN_OUT
      ip6tables -w -t filter -A $CHAIN_OUT -o lo -j RETURN
      ip6tables -w -t filter -A $CHAIN_OUT -d fc00::/7 -j RETURN
      ip6tables -w -t filter -A $CHAIN_OUT -d fe80::/10 -j RETURN
      ip6tables -w -t filter -A $CHAIN_OUT -d 2000::/3 -j DROP
      ip6tables -w -t filter -A OUTPUT -j $CHAIN_OUT
      """.trimIndent()

  private fun tproxyUpScript(dir: File, configFile: File, port: Int, ipv6: Boolean, engine: String): String {
    val returns = privateNets.joinToString("\n") { "iptables -w -t mangle -A $CHAIN_PRE -d $it -j RETURN" }
    val returnsOut = privateNets.joinToString("\n") { "iptables -w -t mangle -A $CHAIN_OUT -d $it -j RETURN" }
    val v6 = blockIpv6Script(ipv6)
    return """
      set -e
      ip rule add fwmark $MARK lookup $TABLE 2>/dev/null || true
      ip route add local 0.0.0.0/0 dev lo table $TABLE 2>/dev/null || true

      iptables -w -t mangle -N $CHAIN_PRE 2>/dev/null || iptables -w -t mangle -F $CHAIN_PRE
      iptables -w -t mangle -N $CHAIN_OUT 2>/dev/null || iptables -w -t mangle -F $CHAIN_OUT
      $returns
      iptables -w -t mangle -A $CHAIN_PRE -p tcp -j TPROXY --on-ip 127.0.0.1 --on-port $port --tproxy-mark $MARK
      iptables -w -t mangle -A $CHAIN_PRE -p udp -j TPROXY --on-ip 127.0.0.1 --on-port $port --tproxy-mark $MARK
      iptables -w -t mangle -A PREROUTING -j $CHAIN_PRE

      iptables -w -t mangle -A $CHAIN_OUT -m owner --uid-owner 0 -j RETURN
      $returnsOut
      iptables -w -t mangle -A $CHAIN_OUT -p tcp -j MARK --set-mark $MARK
      iptables -w -t mangle -A $CHAIN_OUT -p udp -j MARK --set-mark $MARK
      iptables -w -t mangle -A OUTPUT -j $CHAIN_OUT

      # The capture port takes no password: the kernel hands it connections,
      # there is no handshake. So nothing may connect to it directly - any app
      # could otherwise find it, and the core would dial itself. Redirected
      # traffic keeps its real destination and never matches.
      iptables -w -t filter -N $CHAIN_GUARD 2>/dev/null || iptables -w -t filter -F $CHAIN_GUARD
      iptables -w -t filter -A $CHAIN_GUARD -d 127.0.0.0/8 -p tcp --dport $port -j REJECT --reject-with tcp-reset
      iptables -w -t filter -A $CHAIN_GUARD -d 127.0.0.0/8 -p udp --dport $port -j DROP
      iptables -w -t filter -A OUTPUT -j $CHAIN_GUARD

      $v6

      $engine

      ${launch(dir, configFile)}
    """.trimIndent()
  }

  /**
   * Root TUN: the Linux build of Xray creates [tun] and gives it its
   * addresses; this script routes into it. Android picks routes through
   * policy rules, one table per network, so Xray's own route setup (the main
   * table) would never be consulted. Instead a rule ahead of Android's sends
   * everything except root's traffic to a table whose default route is the
   * TUN. The core runs as root, so its own connections keep using Android's
   * routing and do not loop back in, the same exemption the tproxy mode makes.
   */
  private fun rootTunUpScript(dir: File, configFile: File, tun: String, ipv6: Boolean, engine: String): String {
    val log = File(dir, "xray.log").absolutePath
    val savedRpFilter = File(dir, "rp_filter.saved").absolutePath
    val throws = privateNets.joinToString("\n") { "ip route add throw $it table $TUN_TABLE" }
    val v6 =
      if (ipv6)
        """
        ip -6 route add default dev $tun table $TUN_TABLE
        ip -6 route add throw fc00::/7 table $TUN_TABLE
        ip -6 route add throw fe80::/10 table $TUN_TABLE
        ip -6 rule add not uidrange 0-0 lookup $TUN_TABLE pref $TUN_RULE_PREF
        """.trimIndent()
      else blockIpv6Script(false)
    return """
      set -e
      if ip link show $tun >/dev/null 2>&1; then
        echo "a network interface named $tun already exists; set another interface name in Settings" >&2
        exit 1
      fi
      [ -e /dev/net/tun ] || { mkdir -p /dev/net; mknod /dev/net/tun c 10 200; }

      $engine

      ${launch(dir, configFile, XRAY_TUN, "SSL_CERT_DIR=$ANDROID_CA_DIRS TMPDIR=${dir.absolutePath}")}

      for i in ${'$'}(seq 1 50); do ip link show $tun 2>/dev/null | grep -q '[<,]UP[,>]' && break; sleep 0.1; done
      if ! ip link show $tun 2>/dev/null | grep -q '[<,]UP[,>]'; then
        echo "the core did not bring up $tun:" >&2
        tail -n 5 $log >&2
        exit 1
      fi

      # Replies come in on the TUN from addresses Android routes elsewhere;
      # strict reverse-path filtering would drop them. Relaxed to loose for
      # the session, restored on teardown.
      [ -f $savedRpFilter ] || cat /proc/sys/net/ipv4/conf/all/rp_filter > $savedRpFilter
      [ "${'$'}(cat /proc/sys/net/ipv4/conf/all/rp_filter)" = 1 ] && echo 2 > /proc/sys/net/ipv4/conf/all/rp_filter
      echo 0 > /proc/sys/net/ipv4/conf/$tun/rp_filter

      ip route add default dev $tun table $TUN_TABLE
      $throws
      ip rule add not uidrange 0-0 lookup $TUN_TABLE pref $TUN_RULE_PREF

      $v6
    """.trimIndent()
  }

  /**
   * Removes everything either mode sets up, whatever mode ran and whatever
   * the IPv6 setting was: idempotent, so it also clears what an earlier run
   * left behind. `-w` waits for the xtables lock, which Android's netd holds
   * often; without it a delete fails quietly and leaves traffic going to a
   * core that is gone. Jumps are deleted until none is left, in case a rule
   * was added twice.
   */
  private fun downScript(): String {
    val pid = File(stealthDir(this), "xray.pid").absolutePath
    val enginePid = File(stealthDir(this), "engine.pid").absolutePath
    val dir = stealthDir(this).absolutePath
    val savedRpFilter = File(stealthDir(this), "rp_filter.saved").absolutePath
    // Order matters: the rules go first, so that traffic is the system's again
    // before anything else can go wrong; then the processes.
    //
    // Every delete names what is ours exactly. A rule is matched by its table
    // too, never by its priority alone: the system or the ROM may have rules
    // of its own at the same priority, and deleting those cut the phone off
    // the network until a reboot.
    //
    // A pid file is trusted only when the process is still ours: an old one
    // may name a pid the system has since given to something else.
    //
    // The `[y]` keeps pkill from matching this very script: Shizuku runs it as
    // `sh -c <script>`, so the script's text is the shell's command line, and
    // a plain "…/xray run" would make pkill kill the shell running it. The
    // bracket matches "xray", not its own spelling.
    return """
      while iptables -w -t mangle -D OUTPUT -j $CHAIN_OUT 2>/dev/null; do :; done
      while iptables -w -t mangle -D PREROUTING -j $CHAIN_PRE 2>/dev/null; do :; done
      iptables -w -t mangle -F $CHAIN_OUT 2>/dev/null || true
      iptables -w -t mangle -F $CHAIN_PRE 2>/dev/null || true
      iptables -w -t mangle -X $CHAIN_OUT 2>/dev/null || true
      iptables -w -t mangle -X $CHAIN_PRE 2>/dev/null || true
      while iptables -w -t filter -D OUTPUT -j $CHAIN_GUARD 2>/dev/null; do :; done
      iptables -w -t filter -F $CHAIN_GUARD 2>/dev/null || true
      iptables -w -t filter -X $CHAIN_GUARD 2>/dev/null || true
      while ip6tables -w -t filter -D OUTPUT -j $CHAIN_OUT 2>/dev/null; do :; done
      ip6tables -w -t filter -F $CHAIN_OUT 2>/dev/null || true
      ip6tables -w -t filter -X $CHAIN_OUT 2>/dev/null || true
      while ip rule del fwmark $MARK lookup $TABLE 2>/dev/null; do :; done
      ip route flush table $TABLE 2>/dev/null || true
      while ip rule del pref $TUN_RULE_PREF lookup $TUN_TABLE 2>/dev/null; do :; done
      while ip -6 rule del pref $TUN_RULE_PREF lookup $TUN_TABLE 2>/dev/null; do :; done
      ip route flush table $TUN_TABLE 2>/dev/null || true
      ip -6 route flush table $TUN_TABLE 2>/dev/null || true
      if [ -f $savedRpFilter ]; then cat $savedRpFilter > /proc/sys/net/ipv4/conf/all/rp_filter; rm -f $savedRpFilter; fi

      ours() { [ -n "${'$'}1" ] && tr '\0' ' ' < /proc/${'$'}1/cmdline 2>/dev/null | grep -qE "${'$'}2"; }
      for file in $pid $enginePid; do
        p=${'$'}(cat ${'$'}file 2>/dev/null)
        ours "${'$'}p" "$dir/xra[y]|libuniray_engin[e]" && kill ${'$'}p 2>/dev/null
        rm -f ${'$'}file
      done
      pkill -f "$dir/xra[y] run" 2>/dev/null || true
      pkill -f "$dir/xra[y]-tun run" 2>/dev/null || true
      pkill -f "libuniray_engin[e].so" 2>/dev/null || true
      pkill -f "libnaiv[e].so" 2>/dev/null || true
      rm -f $dir/engine.json $dir/engine.out 2>/dev/null || true
      true
    """.trimIndent()
  }

  /** The routing rules, both families, for [logRuleChanges]. */
  private fun rules(level: Privilege): List<String>? {
    val result = PrivilegedShell.execPrivileged(level, "ip rule 2>/dev/null; ip -6 rule 2>/dev/null | sed 's/^/v6 /'")
    return if (result.ok) result.out.map { it.trim().replace(Regex("\\s+"), " ") }.filter { it.isNotEmpty() } else null
  }

  private fun rulesBeforeFile() = File(stealthDir(this), "rules.before")

  /**
   * Compares the routing rules after a teardown with those from before the
   * connection and reports what changed in the app's log, with whether the
   * internet is reachable now. Nothing of ours may be left, and nothing of
   * the system's may be missing; when the phone has no internet after a
   * disconnect, this says why. (A change of network while connected also
   * changes the system's rules; that shows here too.)
   */
  private fun logRuleChanges(level: Privilege, spec: JSONObject?) {
    val before = try {
      rulesBeforeFile().readLines().filter { it.isNotEmpty() }
    } catch (e: Exception) {
      return
    }
    val after = rules(level) ?: return
    val ours = Regex("lookup ($TABLE|$TUN_TABLE)\\b")
    val missing = before.filter { it !in after && !ours.containsMatchIn(it) }
    val left = after.filter { ours.containsMatchIn(it) }
    val log = spec?.optString("config")?.let { Engine.logFile(it, File(stealthDir(this), "teardown.log")) }
    val lines = mutableListOf<String>()
    if (missing.isNotEmpty()) lines += "[Warning] stealth: routing rules gone since connecting: ${missing.joinToString(" | ")}"
    if (left.isNotEmpty()) lines += "[Warning] stealth: routing rules of ours left: ${left.joinToString(" | ")}"
    val probe = PrivilegedShell.execPrivileged(level, "ip route get 1.1.1.1 2>&1 | head -1; ping -c 1 -W 3 1.1.1.1 >/dev/null 2>&1 && echo reachable || echo unreachable")
    val reach = probe.out.map { it.trim() }.filter { it.isNotEmpty() }
    val reachable = reach.lastOrNull() == "reachable"
    lines += "[${if (reachable) "Info" else "Warning"}] stealth: after teardown 1.1.1.1 is ${reach.lastOrNull() ?: "unknown"} (${reach.firstOrNull().orEmpty()})"
    Log.i(TAG, lines.joinToString("\n"))
    try {
      log?.appendText(lines.joinToString("\n", postfix = "\n"))
    } catch (e: Exception) {
      // The log is a convenience.
    }
    rulesBeforeFile().delete()
  }

  /** Prints what of the setup is still in place; nothing when all is gone. */
  private fun leftoversScript(): String =
    """
    iptables -w -t mangle -S 2>/dev/null | grep -q -e $CHAIN_OUT -e $CHAIN_PRE && echo "iptables rules"
    iptables -w -t filter -S 2>/dev/null | grep -q $CHAIN_GUARD && echo "iptables guard"
    ip6tables -w -t filter -S 2>/dev/null | grep -q $CHAIN_OUT && echo "ip6tables rules"
    ip rule 2>/dev/null | grep -qE "lookup ($TABLE|$TUN_TABLE)( |${'$'})" && echo "routing rules"
    ip -6 rule 2>/dev/null | grep -qE "lookup $TUN_TABLE( |${'$'})" && echo "IPv6 routing rules"
    pgrep -f "libuniray_engin[e].so" >/dev/null 2>&1 && echo "the engine"
    pgrep -f "libnaiv[e].so" >/dev/null 2>&1 && echo "naive"
    true
    """.trimIndent()

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
        worker.execute { shutdown(null, "the stealth core stopped responding") }
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
