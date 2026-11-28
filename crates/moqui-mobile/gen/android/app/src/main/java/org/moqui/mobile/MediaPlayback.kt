// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.drawable.Icon
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.Executors

/**
 * A server page's media in the background (`moquiDesktop.media`, src/media.rs): what plays, kept by Rust's checks.
 * `start` starts [MediaPlaybackService], a foreground service of type mediaPlayback with a MediaSession and its media
 * notification (play, pause, back 10 s, forward 10 s), which keeps the process, and the WebView's audio, while the
 * app is in the background or the screen is off. `update` changes the notification; `stop` ends the service. The
 * notification's and the session's buttons go back to the page (`Native.mediaAction`).
 *
 * While the service runs, the activity does not pause the webview ([MainActivity.keepWebViewRunning]).
 */
object MediaPlayback {
    const val TAG = "moqui-media"
    /** The most bytes of an artwork image. */
    private const val ARTWORK_MAX = 4 * 1024 * 1024
    /** The longest side of the artwork kept. */
    private const val ARTWORK_SIDE = 512

    private val main = Handler(Looper.getMainLooper())
    private val loader = Executors.newSingleThreadExecutor { task -> Thread(task, "moqui-media-art") }

    /** What plays now: the page's last word, the main thread's. */
    class State {
        var title = ""
        var artist = ""
        var artworkUrl: String? = null
        var artwork: Bitmap? = null
        var playing = false
        /** milliseconds, at [at] */
        var position = 0L
        var at = SystemClock.elapsedRealtime()
        /** milliseconds, or -1 */
        var duration = -1L
        var rate = 1f
    }

    val state = State()

    /** Whether the service runs (the media plays or is paused, not stopped). */
    @Volatile var active = false
        internal set

    /** The service, while it runs. */
    internal var service: MediaPlaybackService? = null

    /**
     * A stop came after startForegroundService but before the service called startForeground. Android ends the app
     * when such a service stops before startForeground, so the service goes to the foreground first, then stops.
     */
    internal var stopWhenStarted = false

    /** The page's word (Rust's `media_session`, on its thread): `start`, `update` or `stop` with the checked JSON. */
    @JvmStatic
    fun fromPage(context: Context, action: String, json: String) {
        val app = context.applicationContext
        val given = try { JSONObject(json) } catch (err: Exception) { JSONObject() }
        main.post { apply(app, action, given) }
    }

    private fun apply(app: Context, action: String, given: JSONObject) {
        if (action == "stop") {
            if (active) {
                if (service?.foreground == true) app.stopService(Intent(app, MediaPlaybackService::class.java))
                else stopWhenStarted = true
            }
            active = false
            return
        }
        if (action == "update" && !active) return
        val s = state
        if (action == "start") {
            s.title = ""; s.artist = ""; s.artworkUrl = null; s.artwork = null; s.duration = -1L; s.rate = 1f
            s.position = 0L; s.at = SystemClock.elapsedRealtime(); s.playing = false
        }
        if (given.has("title")) s.title = given.optString("title")
        if (given.has("artist")) s.artist = given.optString("artist")
        if (given.has("playing")) s.playing = given.optBoolean("playing")
        if (given.has("position")) { s.position = (given.optDouble("position") * 1000).toLong(); s.at = SystemClock.elapsedRealtime() }
        if (given.has("duration")) s.duration = (given.optDouble("duration") * 1000).toLong()
        if (given.has("rate")) s.rate = given.optDouble("rate", 1.0).toFloat()
        val url = given.optString("artworkUrl", "").ifEmpty { null }
        if (url != null && url != s.artworkUrl) {
            s.artworkUrl = url
            s.artwork = null
            loadArtwork(url)
        }
        if (action == "start" && !active) {
            stopWhenStarted = false
            try {
                app.startForegroundService(Intent(app, MediaPlaybackService::class.java))
                active = true
            } catch (err: Exception) {
                // Android 12+ refuses a foreground service started from the background: the page started its media
                // with the app in front, so this is rare; the media then plays only while the app shows
                Log.w(TAG, "The media service did not start (${err.javaClass.simpleName})", err)
            }
        } else {
            service?.refresh()
        }
    }

    /** The artwork from its https URL (Rust checked it), at most [ARTWORK_MAX] bytes, scaled to [ARTWORK_SIDE]. */
    private fun loadArtwork(url: String) {
        loader.execute {
            val bitmap = try {
                val connection = URL(url).openConnection() as HttpURLConnection
                connection.connectTimeout = 10_000
                connection.readTimeout = 10_000
                connection.instanceFollowRedirects = false
                try {
                    if (connection.responseCode != 200) null else {
                        val bytes = ByteArrayOutputStream()
                        connection.inputStream.use { input ->
                            val buffer = ByteArray(16 * 1024)
                            while (true) {
                                val n = input.read(buffer)
                                if (n < 0) break
                                bytes.write(buffer, 0, n)
                                if (bytes.size() > ARTWORK_MAX) throw IllegalStateException("artwork too large")
                            }
                        }
                        decode(bytes.toByteArray())
                    }
                } finally {
                    connection.disconnect()
                }
            } catch (err: Exception) {
                Log.i(TAG, "No artwork: ${err.message}")
                null
            }
            if (bitmap != null) main.post {
                if (state.artworkUrl == url) {
                    state.artwork = bitmap
                    service?.refresh()
                }
            }
        }
    }

    private fun decode(data: ByteArray): Bitmap? {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(data, 0, data.size, bounds)
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
        var sample = 1
        while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= ARTWORK_SIDE) sample *= 2
        return BitmapFactory.decodeByteArray(data, 0, data.size, BitmapFactory.Options().apply { inSampleSize = sample })
    }

    /** A button of the notification or the session: the page hears it; `position` in milliseconds, or -1. */
    fun tellPage(action: String, position: Long = -1L) {
        Native.mediaActionSafe(action, if (position < 0) -1.0 else position / 1000.0)
    }

    /** Where the media is now, by the page's last place and the time since. */
    fun positionNow(): Long {
        val s = state
        if (!s.playing) return s.position
        val now = s.position + ((SystemClock.elapsedRealtime() - s.at) * s.rate).toLong()
        return if (s.duration > 0) minOf(now, s.duration) else now
    }
}

/** The mediaPlayback foreground service of [MediaPlayback]: its MediaSession and its notification. */
class MediaPlaybackService : Service() {
    companion object {
        private const val CHANNEL = "media-playback"
        private const val NOTIFICATION_ID = 7401
        private const val ACTION_PLAY = "org.moqui.mobile.media.PLAY"
        private const val ACTION_PAUSE = "org.moqui.mobile.media.PAUSE"
        private const val ACTION_BACK = "org.moqui.mobile.media.BACK"
        private const val ACTION_FORWARD = "org.moqui.mobile.media.FORWARD"
        private const val ACTION_STOP = "org.moqui.mobile.media.STOP"
    }

    private lateinit var session: MediaSession
    /** Whether startForeground ran (a stop before it must wait for it: MediaPlayback.stopWhenStarted). */
    @Volatile internal var foreground = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val manager = getSystemService(NotificationManager::class.java)
        if (manager.getNotificationChannel(CHANNEL) == null) {
            val channel = NotificationChannel(CHANNEL, getString(R.string.channel_media), NotificationManager.IMPORTANCE_LOW)
            channel.description = getString(R.string.channel_media_about)
            channel.setShowBadge(false)
            manager.createNotificationChannel(channel)
        }
        session = MediaSession(this, "moqui-media")
        session.setCallback(object : MediaSession.Callback() {
            override fun onPlay() = MediaPlayback.tellPage("play")
            override fun onPause() = MediaPlayback.tellPage("pause")
            override fun onStop() = MediaPlayback.tellPage("stop")
            override fun onSeekTo(pos: Long) = MediaPlayback.tellPage("seekto", pos)
            override fun onFastForward() = MediaPlayback.tellPage("seekforward")
            override fun onRewind() = MediaPlayback.tellPage("seekbackward")
            override fun onSkipToNext() = MediaPlayback.tellPage("seekforward")
            override fun onSkipToPrevious() = MediaPlayback.tellPage("seekbackward")
        })
        launchIntent()?.let { session.setSessionActivity(it) }
        session.isActive = true
        MediaPlayback.service = this
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_PLAY -> MediaPlayback.tellPage("play")
            ACTION_PAUSE -> MediaPlayback.tellPage("pause")
            ACTION_BACK -> MediaPlayback.tellPage("seekbackward")
            ACTION_FORWARD -> MediaPlayback.tellPage("seekforward")
            ACTION_STOP -> MediaPlayback.tellPage("stop")
        }
        try {
            val notification = build()
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK)
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }
            foreground = true
        } catch (err: Exception) {
            Log.w(MediaPlayback.TAG, "The media service is not in the foreground (${err.javaClass.simpleName})", err)
            MediaPlayback.active = false
            stopSelf()
            return START_NOT_STICKY
        }
        if (MediaPlayback.stopWhenStarted) {
            // the page stopped its media while the service was starting
            MediaPlayback.stopWhenStarted = false
            stopSelf()
            return START_NOT_STICKY
        }
        refreshSession()
        return START_NOT_STICKY
    }

    /** The app is swiped away from the recent apps: the media goes with it. */
    override fun onTaskRemoved(rootIntent: Intent?) {
        MediaPlayback.active = false
        stopSelf()
        super.onTaskRemoved(rootIntent)
    }

    override fun onDestroy() {
        foreground = false
        MediaPlayback.service = null
        MediaPlayback.active = false
        session.isActive = false
        session.release()
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }

    /** The page's new word: the session and the notification. */
    fun refresh() {
        refreshSession()
        try {
            getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, build())
        } catch (err: SecurityException) {
            Log.i(MediaPlayback.TAG, "The media notification is not shown: ${err.message}")
        }
    }

    private fun refreshSession() {
        val s = MediaPlayback.state
        val meta = MediaMetadata.Builder()
            .putString(MediaMetadata.METADATA_KEY_TITLE, s.title)
            .putString(MediaMetadata.METADATA_KEY_ARTIST, s.artist)
        if (s.duration > 0) meta.putLong(MediaMetadata.METADATA_KEY_DURATION, s.duration)
        s.artwork?.let { meta.putBitmap(MediaMetadata.METADATA_KEY_ART, it) }
        session.setMetadata(meta.build())
        val actions = PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or PlaybackState.ACTION_PLAY_PAUSE or
            PlaybackState.ACTION_STOP or PlaybackState.ACTION_SEEK_TO or PlaybackState.ACTION_FAST_FORWARD or
            PlaybackState.ACTION_REWIND
        session.setPlaybackState(
            PlaybackState.Builder()
                .setActions(actions)
                .setState(if (s.playing) PlaybackState.STATE_PLAYING else PlaybackState.STATE_PAUSED, s.position,
                    if (s.playing) s.rate else 0f, s.at)
                .build()
        )
    }

    private fun launchIntent(): PendingIntent? {
        val open = packageManager.getLaunchIntentForPackage(packageName) ?: return null
        open.flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_NEW_TASK
        return PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun button(icon: Int, title: Int, what: String): Notification.Action {
        val intent = Intent(this, MediaPlaybackService::class.java).setAction(what)
        val pending = PendingIntent.getService(this, what.hashCode(), intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        return Notification.Action.Builder(Icon.createWithResource(this, icon), getString(title), pending).build()
    }

    private fun build(): Notification {
        val s = MediaPlayback.state
        val builder = Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(s.title.ifEmpty { getString(R.string.media_playing) })
            .setContentText(s.artist)
            .setOngoing(s.playing)
            .setShowWhen(false)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setCategory(Notification.CATEGORY_TRANSPORT)
            .setDeleteIntent(PendingIntent.getService(this, ACTION_STOP.hashCode(),
                Intent(this, MediaPlaybackService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE))
            .addAction(button(android.R.drawable.ic_media_rew, R.string.media_back, ACTION_BACK))
            .addAction(
                if (s.playing) button(android.R.drawable.ic_media_pause, R.string.media_pause, ACTION_PAUSE)
                else button(android.R.drawable.ic_media_play, R.string.media_play, ACTION_PLAY)
            )
            .addAction(button(android.R.drawable.ic_media_ff, R.string.media_forward, ACTION_FORWARD))
            .setStyle(Notification.MediaStyle().setMediaSession(session.sessionToken).setShowActionsInCompactView(0, 1, 2))
        s.artwork?.let { builder.setLargeIcon(it) }
        launchIntent()?.let { builder.setContentIntent(it) }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        return builder.build()
    }
}
