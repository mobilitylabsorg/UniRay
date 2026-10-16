package dev.uniray.app

import android.app.Activity
import android.app.Service
import android.content.Context
import android.content.Intent
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.io.File

/**
 * The Google Play edition has no stealth mode: Play allows a device-wide
 * tunnel only through VpnService, and this build carries neither the root
 * tunnel nor libsu or Shizuku (src/full has them). The VPN service is the
 * only tunnel; the Rust side never asks for stealth here (cfg play_store).
 */
object Stealth {
  val service: Class<out Service>? = null

  const val ACTION_STOP = ""
  const val ACTION_NOTIFY = ""
  const val ACTION_KEEP_AWAKE = ""

  fun specFile(context: Context): File? = null

  fun startIntent(context: Context, request: String): Intent? = null

  fun privilegeState(activity: Activity, invoke: Invoke) {
    invoke.resolve(JSObject().put("level", "none").put("shizukuAvailable", false).put("shizukuGranted", false))
  }

  fun requestShizuku(activity: Activity, invoke: Invoke) = privilegeState(activity, invoke)

  fun start(plugin: VpnPlugin, activity: Activity, invoke: Invoke) {
    invoke.reject("not in this edition", "noRoot")
  }
}
