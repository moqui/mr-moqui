package org.moqui.mobile

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.View
import android.view.WindowManager
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

/** Whether the app's activity ended for good (it finished, not made again for a configuration change): Rust ends the
 *  client and the process only then (src/lib.rs, through JNI). */
object ActivityEnd {
  @Volatile private var forGood = true

  fun note(activity: MainActivity) {
    forGood = activity.isFinishing && !activity.isChangingConfigurations
  }

  fun started() {
    forGood = false
  }

  @JvmStatic
  fun endedForGood(): Boolean = forGood
}

/**
 * A change of the phone's default network during a call (Wi-Fi to mobile data and back) tells the call's page, which
 * restarts ICE on the same connection, so the call goes on with no new join or ring (plans/Comms-Messaging.md §8.3
 * item 10; WebrootCall.js `moqui.call.networkChanged`). The first network, the one there at the start, is not a change.
 */
class NetworkWatch(private val webView: () -> WebView?) : ConnectivityManager.NetworkCallback() {
  @Volatile private var current: Network? = null

  override fun onAvailable(network: Network) {
    val was = current
    current = network
    if (was == null || was == network || !CallManager.inCall()) return
    Log.i(CallManager.TAG, "The network changed during a call: the page restarts ICE")
    // the views are the main thread's
    Handler(Looper.getMainLooper()).post {
      webView()?.evaluateJavascript(
        "window.moqui && moqui.call && moqui.call.networkChanged && moqui.call.networkChanged()", null
      )
    }
  }

  fun register(context: Context) {
    val cm = context.getSystemService(ConnectivityManager::class.java) ?: return
    try {
      cm.registerDefaultNetworkCallback(this)
    } catch (e: RuntimeException) {
      Log.w(CallManager.TAG, "No network callback: a call restarts ICE only when its page sees the change", e)
    }
  }

  fun unregister(context: Context) {
    val cm = context.getSystemService(ConnectivityManager::class.java) ?: return
    try {
      cm.unregisterNetworkCallback(this)
    } catch (e: RuntimeException) {
      // not registered
    }
  }
}

/** The activity's WebView, found in its views (the page the call is in). */
fun findWebView(view: View?): WebView? {
  if (view is WebView) return view
  if (view is android.view.ViewGroup) {
    for (i in 0 until view.childCount) findWebView(view.getChildAt(i))?.let { return it }
  }
  return null
}

class MainActivity : TauriActivity() {
  private val networkWatch = NetworkWatch { findWebView(window?.decorView) }

  override fun onCreate(savedInstanceState: Bundle?) {
    ActivityEnd.started()
    replayed(intent)
    // a call answered on the ring screen: the app shows over the lock screen until the call is over
    showOverLock(CallManager.ownsLink(intent?.data))
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    CallManager.know(this)
    CallManager.appScreenShown(this)
    networkWatch.register(applicationContext)
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
    replayed(intent)
    if (CallManager.ownsLink(intent.data)) showOverLock(true)
    super.onNewIntent(intent)
  }

  /** A call link the activity is started with again from the recent apps is not taken again (its nonce is gone
   *  anyway): it goes before tao reads the intent. */
  private fun replayed(intent: Intent?) {
    val data = intent?.data ?: return
    if (intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0 && data.scheme == "org.moqui.mobile" &&
      data.path == "/call"
    ) {
      intent.data = null
    }
  }

  override fun onResume() {
    super.onResume()
    askForCallsOnce()
  }

  override fun onStop() {
    super.onStop()
    // the outbox sent in the background (WorkManager)
    FlushWorker.background(this)
  }

  override fun onDestroy() {
    networkWatch.unregister(applicationContext)
    CallManager.appScreenShown(null)
    // before Rust hears of the end (super.onDestroy)
    ActivityEnd.note(this)
    // the process ends with a finished activity: the background send kept first
    if (isFinishing && !isChangingConfigurations) FlushWorker.background(this, ending = true)
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
