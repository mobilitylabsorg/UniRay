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

  @Synchronized
  fun set(enabled: Boolean) {
    if (enabled) acquire() else release()
  }

  // No timeout on purpose: it is held exactly as long as the tunnel is up,
  // and released when the tunnel stops.
  @SuppressLint("WakelockTimeout")
  private fun acquire() {
    try {
      if (wake?.isHeld != true) {
        val power = context.getSystemService(PowerManager::class.java) ?: return
        wake = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "UniRay:tunnel").apply {
          setReferenceCounted(false)
          acquire()
        }
      }
      if (wifi?.isHeld != true) {
        val manager = context.applicationContext.getSystemService(WifiManager::class.java)
        // HIGH_PERF keeps Wi-Fi out of power save with the screen off; from
        // Android 14 it does nothing and LOW_LATENCY is what is left.
        @Suppress("DEPRECATION")
        val mode =
          if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) WifiManager.WIFI_MODE_FULL_LOW_LATENCY
          else WifiManager.WIFI_MODE_FULL_HIGH_PERF
        wifi = manager?.createWifiLock(mode, "UniRay:tunnel")?.apply {
          setReferenceCounted(false)
          acquire()
        }
      }
    } catch (e: Exception) {
      Log.w(TAG, "cannot keep the device awake", e)
    }
  }

  private fun release() {
    try {
      wake?.takeIf { it.isHeld }?.release()
      wifi?.takeIf { it.isHeld }?.release()
    } catch (e: Exception) {
      Log.w(TAG, "releasing the wake locks failed", e)
    }
    wake = null
    wifi = null
  }

  companion object {
    private const val TAG = "UniRayAwake"

    /** Whether Doze leaves the app's network alone (battery optimization off). */
    fun batteryExempt(context: Context): Boolean {
      val power = context.getSystemService(PowerManager::class.java) ?: return true
      return power.isIgnoringBatteryOptimizations(context.packageName)
    }

    /**
     * Asks the user to exempt the app from battery optimization, the system
     * dialog. Without it Doze cuts the app's network off in deep sleep, wake
     * lock or not. Falls back to the battery optimization list where the
     * dialog is not there (some ROMs).
     */
    @SuppressLint("BatteryLife")
    fun requestBatteryExemption(context: Context) {
      if (batteryExempt(context)) return
      val direct =
        android.content.Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:${context.packageName}"))
      val list = android.content.Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
      for (intent in listOf(direct, list)) {
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
