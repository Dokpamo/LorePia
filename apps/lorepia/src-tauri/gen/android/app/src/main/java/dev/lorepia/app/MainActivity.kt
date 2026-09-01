package dev.lorepia.app

import android.os.Bundle
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  private var appWebView: WebView? = null
  private lateinit var appBackCallback: OnBackPressedCallback

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    appBackCallback = object : OnBackPressedCallback(true) {
      override fun handleOnBackPressed() {
        val webView = appWebView
        if (webView == null) {
          moveTaskToBack(true)
          return
        }
        webView.evaluateJavascript(BACK_SCRIPT) { result ->
          if (result == "\"exit\"" || result == "null" || result == "undefined") {
            moveTaskToBack(true)
          }
        }
      }
    }
    onBackPressedDispatcher.addCallback(this, appBackCallback)
  }

  override fun onResume() {
    super.onResume()
    if (::appBackCallback.isInitialized) {
      appBackCallback.remove()
      onBackPressedDispatcher.addCallback(this, appBackCallback)
    }
  }

  override fun onWebViewCreate(webView: WebView) {
    appWebView = webView
  }

  companion object {
    private const val BACK_SCRIPT =
      "(() => { try { return window.__LOREPIA_ANDROID_BACK__?.() ?? 'exit'; } catch { return 'exit'; } })()"
  }
}
