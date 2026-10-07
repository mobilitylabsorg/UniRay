package dev.uniray.app

import android.app.Activity
import android.content.Context
import android.content.res.Configuration
import android.os.Build
import androidx.core.view.WindowCompat

/**
 * Follows the system look: light or dark system bar icons, and the Material You
 * palette (Android 12+) mapped onto the web UI's color tokens.
 */
object SystemTheme {
  // The web UI's own colors (src/styles.css), used where there is no palette.
  private const val LIGHT_BG = 0xFFF4F5F7.toInt()
  private const val LIGHT_SURFACE = 0xFFFFFFFF.toInt()
  private const val DARK_BG = 0xFF0E1013.toInt()
  private const val DARK_SURFACE = 0xFF171A1F.toInt()

  fun isDark(context: Context): Boolean =
    context.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK ==
      Configuration.UI_MODE_NIGHT_YES

  /**
   * The system palette as the UI's tokens (`bg`, `surface`, `accent`, ...),
   * following Material 3's tonal roles; null before Android 12. Status colors
   * (danger, warning, good) are left to the UI: they carry meaning.
   */
  fun palette(context: Context, dark: Boolean): Map<String, Int>? {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return null
    fun c(id: Int) = context.getColor(id)
    return if (dark) {
      mapOf(
        "bg" to c(android.R.color.system_neutral1_900),
        "surface" to c(android.R.color.system_neutral1_800),
        "raised" to c(android.R.color.system_neutral1_700),
        "line" to c(android.R.color.system_neutral2_700),
        "ink" to c(android.R.color.system_neutral1_100),
        "muted" to c(android.R.color.system_neutral2_200),
        "accent" to c(android.R.color.system_accent1_200),
        "accent-ink" to c(android.R.color.system_accent1_800),
        "accent-soft" to c(android.R.color.system_accent1_700),
      )
    } else {
      mapOf(
        "bg" to c(android.R.color.system_neutral2_50),
        "surface" to c(android.R.color.system_neutral1_10),
        "raised" to c(android.R.color.system_neutral2_100),
        "line" to c(android.R.color.system_neutral2_200),
        "ink" to c(android.R.color.system_neutral1_900),
        "muted" to c(android.R.color.system_neutral2_700),
        "accent" to c(android.R.color.system_accent1_600),
        "accent-ink" to c(android.R.color.system_accent1_0),
        "accent-soft" to c(android.R.color.system_accent1_100),
      )
    }
  }

  fun hex(color: Int): String = String.format("#%06X", 0xFFFFFF and color)

  /**
   * Dark icons on a light theme and light icons on a dark one. Before Android
   * 15 the bars are opaque, so they also take the page and navigation colors;
   * from 15 on the app draws under transparent bars and the page shows through.
   */
  fun applyBars(activity: Activity) {
    val dark = isDark(activity)
    val window = activity.window
    WindowCompat.getInsetsController(window, window.decorView).apply {
      isAppearanceLightStatusBars = !dark
      isAppearanceLightNavigationBars = !dark
    }
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.VANILLA_ICE_CREAM) {
      val palette = palette(activity, dark)
      @Suppress("DEPRECATION")
      window.statusBarColor = palette?.get("bg") ?: if (dark) DARK_BG else LIGHT_BG
      @Suppress("DEPRECATION")
      window.navigationBarColor = palette?.get("surface") ?: if (dark) DARK_SURFACE else LIGHT_SURFACE
    }
  }
}
