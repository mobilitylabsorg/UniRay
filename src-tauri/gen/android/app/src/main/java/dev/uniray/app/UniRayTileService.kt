package dev.uniray.app

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.graphics.drawable.Icon
import android.net.VpnService
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.VibrationEffect
import android.os.Vibrator
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import android.util.Log
import androidx.core.content.ContextCompat
import java.io.File
import java.util.UUID
import org.json.JSONArray
import org.json.JSONObject

/**
 * Quick Settings tile for UniRay: one-tap toggle for the VPN tunnel
 * straight from the Android notification shade.
 */
class UniRayTileService : TileService() {
  companion object {
    private const val TAG = "UniRayTile"
    const val ACTION_VPN_STATE_CHANGED = "dev.uniray.app.action.VPN_STATE_CHANGED"

    fun requestUpdate(context: Context) {
      try {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
          requestListeningState(context, ComponentName(context, UniRayTileService::class.java))
        }
      } catch (e: Exception) {
        Log.w(TAG, "Failed to request tile update", e)
      }
    }
  }

  private val mainHandler = Handler(Looper.getMainLooper())
  private var isTransitioning = false
  private var transitionPollRunnable: Runnable? = null
  private var isReceiverRegistered = false

  private val stateReceiver = object : BroadcastReceiver() {
    override fun onReceive(context: Context?, intent: Intent?) {
      Log.d(TAG, "VPN state broadcast received")
      stopTransitionPolling()
      updateTileState()
    }
  }

  override fun onTileAdded() {
    super.onTileAdded()
    updateTileState()
  }

  override fun onStartListening() {
    super.onStartListening()
    Log.d(TAG, "onStartListening()")
    registerStateReceiver()
    if (!isTransitioning) {
      updateTileState()
    }
  }

  override fun onStopListening() {
    super.onStopListening()
    Log.d(TAG, "onStopListening()")
    stopTransitionPolling()
    unregisterStateReceiver()
  }

  override fun onClick() {
    super.onClick()
    vibrate()

    val running = VpnState.isRunning(this)
    Log.d(TAG, "Tile clicked: isRunning=$running, isTransitioning=$isTransitioning")

    if (running) {
      disconnect()
    } else {
      connect()
    }
  }

  private fun vibrate() {
    try {
      val vibrator = getSystemService(Vibrator::class.java) ?: return
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        vibrator.vibrate(VibrationEffect.createPredefined(VibrationEffect.EFFECT_CLICK))
      } else {
        @Suppress("DEPRECATION")
        vibrator.vibrate(40L)
      }
    } catch (_: Exception) {}
  }

  private fun registerStateReceiver() {
    if (isReceiverRegistered) return
    try {
      val filter = IntentFilter(ACTION_VPN_STATE_CHANGED)
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        registerReceiver(stateReceiver, filter, Context.RECEIVER_NOT_EXPORTED)
      } else {
        registerReceiver(stateReceiver, filter)
      }
      isReceiverRegistered = true
    } catch (e: Exception) {
      Log.w(TAG, "Failed to register state receiver", e)
    }
  }

  private fun unregisterStateReceiver() {
    if (!isReceiverRegistered) return
    try {
      unregisterReceiver(stateReceiver)
    } catch (_: Exception) {}
    isReceiverRegistered = false
  }

  private fun updateTileState() {
    val tile = qsTile ?: return
    val running = VpnState.isRunning(this)
    Log.d(TAG, "updateTileState(): running=$running")

    tile.label = getString(R.string.qs_tile_label)
    tile.icon = Icon.createWithResource(this, R.drawable.ic_vpn_key)

    if (running) {
      tile.state = Tile.STATE_ACTIVE
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        val server = getServerName() ?: getString(R.string.vpn_connected)
        tile.subtitle = server
      }
    } else {
      tile.state = Tile.STATE_INACTIVE
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        tile.subtitle = getString(R.string.qs_tile_disconnected)
      }
    }

    tile.updateTile()
  }

  private fun getServerName(): String? {
    val vpnSpec = UniRayVpnService.specFile(this)
    val stealthSpec = StealthService.specFile(this)
    val file = when {
      stealthSpec.exists() && vpnSpec.exists() -> {
        if (stealthSpec.lastModified() > vpnSpec.lastModified()) stealthSpec else vpnSpec
      }
      stealthSpec.exists() -> stealthSpec
      vpnSpec.exists() -> vpnSpec
      else -> null
    } ?: return null

    return try {
      JSONObject(file.readText()).optString("serverName").takeIf { it.isNotEmpty() }
    } catch (_: Exception) {
      null
    }
  }

  private fun connect() {
    val vpnSpec = UniRayVpnService.specFile(this)
    val stealthSpec = StealthService.specFile(this)

    if (!vpnSpec.exists() && !stealthSpec.exists()) {
      launchApp()
      return
    }

    val useStealth = stealthSpec.exists() && (!vpnSpec.exists() || stealthSpec.lastModified() > vpnSpec.lastModified())

    if (!useStealth) {
      val consent = VpnService.prepare(this)
      if (consent != null) {
        launchApp()
        return
      }
    }

    // Set connecting status immediately on tile
    isTransitioning = true
    val tile = qsTile
    if (tile != null) {
      tile.state = Tile.STATE_INACTIVE
      tile.label = getString(R.string.qs_tile_label)
      tile.icon = Icon.createWithResource(this, R.drawable.ic_vpn_key)
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        tile.subtitle = getString(R.string.qs_tile_connecting)
      }
      tile.updateTile()
    }

    val activeSpec = if (useStealth) stealthSpec else vpnSpec
    syncSessionFile(activeSpec)

    val request = UUID.randomUUID().toString()
    try {
      if (useStealth) {
        val intent = Intent(this, StealthService::class.java)
          .setAction(StealthService.ACTION_START)
          .putExtra(StealthService.EXTRA_REQUEST, request)
        ContextCompat.startForegroundService(this, intent)
      } else {
        val intent = Intent(this, UniRayVpnService::class.java)
          .setAction(UniRayVpnService.ACTION_START)
          .putExtra(UniRayVpnService.EXTRA_REQUEST, request)
        ContextCompat.startForegroundService(this, intent)
      }
    } catch (e: Exception) {
      Log.e(TAG, "Failed to start service from tile", e)
      isTransitioning = false
      updateTileState()
      launchApp()
      return
    }

    startTransitionPolling(targetRunning = true)
  }

  private fun disconnect() {
    // Set disconnecting status immediately on tile
    isTransitioning = true
    val tile = qsTile
    if (tile != null) {
      tile.state = Tile.STATE_INACTIVE
      tile.label = getString(R.string.qs_tile_label)
      tile.icon = Icon.createWithResource(this, R.drawable.ic_vpn_key)
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        tile.subtitle = getString(R.string.qs_tile_disconnecting)
      }
      tile.updateTile()
    }

    val targets = VpnState.tunnelsToStop(this)
    val request = UUID.randomUUID().toString()

    for (service in targets) {
      val action = if (service == StealthService::class.java) StealthService.ACTION_STOP else UniRayVpnService.ACTION_STOP
      val intent = Intent(this, service)
        .setAction(action)
        .putExtra(UniRayVpnService.EXTRA_REQUEST, request)
      try {
        startService(intent)
      } catch (e: Exception) {
        Log.e(TAG, "Failed to stop service: $service", e)
      }
    }

    clearSessionFiles()
    startTransitionPolling(targetRunning = false)
  }

  private fun startTransitionPolling(targetRunning: Boolean) {
    stopTransitionPolling()
    isTransitioning = true
    val startTime = System.currentTimeMillis()
    val runnable = object : Runnable {
      override fun run() {
        val currentRunning = VpnState.isRunning(this@UniRayTileService)
        Log.d(TAG, "transition polling: current=$currentRunning, target=$targetRunning")
        if (currentRunning == targetRunning || System.currentTimeMillis() - startTime > 10000L) {
          stopTransitionPolling()
          updateTileState()
        } else {
          mainHandler.postDelayed(this, 250L)
        }
      }
    }
    transitionPollRunnable = runnable
    mainHandler.postDelayed(runnable, 250L)
  }

  private fun stopTransitionPolling() {
    transitionPollRunnable?.let { mainHandler.removeCallbacks(it) }
    transitionPollRunnable = null
    isTransitioning = false
  }

  private fun launchApp() {
    if (isLocked) {
      unlockAndRun { openApp() }
    } else {
      openApp()
    }
  }

  private fun openApp() {
    val intent = Intent(this, MainActivity::class.java).apply {
      flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
    }
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      val pending = PendingIntent.getActivity(
        this,
        0,
        intent,
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
      )
      startActivityAndCollapse(pending)
    } else {
      @Suppress("DEPRECATION")
      startActivityAndCollapse(intent)
    }
  }

  private fun syncSessionFile(specFile: File) {
    try {
      val spec = JSONObject(specFile.readText())
      val serverName = spec.optString("serverName")
      val metricsSocket = spec.optString("metricsSocket")
      val proxyTags = spec.optJSONArray("proxyTags") ?: JSONArray()

      // The spec names the server it runs; an older spec (from before the id
      // was saved in it) only has the name, which the app then does not find
      // and simply shows no server as connected.
      val serverId = spec.optString("serverId").ifEmpty { serverName }

      val sessionJson = JSONObject().apply {
        put("serverId", serverId)
        put("since", System.currentTimeMillis() / 1000L)
        put("metricsSocket", metricsSocket)
        put("proxyTags", proxyTags)
      }

      val dirs = listOf(
        File(filesDir.parentFile, "run"),
        File(filesDir, "run"),
      )
      for (dir in dirs) {
        dir.mkdirs()
        File(dir, "session.json").writeText(sessionJson.toString())
      }
    } catch (e: Exception) {
      Log.w(TAG, "syncSessionFile error", e)
    }
  }

  private fun clearSessionFiles() {
    try {
      val dirs = listOf(
        File(filesDir.parentFile, "run"),
        File(filesDir, "run"),
      )
      for (dir in dirs) {
        File(dir, "session.json").delete()
      }
    } catch (_: Exception) {}
  }
}
