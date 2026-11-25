// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.util.DisplayMetrics
import android.util.Log
import android.view.Display
import android.view.Surface
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import org.json.JSONObject

/**
 * The native screen share (plans/archive/Comms-Whiteboard.md §3.6, plans/archive/MM-Spike.md): the system's consent each time
 * ([ScreenConsentActivity]), a foreground service of type mediaProjection ([ScreenShareService], which Android 14 needs
 * before the projection is taken), the projection into a VirtualDisplay drawn on a MediaCodec VP8 encoder's input
 * surface, and each encoded frame to Rust (`Native.screenFrame`), whose str0m leg sends it to the media server. Rust asks
 * for a keyframe (a new subscriber's PLI) and a bitrate (its bandwidth estimate); either side's end ends both.
 */
object ScreenShare {
    private const val TAG = "moqui-mobile"
    internal const val CHANNEL = "screen-share"
    /** Not the call's own (CallService 7301): both are shown while a call shares the screen. */
    internal const val NOTIFICATION_ID = 7302
    internal const val ACTION_STOP = "org.moqui.mobile.SCREEN_STOP"

    private const val FRAME_RATE = 15
    /** The encoder repeats the last frame after this long without a new one, so a still screen still answers a keyframe. */
    private const val REPEAT_US = 250_000L

    @Volatile private var projection: MediaProjection? = null
    @Volatile private var display: VirtualDisplay? = null
    @Volatile private var encoder: MediaCodec? = null
    @Volatile private var inputSurface: Surface? = null
    @Volatile private var current: Pair<Int, Int>? = null
    @Volatile private var bitrates: android.util.Range<Int>? = null
    private var displayListener: DisplayManager.DisplayListener? = null
    private var displayManager: DisplayManager? = null
    private var thread: HandlerThread? = null
    @Volatile private var appContext: Context? = null

    // what the spike measures (wall clock, ms)
    @Volatile private var askedMs = 0L
    @Volatile private var consentMs = 0L
    @Volatile private var firstFrameMs = 0L
    @Volatile private var frames = 0L
    @Volatile private var keyframes = 0L
    @Volatile private var bytes = 0L
    @Volatile private var codecName: String? = null
    @Volatile private var size = ""
    @Volatile private var bitrateBps = 0
    @Volatile private var lastKeyAskMs = 0L
    @Volatile private var lastKeyMs = 0L
    @Volatile private var state = "idle"

    /** Ask the system's consent (Rust's `screen_start`); the share starts when it is given. */
    @JvmStatic
    fun request(context: Context): Boolean {
        if (projection != null) return false
        consentMs = 0L
        firstFrameMs = 0L
        frames = 0L
        keyframes = 0L
        bytes = 0L
        lastKeyAskMs = 0L
        lastKeyMs = 0L
        askedMs = System.currentTimeMillis()
        state = "asking"
        val intent = Intent(context, ScreenConsentActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        context.startActivity(intent)
        return true
    }

    internal fun consented(context: Context, resultCode: Int, data: Intent?) {
        if (resultCode != Activity.RESULT_OK || data == null) {
            Log.i(TAG, "Screen share: consent refused")
            state = "refused"
            return
        }
        consentMs = System.currentTimeMillis()
        Log.i(TAG, "Screen share: consent given at $consentMs (${consentMs - askedMs} ms after asking)")
        val service = Intent(context, ScreenShareService::class.java)
            .putExtra(ScreenShareService.EXTRA_CODE, resultCode)
            .putExtra(ScreenShareService.EXTRA_DATA, data)
        ContextCompat.startForegroundService(context, service)
    }

    /** In the foreground service, once it is in the foreground: the projection, the encoder and the display. */
    internal fun begin(service: Service, resultCode: Int, data: Intent) {
        val manager = service.getSystemService(MediaProjectionManager::class.java)
        val mp = manager.getMediaProjection(resultCode, data) ?: run {
            Log.w(TAG, "Screen share: no projection")
            end(service)
            return
        }
        appContext = service.applicationContext
        val handlerThread = HandlerThread("screen-share").also { it.start() }
        thread = handlerThread
        val handler = Handler(handlerThread.looper)
        // Android 14: the callback is registered before the display is made; the system's stop ends the share, and a
        // captured content of a new size (a rotation, a folded screen) starts the encoder again at it
        mp.registerCallback(object : MediaProjection.Callback() {
            override fun onStop() {
                Log.i(TAG, "Screen share: the projection stopped")
                end(service)
            }

            override fun onCapturedContentResize(width: Int, height: Int) {
                resize(service, width, height)
            }
        }, handler)
        projection = mp
        val (screenW, screenH) = screenSize(service)
        val (width, height) = ScreenRules.captureSize(screenW, screenH)
        bitrateBps = START_BPS
        val (codec, surface) = startEncoder(service, width, height, handler)
        display = mp.createVirtualDisplay(
            "moqui-screen", width, height, service.resources.displayMetrics.densityDpi,
            DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, surface, null, handler
        )
        encoder = codec
        inputSurface = surface
        // before Android 14 a rotation is the default display's change, not the projection's
        val displays = service.getSystemService(DisplayManager::class.java)
        val listener = object : DisplayManager.DisplayListener {
            override fun onDisplayAdded(displayId: Int) {}
            override fun onDisplayRemoved(displayId: Int) {}
            override fun onDisplayChanged(displayId: Int) {
                if (displayId != Display.DEFAULT_DISPLAY) return
                val (w, h) = screenSize(service)
                resize(service, w, h)
            }
        }
        displays.registerDisplayListener(listener, handler)
        displayListener = listener
        displayManager = displays
        state = "sharing"
        Log.i(TAG, "Screen share: capturing $size with $codecName")
    }

    /** The default display's whole size, as the projection mirrors it. */
    private fun screenSize(context: Context): Pair<Int, Int> {
        val display = context.getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
            ?: return context.resources.displayMetrics.let { Pair(it.widthPixels, it.heightPixels) }
        val metrics = DisplayMetrics()
        @Suppress("DEPRECATION")
        display.getRealMetrics(metrics)
        return Pair(metrics.widthPixels, metrics.heightPixels)
    }

    /** A screen of a new size: the encoder again at the capture's new size, the display drawn on its surface (its first
     *  frame is a keyframe, which a VP8 decoder takes as the new size). On the share's thread. */
    private fun resize(service: Service, screenW: Int, screenH: Int) {
        val vd = display ?: return
        if (!ScreenRules.resized(current, screenW, screenH)) return
        val (width, height) = ScreenRules.captureSize(screenW, screenH)
        val handler = Handler(thread?.looper ?: return)
        val old = encoder
        val oldSurface = inputSurface
        try {
            val (codec, surface) = startEncoder(service, width, height, handler)
            vd.resize(width, height, service.resources.displayMetrics.densityDpi)
            vd.surface = surface
            encoder = codec
            inputSurface = surface
            Log.i(TAG, "Screen share: the screen is now $size")
        } catch (err: Throwable) {
            Log.e(TAG, "Screen share: the encoder did not start again at ${width}x$height", err)
            end(service)
        } finally {
            if (encoder !== old) {
                old?.let { stopCodec(it) }
                oldSurface?.release()
            }
        }
    }

    /** A VP8 encoder at `width` x `height` and its input surface, its frames to Rust. */
    private fun startEncoder(service: Service, width: Int, height: Int, handler: Handler): Pair<MediaCodec, Surface> {
        current = Pair(width, height)
        size = "${width}x$height"
        val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_VP8, width, height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, bitrateBps)
            setInteger(MediaFormat.KEY_BITRATE_MODE, MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
            setInteger(MediaFormat.KEY_FRAME_RATE, FRAME_RATE)
            // keyframes when asked (a new subscriber, a loss), and one a minute at most otherwise
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 60)
            setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, REPEAT_US)
            // the display draws at its refresh rate (60 on the emulator), which KEY_FRAME_RATE does not limit: the
            // surface drops what is over this, so a scrolling screen stays inside the bitrate
            if (Build.VERSION.SDK_INT >= 29) setFloat(MediaFormat.KEY_MAX_FPS_TO_ENCODER, FRAME_RATE.toFloat())
        }
        val name = MediaCodecList(MediaCodecList.REGULAR_CODECS).findEncoderForFormat(format)
        val codec = if (name != null) MediaCodec.createByCodecName(name) else MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_VP8)
        codecName = codec.name
        bitrates = codec.codecInfo.getCapabilitiesForType(MediaFormat.MIMETYPE_VIDEO_VP8).videoCapabilities?.bitrateRange
        codec.setCallback(object : MediaCodec.Callback() {
            override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {}

            override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
                try {
                    if (info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                        val buffer = codec.getOutputBuffer(index) ?: return
                        buffer.position(info.offset)
                        buffer.limit(info.offset + info.size)
                        val frame = ByteArray(info.size)
                        buffer.get(frame)
                        val now = System.currentTimeMillis()
                        if (frames == 0L) {
                            firstFrameMs = now
                            Log.i(TAG, "Screen share: first frame at $now (${now - consentMs} ms after consent), $codecName $size")
                        }
                        frames++
                        bytes += info.size
                        if (info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0) {
                            keyframes++
                            lastKeyMs = now
                        }
                        Native.screenFrame(frame, info.presentationTimeUs)
                    }
                } catch (err: Throwable) {
                    Log.w(TAG, "Screen share: a frame was dropped", err)
                } finally {
                    try {
                        codec.releaseOutputBuffer(index, false)
                    } catch (_: IllegalStateException) {
                        // an encoder stopped for a new size
                    }
                }
            }

            override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
                if (codec !== encoder) return
                Log.e(TAG, "Screen share: the encoder failed", e)
                end(service)
            }

            override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {}
        }, handler)
        codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        val surface = codec.createInputSurface()
        codec.start()
        return Pair(codec, surface)
    }

    private fun stopCodec(codec: MediaCodec) {
        try {
            codec.stop()
        } catch (_: IllegalStateException) {
        }
        codec.release()
    }

    /** A keyframe now (Rust: the leg connected, or the media server asked). */
    @JvmStatic
    fun keyframe() {
        lastKeyAskMs = System.currentTimeMillis()
        encoder?.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
    }

    /** The encoder's bitrate (Rust: the bandwidth estimate changed), within what it takes. */
    @JvmStatic
    fun bitrate(bps: Int) {
        val range = bitrates
        bitrateBps = ScreenRules.encoderBitrate(bps, range?.lower, range?.upper)
        encoder?.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_VIDEO_BITRATE, bitrateBps) })
    }

    /** The page ends the share. */
    @JvmStatic
    fun stop() {
        projection?.stop() ?: run { state = "idle" }
    }

    /** The leg's connection ended (Rust): the projection goes too. */
    @JvmStatic
    fun legEnded() {
        projection?.stop()
    }

    @JvmStatic
    fun state(): String = JSONObject().apply {
        put("state", state)
        put("codec", codecName)
        put("size", size)
        put("askedMs", askedMs)
        put("consentMs", consentMs)
        put("firstFrameMs", firstFrameMs)
        put("frames", frames)
        put("keyframes", keyframes)
        put("bytes", bytes)
        put("bitrate", bitrateBps)
        put("lastKeyAskMs", lastKeyAskMs)
        put("lastKeyMs", lastKeyMs)
    }.toString()

    /** Everything goes once: the display, the encoder, the projection, the leg and the service with its notification. */
    @Synchronized
    internal fun end(service: Service?) {
        val mp = projection
        projection = null
        if (mp != null) release(mp)
        service?.let {
            it.stopForeground(Service.STOP_FOREGROUND_REMOVE)
            it.stopSelf()
        }
        // the notification goes with the share whichever way it ended
        appContext?.getSystemService(NotificationManager::class.java)?.cancel(NOTIFICATION_ID)
    }

    private fun release(mp: MediaProjection) {
        display?.release()
        display = null
        encoder?.let {
            try {
                it.stop()
            } catch (_: IllegalStateException) {
            }
            it.release()
        }
        encoder = null
        inputSurface?.release()
        inputSurface = null
        current = null
        displayListener?.let { displayManager?.unregisterDisplayListener(it) }
        displayListener = null
        displayManager = null
        thread?.quitSafely()
        thread = null
        try {
            mp.stop()
        } catch (_: Throwable) {
        }
        state = "ended"
        Native.screenEnded()
        Log.i(TAG, "Screen share: ended after $frames frames ($keyframes key, $bytes bytes)")
    }

    private const val START_BPS = 1_200_000

    internal fun notification(context: Context): Notification {
        val manager = context.getSystemService(NotificationManager::class.java)
        if (manager.getNotificationChannel(CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL, "Screen sharing", NotificationManager.IMPORTANCE_LOW).apply {
                    description = "Your screen is shared in a call"
                }
            )
        }
        val stop = PendingIntent.getService(
            context, 0, Intent(context, ScreenShareService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )
        return NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_menu_slideshow)
            .setContentTitle("Sharing your screen")
            .setContentText("Everyone in the call sees your screen")
            .setOngoing(true)
            .addAction(0, "Stop sharing", stop)
            .build()
    }
}

/** The foreground service of type mediaProjection the capture runs in (Android 14 needs it before the projection). */
class ScreenShareService : Service() {
    companion object {
        const val EXTRA_CODE = "code"
        const val EXTRA_DATA = "data"
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ScreenShare.ACTION_STOP) {
            ScreenShare.end(this)
            return START_NOT_STICKY
        }
        val notification = ScreenShare.notification(this)
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(ScreenShare.NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
        } else {
            startForeground(ScreenShare.NOTIFICATION_ID, notification)
        }
        val code = intent?.getIntExtra(EXTRA_CODE, 0) ?: 0
        val data: Intent? = if (Build.VERSION.SDK_INT >= 33) {
            intent?.getParcelableExtra(EXTRA_DATA, Intent::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent?.getParcelableExtra(EXTRA_DATA)
        }
        if (data == null) {
            ScreenShare.end(this)
            stopSelf()
            return START_NOT_STICKY
        }
        try {
            ScreenShare.begin(this, code, data)
        } catch (err: Throwable) {
            Log.e("moqui-mobile", "Screen share: the capture did not start", err)
            ScreenShare.end(this)
            stopSelf()
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        ScreenShare.end(null)
        super.onDestroy()
    }
}
