package dev.uniray.app

import android.content.res.Configuration
import android.os.Bundle

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    SystemTheme.applyBars(this)
  }

  // The manifest handles uiMode changes itself, so a switch between light and
  // dark arrives here instead of recreating the activity.
  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    SystemTheme.applyBars(this)
  }
}
