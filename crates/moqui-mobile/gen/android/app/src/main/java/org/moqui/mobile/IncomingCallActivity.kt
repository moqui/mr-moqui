// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.Manifest
import android.app.Activity
import android.app.KeyguardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.os.Build
import android.os.Bundle
import android.util.TypedValue
import android.view.Gravity
import android.view.WindowManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView

/**
 * The ring on the screen, over the lock screen (the ring notification's full-screen intent, or its Answer): who calls,
 * Answer and Decline, native and with no WebView. Answer asks for the microphone (and the camera for a video call)
 * here, where the user acts, then opens the app at the call ([CallManager.answer]). A WebView too old for encrypted
 * calls is named, since those fail closed in the page.
 */
class IncomingCallActivity : Activity() {
    private var id: String? = null
    private var answering = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        CallManager.know(this)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            setShowWhenLocked(true)
            setTurnScreenOn(true)
        } else {
            @Suppress("DEPRECATION")
            window.addFlags(WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED or WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON)
        }
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        show(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        show(intent)
    }

    override fun onDestroy() {
        CallManager.ringScreenShown(null)
        super.onDestroy()
    }

    private fun show(intent: Intent) {
        val call = CallManager.call(intent.getStringExtra(CallManager.EXTRA_CALL))
        if (call == null || call.state != CallManager.State.RINGING) {
            finish()
            return
        }
        id = call.id
        CallManager.ringScreenShown(this)
        setContentView(layout(call))
        if (intent.action == CallService.ACTION_ANSWER) answer()
    }

    private fun dp(value: Int) = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, value.toFloat(), resources.displayMetrics).toInt()

    private fun layout(call: CallManager.Call): LinearLayout {
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
            setBackgroundColor(Color.rgb(18, 24, 33))
            setPadding(dp(24), dp(96), dp(24), dp(48))
        }
        fun text(value: String, size: Float, color: Int, bold: Boolean = false) = TextView(this).apply {
            text = value
            setTextColor(color)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, size)
            gravity = Gravity.CENTER
            if (bold) typeface = Typeface.DEFAULT_BOLD
        }
        column.addView(text(if (call.video) "Incoming video call" else "Incoming call", 18f, Color.rgb(170, 184, 200)))
        column.addView(text(call.caller, 32f, Color.WHITE, true).apply { setPadding(0, dp(16), 0, dp(8)) })
        if (call.title.isNotBlank() && call.title != call.caller) column.addView(text(call.title, 16f, Color.rgb(170, 184, 200)))
        val webView = CallManager.webViewMajor()
        if (webView in 1 until CallManager.ENCRYPTED_CALLS_WEBVIEW) {
            column.addView(
                text(
                    "This phone's Android System WebView ($webView) cannot join end-to-end encrypted calls: they need " +
                        "${CallManager.ENCRYPTED_CALLS_WEBVIEW} or later. Update it from the Play Store.",
                    14f, Color.rgb(255, 196, 120)
                ).apply { setPadding(0, dp(24), 0, 0) }
            )
        }
        column.addView(LinearLayout(this), LinearLayout.LayoutParams(1, 0, 1f))
        val buttons = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER
        }
        fun button(label: String, color: Int, action: () -> Unit) = Button(this).apply {
            text = label
            setTextColor(Color.WHITE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 18f)
            isAllCaps = false
            background = GradientDrawable().apply {
                shape = GradientDrawable.RECTANGLE
                cornerRadius = dp(32).toFloat()
                setColor(color)
            }
            setOnClickListener { action() }
        }
        val size = LinearLayout.LayoutParams(dp(140), dp(64)).apply { setMargins(dp(16), 0, dp(16), 0) }
        buttons.addView(button("Decline", Color.rgb(211, 47, 47)) { decline() }.apply { contentDescription = "Decline" }, size)
        buttons.addView(button("Answer", Color.rgb(46, 125, 50)) { answer() }.apply { contentDescription = "Answer" }, size)
        column.addView(buttons)
        return column
    }

    private fun decline() {
        id?.let { CallManager.decline(this, it) }
        finish()
    }

    /**
     * The microphone (and the camera for a video call) asked for first, where the user acts, when the phone is not
     * locked: Android's permission dialog asks for the unlock first, so on a locked phone the app opens at once and
     * the page asks once the user unlocks. The app asks for the microphone with the notifications after sign-in
     * (MainActivity), so this is seldom needed.
     */
    private fun answer() {
        val call = CallManager.call(id) ?: return finish()
        if (answering) return
        answering = true
        val wanted = mutableListOf(Manifest.permission.RECORD_AUDIO)
        if (call.video) wanted.add(Manifest.permission.CAMERA)
        val missing = wanted.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        val locked = getSystemService(KeyguardManager::class.java)?.isKeyguardLocked ?: false
        if (missing.isEmpty() || locked) {
            answered()
        } else {
            requestPermissions(missing.toTypedArray(), 1)
        }
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        // a call with no microphone still opens: the page says what it lacks
        answered()
    }

    private fun answered() {
        id?.let { CallManager.answer(this, this, it) }
        finish()
    }
}
