package org.moqui.mobile

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.View
import android.view.WindowManager
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    // a call answered on the ring screen: the app shows over the lock screen until the call is over
    showOverLock(isCallLink(intent))
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    CallManager.know(this)
    CallManager.appScreenShown(this)
    // the native code's library and context, then push (FCM, when the build has Firebase)
    Native.work {
      Native.ensure(applicationContext)
      Push.start(applicationContext)
    }
    // The pages (the shell, a server's app) are laid out for a window of their own: the webview stays clear of the
    // status and navigation bars and the keyboard, rather than under them.
    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime()
      )
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
  }

  override fun onNewIntent(intent: Intent) {
    if (isCallLink(intent)) showOverLock(true)
    super.onNewIntent(intent)
  }

  override fun onResume() {
    super.onResume()
    askForCallsOnce()
  }

  override fun onDestroy() {
    CallManager.appScreenShown(null)
    super.onDestroy()
  }

  @Deprecated("Android's back, past the webview's own history")
  override fun onBackPressed() {
    // in a call the app goes to the background rather than end (its activity's end ends the process, and the call)
    if (CallManager.inCall()) {
      moveTaskToBack(true)
      return
    }
    @Suppress("DEPRECATION")
    super.onBackPressed()
  }

  private fun isCallLink(intent: Intent?): Boolean {
    val data = intent?.data ?: return false
    // an answered call, not the call notification's Hang up
    return data.scheme == "org.moqui.mobile" && data.path == "/call" && data.getQueryParameter("hangUp") != "true"
  }

  /** Show over the lock screen and turn the screen on (a call), or not (the call is over). */
  fun showOverLock(on: Boolean) {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
      setShowWhenLocked(on)
      setTurnScreenOn(on)
    } else {
      @Suppress("DEPRECATION")
      val flags = WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED or WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON
      if (on) window.addFlags(flags) else window.clearFlags(flags)
    }
  }

  /**
   * What calls need, asked once an account is signed in (the ring is that account's), not at the first start's
   * sign-in screen: Android 13+'s notifications, without which a ring is not shown, and the microphone, which a
   * permission dialog could not ask for on a locked phone at the Answer (it asks for the unlock first).
   */
  private fun askForCallsOnce() {
    val wanted = mutableListOf(Manifest.permission.RECORD_AUDIO)
    if (Build.VERSION.SDK_INT >= 33) wanted.add(Manifest.permission.POST_NOTIFICATIONS)
    val missing = wanted.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
    if (missing.isEmpty()) return
    val prefs = getSharedPreferences("moqui-calls", MODE_PRIVATE)
    if (prefs.getBoolean("askedForCalls", false)) return
    val dir = Native.dataDir(this)
    val main = Handler(Looper.getMainLooper())
    Native.work {
      Native.ensure(applicationContext)
      if (Native.hasAccount(dir)) {
        main.post {
          if (isFinishing || isDestroyed) return@post
          prefs.edit().putBoolean("askedForCalls", true).apply()
          Log.i(CallManager.TAG, "Asking for what calls need: $missing")
          requestPermissions(missing.toTypedArray(), 2)
        }
      }
    }
  }
}
