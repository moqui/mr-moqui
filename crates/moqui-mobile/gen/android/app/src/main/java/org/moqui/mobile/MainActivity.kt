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
import androidx.activity.OnBackPressedCallback
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

/** What Android's back asks the page first: true when it closed something of its own (`moqui.back.pop()`). */
/** How long back waits for the page's answer before Tauri's own back goes on. */
private const val BACK_WAIT_MS = 400L

private const val BACK_SCRIPT =
  "(function(){try{return !!(window.moqui&&window.moqui.back&&window.moqui.back.pop());}catch(e){return false;}})()"

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
    // no touch through another app's window over the app (the overlay defence below Android 12, SEC-CLI-116)
    ObscuredTouches.refuse(window.decorView)
    CallManager.know(this)
    CallManager.appScreenShown(this)
    // screenshots blocked while a reason holds it (a view-once photo open): kept over a new activity
    ScreenSecurity.attach(this)
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

  /** A call or dial link the activity is started with again from the recent apps is not taken again (its nonce is gone
   *  anyway): it goes before tao reads the intent. */
  private fun replayed(intent: Intent?) {
    val data = intent?.data ?: return
    if (intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0 && data.scheme == "org.moqui.mobile" &&
      (data.path == "/call" || data.path == "/dial")
    ) {
      intent.data = null
      return
    }
    // a number from the phone's dialer came: its notification, if one was shown, goes
    if (data.scheme == "org.moqui.mobile" && data.path == "/dial") DialAccount.handedOver(this)
  }

  override fun onResume() {
    super.onResume()
    ChatNotifications.foreground = true
    // the screen off at the ear follows the app coming to the front
    CallAudio.changed(null)
    // the contacts allowed in Android's settings while the app was away: the page names its phone rooms again
    PhoneBook.check(this)
    askForCallsOnce()
    tellForceStopOnce()
  }

  override fun onPause() {
    ChatNotifications.foreground = false
    // and the app going to the back: the lock goes
    CallAudio.changed(null)
    super.onPause()
  }

  /** The system's permission dialog has gone: the full-screen intent's question may come now. */
  override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
    super.onRequestPermissionsResult(requestCode, permissions, grantResults)
    if (requestCode == ASK_CALLS) askForFullScreenOnce()
    if (requestCode == PhoneBook.ASK_CONTACTS) PhoneBook.answered(this)
  }

  private companion object {
    /** The request code of what calls and notifications need ([askForCallsOnce]). */
    const val ASK_CALLS = 2
  }

  /**
   * The user force-stopped the app since it last said so (Android 11+): no push reached it until now, which it says once
   * (plans/Comms-Messaging.md §7.4, After reboot and update).
   */
  private fun tellForceStopOnce() {
    val stopped = PushCheck.forceStoppedDate(this)
    if (stopped <= 0) return
    val prefs = getSharedPreferences("moqui-push", MODE_PRIVATE)
    if (prefs.getLong("forceStopTold", 0L) >= stopped) return
    prefs.edit().putLong("forceStopTold", stopped).apply()
    if (prefs.getLong("forceStopSeen", 0L) == 0L) {
      // the first look: an earlier stop is not news to tell
      prefs.edit().putLong("forceStopSeen", stopped).apply()
      return
    }
    ObscuredTouches.show(
      android.app.AlertDialog.Builder(this)
        .setTitle(R.string.force_stopped_title)
        .setMessage(R.string.force_stopped_reason)
        .setPositiveButton(R.string.ask_continue, null)
    )
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

  /**
   * Android's back asks the page first (docs/ui/Mobile-Client.md "Back"): a dialog, menu, sheet, pane or room it
   * opened, then the shell's own history. When the page closes nothing, Tauri's own back (its app plugin: the webview's
   * history, then [onBackPressed]) goes on. Added once the webview is made, after Tauri's callback (made with the
   * plugins, before the webview), so the dispatcher asks this one first; with the activity as its owner it is added
   * again after Tauri's at each start too.
   */
  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    val main = Handler(Looper.getMainLooper())
    val callback = object : OnBackPressedCallback(true) {
      // Tauri's back, past this callback
      fun onwards() {
        // in a video call on its call screen, Back past the page takes the call to picture-in-picture, not away
        if (CallManager.pipWanted() && enterPip()) return
        isEnabled = false
        onBackPressedDispatcher.onBackPressed()
        isEnabled = true
      }

      override fun handleOnBackPressed() {
        // one answer: the page's, or after BACK_WAIT_MS Tauri's back (a page that does not answer); a late answer is dropped
        var settled = false
        val timeout = Runnable { if (!settled) { settled = true; onwards() } }
        main.postDelayed(timeout, BACK_WAIT_MS)
        webView.evaluateJavascript(BACK_SCRIPT) { answer ->
          if (settled) return@evaluateJavascript
          settled = true
          main.removeCallbacks(timeout)
          if (answer != "true") onwards()
        }
      }
    }
    onBackPressedDispatcher.addCallback(this, callback)
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

  // ---- picture-in-picture (plans/Comms-Messaging.md §8.2 Leave the app, §8.3 item 4) ----

  /** The window's actions in picture-in-picture: Mute and Hang up the call the app is in. */
  private fun pipActions(): List<android.app.RemoteAction> {
    val call = CallManager.activeCall() ?: return emptyList()
    fun action(icon: Int, title: Int, what: String): android.app.RemoteAction {
      val intent = android.content.Intent(this, CallActionReceiver::class.java).setAction(what).putExtra(CallManager.EXTRA_CALL, call.id)
      val pending = android.app.PendingIntent.getBroadcast(this, what.hashCode(), intent,
        android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT)
      val label = getString(title)
      return android.app.RemoteAction(android.graphics.drawable.Icon.createWithResource(this, icon), label, label, pending)
    }
    return listOf(
      action(android.R.drawable.ic_lock_silent_mode, R.string.pip_mute, CallService.ACTION_MUTE),
      action(android.R.drawable.ic_menu_close_clear_cancel, R.string.pip_hang_up, CallService.ACTION_HANG_UP),
    )
  }

  private fun pipParams(): android.app.PictureInPictureParams {
    val builder = android.app.PictureInPictureParams.Builder().setAspectRatio(android.util.Rational(3, 4)).setActions(pipActions())
    // Android 12+: the system takes the app to picture-in-picture by itself when the user leaves it in a video call
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) builder.setAutoEnterEnabled(CallManager.pipWanted())
    return builder.build()
  }

  /** The call's state changed ([CallManager]): picture-in-picture is offered while a video call shows; the call over,
   *  the small window goes. */
  fun updatePip() {
    try {
      setPictureInPictureParams(pipParams())
    } catch (err: Exception) {
      Log.d(CallManager.TAG, "No picture-in-picture: ${err.message}")
    }
    // the small window goes with the last joined call, not when the only call goes on hold
    if (isInPictureInPictureMode && !CallRules.pipStays(CallManager.calls().count { it.joined })) moveTaskToBack(false)
  }

  private fun enterPip(): Boolean = try {
    enterPictureInPictureMode(pipParams())
  } catch (err: Exception) {
    Log.w(CallManager.TAG, "Picture-in-picture did not start: ${err.message}")
    false
  }

  /** Before Android 12: the user leaves the app (Home, Recents) during a video call: it goes to picture-in-picture. */
  override fun onUserLeaveHint() {
    super.onUserLeaveHint()
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S && CallManager.pipWanted()) enterPip()
  }

  /** In or out of picture-in-picture: the call screen shows only the video while in it (`moqui.call.setHostPip`). */
  override fun onPictureInPictureModeChanged(isInPictureInPictureMode: Boolean, newConfig: android.content.res.Configuration) {
    super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
    Native.tellPage("pip", isInPictureInPictureMode.toString())
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
          // the reason first (plans/Comms-Messaging.md §7.4, Permissions), then the system's question; "Not now" leaves
          // out only the notifications. The full-screen intent's question comes after the system's dialog has gone
          // (onRequestPermissionsResult), never over it.
          if (Manifest.permission.POST_NOTIFICATIONS in missing) {
            val others = missing - Manifest.permission.POST_NOTIFICATIONS
            var asked = false
            ObscuredTouches.show(
              android.app.AlertDialog.Builder(this)
                .setTitle(R.string.ask_notifications_title)
                .setMessage(R.string.ask_notifications_reason)
                .setPositiveButton(R.string.ask_continue) { _, _ ->
                  asked = true
                  requestPermissions(missing.toTypedArray(), ASK_CALLS)
                }
                .setNegativeButton(R.string.ask_not_now) { _, _ ->
                  asked = true
                  if (others.isNotEmpty()) requestPermissions(others.toTypedArray(), ASK_CALLS) else askForFullScreenOnce()
                }
                .setOnDismissListener { if (!asked && others.isNotEmpty()) requestPermissions(others.toTypedArray(), ASK_CALLS) }
            )
          } else {
            requestPermissions(missing.toTypedArray(), ASK_CALLS)
          }
        }
      }
    }
  }

  /**
   * Android 14+ lets the user turn off the full-screen intent a call's ring shows over the lock screen with: asked
   * once, with the reason, by opening its setting (there is no run-time dialog for it).
   */
  private fun askForFullScreenOnce() {
    if (Build.VERSION.SDK_INT < 34) return
    val notifications = getSystemService(android.app.NotificationManager::class.java) ?: return
    if (notifications.canUseFullScreenIntent()) return
    val prefs = getSharedPreferences("moqui-calls", MODE_PRIVATE)
    if (prefs.getBoolean("askedForFullScreen", false)) return
    prefs.edit().putBoolean("askedForFullScreen", true).apply()
    ObscuredTouches.show(
      android.app.AlertDialog.Builder(this)
        .setTitle(R.string.ask_full_screen_title)
        .setMessage(R.string.ask_full_screen_reason)
        .setPositiveButton(R.string.ask_open_settings) { _, _ -> PushCheck.fix(this, "fullScreen") }
        .setNegativeButton(R.string.ask_not_now, null)
    )
  }
}
