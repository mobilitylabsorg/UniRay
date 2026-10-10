package dev.uniray.app

import android.content.res.Configuration
import android.os.Bundle
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    SystemTheme.applyBars(this)
  }

  // The Back key (a TV remote's above all) first closes what the page has
  // open - a sheet, then the current tab (src/lib/backStack.ts); with
  // nothing left there the app goes to the background, the tunnel running
  // on. Added after wry's own callback, so it is asked first.
  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    onBackPressedDispatcher.addCallback(
      this,
      object : OnBackPressedCallback(true) {
        override fun handleOnBackPressed() {
          webView.evaluateJavascript("window.__unirayBack ? window.__unirayBack() : false") { handled ->
            if (handled != "true") moveTaskToBack(true)
          }
        }
      },
    )
  }

  // The manifest handles uiMode changes itself, so a switch between light and
  // dark arrives here instead of recreating the activity.
  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    SystemTheme.applyBars(this)
  }
}
