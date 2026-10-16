package dev.uniray.app

import android.annotation.SuppressLint
import android.content.Context
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import android.util.Log

/**
 * Keeps the tunnel working with the screen off (the "stay awake" setting):
 * a CPU wake lock, so the phone's sleep does not stall Xray and the engine
 * and drop the connections through them, and a Wi-Fi lock, so Wi-Fi does not
 * doze off either. Held by a tunnel service while it is connected, released
 * when it stops.
 */
class KeepAwake(private val context: Context) {
  private var wake: PowerManager.WakeLock? = null
  private var wifi: WifiManager.WifiLock? = null
  private var isWakeLockEnabled = false
  private var isTunnelActive = false

  @Synchronized
  fun start(enableWakeLock: Boolean) {
    isTunnelActive = true
    isWakeLockEnabled = enableWakeLock
    val power = context.getSystemService(PowerManager::class.java)
    val screenOn = power?.isInteractive ?: true
    updateWifiLock(screenOn)
    if (enableWakeLock) {
      acquireWakeLock()
    } else {
      releaseWakeLock()
    }
  }

  @Synchronized
  fun set(enabled: Boolean) {
    isWakeLockEnabled = enabled
    if (isTunnelActive) {
      if (enabled) acquireWakeLock() else releaseWakeLock()
    } else if (enabled) {
      start(true)
    } else {
      release()
    }
  }

  @Synchronized
  fun onScreenStateChanged(screenOn: Boolean) {
    if (!isTunnelActive) return
    updateWifiLock(screenOn)
  }

  @SuppressLint("WakelockTimeout")
  private fun acquireWakeLock() {
    try {
      if (wake?.isHeld != true) {
        val power = context.getSystemService(PowerManager::class.java) ?: return
        wake = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "UniRay:tunnel").apply {
          setReferenceCounted(false)
          acquire()
        }
      }
    } catch (e: Exception) {
      Log.w(TAG, "cannot acquire wake lock", e)
    }
  }

  private fun releaseWakeLock() {
    try {
      wake?.takeIf { it.isHeld }?.release()
    } catch (e: Exception) {
      Log.w(TAG, "releasing wake lock failed", e)
    }
    wake = null
  }

  private fun updateWifiLock(screenOn: Boolean) {
    try {
      wifi?.takeIf { it.isHeld }?.release()
      wifi = null
      val manager = context.applicationContext.getSystemService(WifiManager::class.java) ?: return
      // When screen is on: low latency / high perf gives best interactive response.
      // When screen is off: switch to standard WIFI_MODE_FULL so 802.11 power saving
      // between DTIM beacons is active, avoiding continuous radio drain while keeping connection alive.
      @Suppress("DEPRECATION")
      val mode = if (screenOn) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) WifiManager.WIFI_MODE_FULL_LOW_LATENCY
        else WifiManager.WIFI_MODE_FULL_HIGH_PERF
      } else {
        WifiManager.WIFI_MODE_FULL
      }
      wifi = manager.createWifiLock(mode, "UniRay:tunnel")?.apply {
        setReferenceCounted(false)
        acquire()
      }
    } catch (e: Exception) {
      Log.w(TAG, "updating wifi lock failed", e)
    }
  }

  private fun releaseWifiLock() {
    try {
      wifi?.takeIf { it.isHeld }?.release()
    } catch (e: Exception) {
      Log.w(TAG, "releasing wifi lock failed", e)
    }
    wifi = null
  }

  @Synchronized
  fun release() {
    isTunnelActive = false
    isWakeLockEnabled = false
    releaseWakeLock()
    releaseWifiLock()
  }

  companion object {
    private const val TAG = "UniRayAwake"
    private const val PREFS_NAME = "uniray_prefs"
    private const val KEY_BATTERY_EXEMPT_REQUESTED = "battery_exempt_requested"

    /** Whether Doze leaves the app's network alone (battery optimization off). */
    fun batteryExempt(context: Context): Boolean {
      val power = context.getSystemService(PowerManager::class.java) ?: return true
      return power.isIgnoringBatteryOptimizations(context.packageName)
    }

    fun hasRequestedBatteryExemption(context: Context): Boolean {
      val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
      return prefs.getBoolean(KEY_BATTERY_EXEMPT_REQUESTED, false)
    }

    /**
     * Asks the user to exempt the app from battery optimization.
     * By default prompts only once unless force is true.
     */
    @SuppressLint("BatteryLife")
    fun requestBatteryExemption(context: Context, force: Boolean = false) {
      if (batteryExempt(context)) return
      if (!force && hasRequestedBatteryExemption(context)) return

      val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
      prefs.edit().putBoolean(KEY_BATTERY_EXEMPT_REQUESTED, true).apply()

      val direct =
        android.content.Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:${context.packageName}"))
      val list = android.content.Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
      // The direct prompt needs REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, which
      // Google Play restricts: that edition opens the system's list instead.
      val intents = if (BuildConfig.PLAY_STORE) listOf(list) else listOf(direct, list)
      for (intent in intents) {
        try {
          if (context !is android.app.Activity) intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
          context.startActivity(intent)
          return
        } catch (e: Exception) {
          Log.w(TAG, "cannot open ${intent.action}", e)
        }
      }
    }
  }
}
