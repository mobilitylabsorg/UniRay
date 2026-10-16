package dev.uniray.app

import android.content.Intent
import android.content.res.Configuration
import android.os.Bundle
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback

class MainActivity : TauriActivity() {
  private var currentWebView: WebView? = null
  private var pendingUri: String? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    SystemTheme.applyBars(this)
    handleIntent(intent)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    handleIntent(intent)
  }

  private fun handleIntent(intent: Intent?) {
    val uri = intent?.data?.toString() ?: return
    val webView = currentWebView
    if (webView != null) {
      val script = "window.__unirayHandleUri && window.__unirayHandleUri('$uri')"
      webView.post { webView.evaluateJavascript(script, null) }
    } else {
      pendingUri = uri
    }
  }

  // The Back key (a TV remote's above all) first closes what the page has
  // open - a sheet, then the current tab (src/lib/backStack.ts); with
  // nothing left there the app goes to the background, the tunnel running
  // on. Added after wry's own callback, so it is asked first.
  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    currentWebView = webView
    pendingUri?.let { uri ->
      val script = "window.__unirayHandleUri && window.__unirayHandleUri('$uri')"
      webView.postDelayed({ webView.evaluateJavascript(script, null) }, 600)
      pendingUri = null
    }
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

  // When activity goes to background, pause WebView timers and rendering loops
  // so the UI process consumes 0% CPU while tunnel is active in the background.
  override fun onPause() {
    super.onPause()
    currentWebView?.let {
      it.onPause()
      it.pauseTimers()
    }
  }

  override fun onResume() {
    super.onResume()
    currentWebView?.let {
      it.resumeTimers()
      it.onResume()
    }
  }

  // The manifest handles uiMode changes itself, so a switch between light and
  // dark arrives here instead of recreating the activity.
  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    SystemTheme.applyBars(this)
  }
}
