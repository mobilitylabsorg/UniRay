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
import java.util.concurrent.Executors
import libXray.DialerController
import libXray.LibXray
import org.json.JSONObject

/**
 * Owns the TUN interface and runs the Xray core on it.
 *
 * The plugin writes what to run into [specFile] and starts the service; the
 * outcome is reported through [onStarted]. The service runs in the app
 * process, so the core's metrics endpoint and log files are directly
 * reachable from the Rust side.
 */
class UniRayVpnService : VpnService() {
  companion object {
    const val ACTION_START = "dev.uniray.app.action.START"
    const val ACTION_STOP = "dev.uniray.app.action.STOP"

    private const val TAG = "UniRayVpn"
    private const val CHANNEL_ID = "vpn"
    private const val NOTIFICATION_ID = 1

    // The tunnel's own subnet. DNS queries are sent to an address inside it,
    // so they enter the tunnel and are answered by the core.
    private const val ADDRESS_V4 = "172.19.0.1"
    private const val DNS_V4 = "172.19.0.2"
    private const val PREFIX_V4 = 30
    private const val ADDRESS_V6 = "fdfe:dcba:9876::1"
    private const val PREFIX_V6 = 126

    @Volatile
    var running = false
      private set

    /** Called once with the error of a start attempt, or null on success. */
    @Volatile
    var onStarted: ((String?) -> Unit)? = null

    /** Called once when a requested stop has completed. */
    @Volatile
    var onStopped: (() -> Unit)? = null

    /** The running service, for updating its notification. */
    @Volatile
    var instance: UniRayVpnService? = null
      private set

    fun specFile(context: Context) = File(context.filesDir, "run/vpn.json")

    fun assetDir(context: Context) = File(context.filesDir, "geo")
  }

  // Start and stop are serialized: libXray keeps process-wide state.
  private val worker = Executors.newSingleThreadExecutor()
  private var tun: ParcelFileDescriptor? = null
  private var notification: NotificationCompat.Builder? = null

  override fun onCreate() {
    super.onCreate()
    instance = this
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (intent?.action == ACTION_STOP) {
      worker.execute { shutdown() }
      return START_NOT_STICKY
    }
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
      onStarted?.invoke(error)
      onStarted = null
      if (error != null) shutdown()
    }
    return START_NOT_STICKY
  }

  /** The user turned the VPN off in system settings, or another VPN took over. */
  override fun onRevoke() {
    worker.execute { shutdown() }
  }

  override fun onDestroy() {
    instance = null
    worker.execute { stopCore() }
    worker.shutdown()
    super.onDestroy()
  }

  private fun startCore(): String? {
    if (running) stopCore()
    val spec = JSONObject(specFile(this).readText())
    val config = JSONObject(spec.getString("config"))
    installAssets()

    val builder =
      Builder()
        // The label Android shows for the VPN in system settings. A neutral
        // name is less conspicuous than the app's own name.
        .setSession(spec.optString("sessionName", getString(R.string.app_name)))
        .setMtu(spec.optInt("mtu", 1500))
        .addAddress(ADDRESS_V4, PREFIX_V4)
        .addRoute("0.0.0.0", 0)
        .addDnsServer(DNS_V4)
    try {
      // Also capture IPv6, so that it cannot bypass the tunnel.
      builder.addAddress(ADDRESS_V6, PREFIX_V6).addRoute("::", 0)
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
    return error
  }

  private fun stopCore() {
    try {
      XrayCore.stop()
      LibXray.resetDNS()
    } catch (e: Exception) {
      Log.w(TAG, "stopping the core failed", e)
    }
    // Closing the descriptor is what takes the interface down.
    try {
      tun?.close()
    } catch (e: Exception) {
      Log.w(TAG, "closing the tunnel failed", e)
    }
    tun = null
    running = false
  }

  private fun shutdown() {
    stopCore()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
      stopForeground(STOP_FOREGROUND_REMOVE)
    } else {
      @Suppress("DEPRECATION") stopForeground(true)
    }
    stopSelf()
    onStopped?.invoke()
    onStopped = null
  }

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

  /**
   * Server in the title; speed and the latency of the last check below.
   * [latencyMs] and [ok] are null before the first check.
   */
  fun updateNotification(server: String, down: Long, up: Long, latencyMs: Long?, ok: Boolean?) {
    val builder = notification ?: return
    if (!running) return
    var text =
      getString(
        R.string.vpn_speed,
        Formatter.formatShortFileSize(this, down),
        Formatter.formatShortFileSize(this, up),
      )
    when {
      ok == true && latencyMs != null -> text += " · " + getString(R.string.vpn_latency, latencyMs)
      ok == false -> text += " · " + getString(R.string.vpn_no_connection)
    }
    builder.setContentTitle(server.ifEmpty { getString(R.string.vpn_connected) }).setContentText(text)
    val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    manager.notify(NOTIFICATION_ID, builder.build())
  }
}
