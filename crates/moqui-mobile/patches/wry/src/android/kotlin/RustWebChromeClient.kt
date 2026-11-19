// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

@file:Suppress("ObsoleteSdkInt", "RedundantOverride", "QueryPermissionsNeeded", "SimpleDateFormat")

package {{package}}

// taken from https://github.com/ionic-team/capacitor/blob/6658bca41e78239347e458175b14ca8bd5c1d6e8/android/capacitor/src/main/java/com/getcapacitor/BridgeWebChromeClient.java

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.ActivityNotFoundException
import android.content.DialogInterface
import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.view.View
import android.webkit.*
import android.widget.EditText
import androidx.core.content.FileProvider
import java.io.File
import java.io.IOException
import java.text.SimpleDateFormat
import java.util.*

class RustWebChromeClient(private val activity: WryActivity, private val webViewId: String) : WebChromeClient() {
  companion object {
    private const val PERMISSION_REQUEST_DEFAULT = 0
    private const val PERMISSION_REQUEST_ALLOW = 1
    private const val PERMISSION_REQUEST_DENY = 2

    // MR Moqui patch (crates/moqui-mobile/patches/README.md, SEC-CLI-119): the app's own origins, as RustWebView's
    // PAGE_ORIGINS: the shell's, and every account's server pages'
    private const val SHELL_ORIGIN = "http://moqui-shell.localhost"
    private const val APP_ORIGIN = "http://moqui-app.localhost"

    // MR Moqui patch (SEC-CLI-89, SEC-CLI-110): the folder of the app's cache a camera capture is written to, the
    // FileProvider's one path (res/xml/file_paths.xml)
    private const val CAPTURES = "captures"

    // MR Moqui patch (SEC-CLI-119): the host of the server whose pages the app origin shows, which its URL does not
    // name; set by Rust as it opens them and cleared as they close (crates/moqui-mobile/src/host.rs open_remote)
    @Volatile private var appServer: String? = null

    /** MR Moqui patch (SEC-CLI-119): the server whose pages the app origin shows, or none. Called by Rust (JNI). */
    @JvmStatic
    fun pageServer(host: String?) {
      appServer = host?.trim()?.takeIf { it.isNotEmpty() }
    }
  }

  init {
    // MR Moqui patch (SEC-CLI-89, SEC-CLI-110): the captures a page was handed went with its webview; a new one finds
    // none (a capture is not deleted when handed over: the WebView reads it only when the page reads or uploads it)
    sweepCaptures()
  }

  /**
   * Render web content in `view`.
   *
   * Both this method and [.onHideCustomView] are required for
   * rendering web content in full screen.
   *
   * @see [](https://developer.android.com/reference/android/webkit/WebChromeClient.onShowCustomView
  ) */
  override fun onShowCustomView(view: View, callback: CustomViewCallback) {
    callback.onCustomViewHidden()
    super.onShowCustomView(view, callback)
  }

  /**
   * Render web content in the original Web View again.
   *
   * Do not remove this method--@see #onShowCustomView(View, CustomViewCallback).
   */
  override fun onHideCustomView() {
    super.onHideCustomView()
  }

  override fun onPermissionRequest(request: PermissionRequest) {
    val requestedResources = request.resources
    if (requestedResources.isEmpty()) {
      request.deny()
      return
    }

    val allowedResources = ArrayList<String>()
    val defaultResources = ArrayList<String>()

    for (resource in requestedResources) {
      when (onPermissionRequestNative(webViewId, resource)) {
        PERMISSION_REQUEST_DENY -> {}
        PERMISSION_REQUEST_ALLOW -> allowedResources.add(resource)
        PERMISSION_REQUEST_DEFAULT -> defaultResources.add(resource)
      }
    }

    val resources =
      allowedResources.plus(filterKnownPermissions(defaultResources)).toList().toTypedArray()
    grantPermissionRequest(request, resources)
  }

  private fun grantPermissionRequest(request: PermissionRequest, resources: Array<String>) {
    if (resources.isEmpty()) {
      request.deny()
      return
    }

    val isRequestPermissionRequired = Build.VERSION.SDK_INT >= Build.VERSION_CODES.M
    val permissionList = androidPermissionsForResources(resources)
    if (permissionList.isNotEmpty() && isRequestPermissionRequired) {
      val permissions = permissionList.toTypedArray()
      activity.requestPermissions(permissions) { isGranted ->
        if (isGranted == true) {
          request.grant(resources)
        } else {
          request.deny()
        }
      }
    } else {
      request.grant(resources)
    }
  }

  private fun androidPermissionsForResources(resources: Array<String>): MutableList<String> {
    val permissionList: MutableList<String> = ArrayList()
    if (resources.contains(PermissionRequest.RESOURCE_VIDEO_CAPTURE)) {
      permissionList.add(Manifest.permission.CAMERA)
    }
    if (resources.contains(PermissionRequest.RESOURCE_AUDIO_CAPTURE)) {
      permissionList.add(Manifest.permission.MODIFY_AUDIO_SETTINGS)
      permissionList.add(Manifest.permission.RECORD_AUDIO)
    }
    return permissionList
  }

  /**
   * @return one of the PERMISSION_REQUEST_* constants.
   */
  private external fun onPermissionRequestNative(webviewId: String, resource: String): Int

  override fun getDefaultVideoPoster(): Bitmap {
    // Return a transparent bitmap so Chromium doesn't paint its default gray play-button placeholder on <video> elements.
    // Matches react-native-webview's approach — see https://github.com/react-native-webview/react-native-webview/blob/58daac9e2e532ca1dcb49003b86568218b9b2b1d/android/src/main/java/com/reactnativecommunity/webview/RNCWebViewManager.kt
    return Bitmap.createBitmap(50, 50, Bitmap.Config.ARGB_8888)
  }

  /**
   * @return true when Rust denies geolocation; false continues the normal Android permission flow.
   */
  private external fun onGeolocationPermissionRequestNative(webviewId: String, origin: String): Boolean

  /**
   * Show the browser alert modal
   * @param view
   * @param url
   * @param message
   * @param result
   * @return
   */
  override fun onJsAlert(view: WebView, url: String, message: String, result: JsResult): Boolean {
    if (activity.isFinishing) {
      return true
    }
    val builder = AlertDialog.Builder(view.context)
    builder
      // MR Moqui patch (SEC-CLI-119): the dialog names who asks (dialogTitle)
      .setTitle(dialogTitle(url))
      .setMessage(message)
      .setPositiveButton(
        "OK"
      ) { dialog: DialogInterface, _: Int ->
        dialog.dismiss()
        result.confirm()
      }
      .setOnCancelListener { dialog: DialogInterface ->
        dialog.dismiss()
        result.cancel()
      }
    val dialog = builder.create()
    dialog.show()
    // MR Moqui patch: no touch through another app's window over the page's dialog (RustWebView.refuseObscuredTouches)
    RustWebView.refuseObscuredTouches(dialog.window?.decorView)
    return true
  }

  /**
   * Show the browser confirm modal
   * @param view
   * @param url
   * @param message
   * @param result
   * @return
   */
  override fun onJsConfirm(view: WebView, url: String, message: String, result: JsResult): Boolean {
    if (activity.isFinishing) {
      return true
    }
    val builder = AlertDialog.Builder(view.context)
    builder
      // MR Moqui patch (SEC-CLI-119): the dialog names who asks (dialogTitle)
      .setTitle(dialogTitle(url))
      .setMessage(message)
      .setPositiveButton(
        "OK"
      ) { dialog: DialogInterface, _: Int ->
        dialog.dismiss()
        result.confirm()
      }
      .setNegativeButton(
        "Cancel"
      ) { dialog: DialogInterface, _: Int ->
        dialog.dismiss()
        result.cancel()
      }
      .setOnCancelListener { dialog: DialogInterface ->
        dialog.dismiss()
        result.cancel()
      }
    val dialog = builder.create()
    dialog.show()
    // MR Moqui patch: no touch through another app's window over the page's dialog (RustWebView.refuseObscuredTouches)
    RustWebView.refuseObscuredTouches(dialog.window?.decorView)
    return true
  }

  /**
   * Show the browser prompt modal
   * @param view
   * @param url
   * @param message
   * @param defaultValue
   * @param result
   * @return
   */
  override fun onJsPrompt(
    view: WebView,
    url: String,
    message: String,
    defaultValue: String,
    result: JsPromptResult
  ): Boolean {
    // MR Moqui patch (SEC-CLI-119): no prompt on the app's own origins: a page of one account's server could ask there,
    // in the app's own dialog, for another account's password
    val origin = pageOrigin(url)
    if (origin == SHELL_ORIGIN || origin == APP_ORIGIN) {
      Logger.warn(Logger.tags("Dialog"), "A page's prompt on the app's origin is refused")
      result.cancel()
      return true
    }
    if (activity.isFinishing) {
      return true
    }
    val builder = AlertDialog.Builder(view.context)
    val input = EditText(view.context)
    builder
      // MR Moqui patch (SEC-CLI-119): the dialog names who asks (dialogTitle)
      .setTitle(dialogTitle(url))
      .setMessage(message)
      .setView(input)
      .setPositiveButton(
        "OK"
      ) { dialog: DialogInterface, _: Int ->
        dialog.dismiss()
        val inputText1 = input.text.toString().trim { it <= ' ' }
        result.confirm(inputText1)
      }
      .setNegativeButton(
        "Cancel"
      ) { dialog: DialogInterface, _: Int ->
        dialog.dismiss()
        result.cancel()
      }
      .setOnCancelListener { dialog: DialogInterface ->
        dialog.dismiss()
        result.cancel()
      }
    val dialog = builder.create()
    dialog.show()
    // MR Moqui patch: no touch through another app's window over the page's dialog (RustWebView.refuseObscuredTouches)
    RustWebView.refuseObscuredTouches(dialog.window?.decorView)
    return true
  }

  /**
   * MR Moqui patch (SEC-CLI-119): the origin (`scheme://host[:port]`) of the page at `url`, as the WebView names a page
   * that asks for a dialog; none when it has no host (`about:blank`, `data:`).
   */
  private fun pageOrigin(url: String): String? {
    val uri = Uri.parse(url)
    val scheme = uri.scheme?.lowercase() ?: return null
    val host = uri.host?.lowercase()?.takeIf { it.isNotEmpty() } ?: return null
    return if (uri.port == -1) "$scheme://$host" else "$scheme://$host:${uri.port}"
  }

  /**
   * MR Moqui patch (SEC-CLI-119): the title of a page's alert, confirm or prompt, which names who asks, since the
   * dialogs are the app's own: the shell's the app's name; a server page's (every account's are on the one app origin)
   * the host of the server Rust opened them for ([pageServer]); any other page's (a frame of a server page) its own
   * host.
   */
  private fun dialogTitle(url: String): String {
    val origin = pageOrigin(url)
    return when {
      origin == SHELL_ORIGIN -> activity.applicationInfo.loadLabel(activity.packageManager).toString()
      origin == APP_ORIGIN -> appServer ?: "A server page"
      origin == null -> "A web page"
      else -> Uri.parse(url).host ?: "A web page"
    }
  }

  /**
   * Handle the browser geolocation permission prompt
   * @param origin
   * @param callback
   */
  override fun onGeolocationPermissionsShowPrompt(
    origin: String,
    callback: GeolocationPermissions.Callback
  ) {
    super.onGeolocationPermissionsShowPrompt(origin, callback)
    if (onGeolocationPermissionRequestNative(webViewId, origin)) {
      callback.invoke(origin, false, false)
      return
    }

    Logger.debug("onGeolocationPermissionsShowPrompt: DOING IT HERE FOR ORIGIN: $origin")
    val geoPermissions = definedGeolocationPermissions()
    if (geoPermissions.isEmpty()) {
      callback.invoke(origin, false, false)
      return
    }

    if (!PermissionHelper.hasPermissions(activity, geoPermissions)) {
      activity.requestPermissions(geoPermissions) { isGranted ->
        if (isGranted == true) {
          callback.invoke(origin, true, false)
        } else {
          val coarsePermission =
            arrayOf(Manifest.permission.ACCESS_COARSE_LOCATION)
          if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
            PermissionHelper.hasPermissions(activity, coarsePermission)
          ) {
            callback.invoke(origin, true, false)
          } else {
            callback.invoke(origin, false, false)
          }
        }
      }
    } else {
      // permission is already granted
      callback.invoke(origin, true, false)
      Logger.debug("onGeolocationPermissionsShowPrompt: has required permission")
    }
  }

  private fun filterKnownPermissions(resources: List<String>): Array<String> {
    return resources.filter {
      it == PermissionRequest.RESOURCE_AUDIO_CAPTURE ||
        it == PermissionRequest.RESOURCE_VIDEO_CAPTURE ||
        it == PermissionRequest.RESOURCE_PROTECTED_MEDIA_ID ||
        it == PermissionRequest.RESOURCE_MIDI_SYSEX
    }.toTypedArray()
  }

  private fun definedGeolocationPermissions(): Array<String> {
    val permissions = ArrayList<String>()
    if (PermissionHelper.hasDefinedPermission(activity, Manifest.permission.ACCESS_COARSE_LOCATION)) {
      permissions.add(Manifest.permission.ACCESS_COARSE_LOCATION)
    }
    if (PermissionHelper.hasDefinedPermission(activity, Manifest.permission.ACCESS_FINE_LOCATION)) {
      permissions.add(Manifest.permission.ACCESS_FINE_LOCATION)
    }
    return permissions.toTypedArray()
  }

  override fun onShowFileChooser(
    webView: WebView,
    filePathCallback: ValueCallback<Array<Uri?>?>,
    fileChooserParams: FileChooserParams
  ): Boolean {
    val acceptTypes = listOf(*fileChooserParams.acceptTypes)
    val captureEnabled = fileChooserParams.isCaptureEnabled
    val capturePhoto = captureEnabled && acceptTypes.contains("image/*")
    val captureVideo = captureEnabled && acceptTypes.contains("video/*")
    if (capturePhoto || captureVideo) {
      if (isMediaCaptureSupported) {
        showMediaCaptureOrFilePicker(filePathCallback, fileChooserParams, captureVideo)
      } else {
        val camPermission = arrayOf(Manifest.permission.CAMERA)
        activity.requestPermissions(camPermission) { isGranted ->
          if (isGranted == true) {
            showMediaCaptureOrFilePicker(filePathCallback, fileChooserParams, captureVideo)
          } else {
            Logger.warn(Logger.tags("FileChooser"), "Camera permission not granted")
            filePathCallback.onReceiveValue(null)
          }
        }
      }
    } else {
      showFilePicker(filePathCallback, fileChooserParams)
    }
    return true
  }

  private val isMediaCaptureSupported: Boolean
    get() {
      val permissions = arrayOf(Manifest.permission.CAMERA)
      return PermissionHelper.hasPermissions(activity, permissions) ||
        !PermissionHelper.hasDefinedPermission(activity, Manifest.permission.CAMERA)
    }

  private fun showMediaCaptureOrFilePicker(
    filePathCallback: ValueCallback<Array<Uri?>?>,
    fileChooserParams: FileChooserParams,
    isVideo: Boolean
  ) {
    val isVideoCaptureSupported = true
    val shown = if (isVideo && isVideoCaptureSupported) {
      showVideoCapturePicker(filePathCallback)
    } else {
      showImageCapturePicker(filePathCallback)
    }
    if (!shown) {
      Logger.warn(
        Logger.tags("FileChooser"),
        "Media capture intent could not be launched. Falling back to default file picker."
      )
      showFilePicker(filePathCallback, fileChooserParams)
    }
  }

  private fun showImageCapturePicker(filePathCallback: ValueCallback<Array<Uri?>?>): Boolean {
    val takePictureIntent = Intent(MediaStore.ACTION_IMAGE_CAPTURE)
    if (takePictureIntent.resolveActivity(activity.packageManager) == null) {
      return false
    }
    // MR Moqui patch (SEC-CLI-89): the photo goes to the app's cache (createCaptureFile), not to shared storage
    val (imageFile, imageFileUri) = try {
      val file = createCaptureFile("JPEG_", ".jpg")
      file to captureUri(file)
    } catch (ex: Exception) {
      Logger.error("Unable to create temporary media capture file: " + ex.message)
      return false
    }
    takePictureIntent.putExtra(MediaStore.EXTRA_OUTPUT, imageFileUri)
    activity.launchActivityForResult(takePictureIntent) { result ->
      var res: Array<Uri?>? = null
      if (result?.resultCode == Activity.RESULT_OK && imageFile.length() > 0) {
        res = arrayOf(imageFileUri)
      } else {
        // cancelled, or the camera wrote nothing: the file goes now
        imageFile.delete()
      }
      filePathCallback.onReceiveValue(res)
    }
    return true
  }

  private fun showVideoCapturePicker(filePathCallback: ValueCallback<Array<Uri?>?>): Boolean {
    val takeVideoIntent = Intent(MediaStore.ACTION_VIDEO_CAPTURE)
    if (takeVideoIntent.resolveActivity(activity.packageManager) == null) {
      return false
    }
    // MR Moqui patch (SEC-CLI-110): the video goes to the app's cache as a photo does (EXTRA_OUTPUT), not where the
    // camera app puts its videos (shared storage, the gallery)
    val (videoFile, videoFileUri) = try {
      val file = createCaptureFile("VID_", ".mp4")
      file to captureUri(file)
    } catch (ex: Exception) {
      Logger.error("Unable to create temporary media capture file: " + ex.message)
      return false
    }
    takeVideoIntent.putExtra(MediaStore.EXTRA_OUTPUT, videoFileUri)
    activity.launchActivityForResult(takeVideoIntent) { result ->
      var res: Array<Uri?>? = null
      if (result?.resultCode == Activity.RESULT_OK) {
        // a camera app that ignores EXTRA_OUTPUT leaves the file empty and says where its video went, if anywhere
        val taken: Uri? = if (videoFile.length() > 0) videoFileUri else result.data?.data
        if (taken != null) {
          res = arrayOf(taken)
        }
      }
      if (res == null || res[0] != videoFileUri) {
        // cancelled, or the video is elsewhere: the file goes now
        videoFile.delete()
      }
      filePathCallback.onReceiveValue(res)
    }
    return true
  }

  private fun showFilePicker(
    filePathCallback: ValueCallback<Array<Uri?>?>,
    fileChooserParams: FileChooserParams
  ) {
    val intent = fileChooserParams.createIntent()
    if (fileChooserParams.mode == FileChooserParams.MODE_OPEN_MULTIPLE) {
      intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
    }
    if (fileChooserParams.acceptTypes.size > 1 || intent.type!!.startsWith(".")) {
      val validTypes = getValidTypes(fileChooserParams.acceptTypes)
      intent.putExtra(Intent.EXTRA_MIME_TYPES, validTypes)
      if (intent.type!!.startsWith(".")) {
        intent.type = validTypes[0]
      }
    }
    try {
      activity.launchActivityForResult(intent) { result ->
        val res: Array<Uri?>?
        val resultIntent = result?.data
        if (result?.resultCode == Activity.RESULT_OK && resultIntent!!.clipData != null) {
          val numFiles = resultIntent.clipData!!.itemCount
          res = arrayOfNulls(numFiles)
          for (i in 0 until numFiles) {
            res[i] = resultIntent.clipData!!.getItemAt(i).uri
          }
        } else {
          res = FileChooserParams.parseResult(
            result?.resultCode ?: 0,
            resultIntent
          )
        }
        filePathCallback.onReceiveValue(res)
      }
    } catch (e: ActivityNotFoundException) {
      filePathCallback.onReceiveValue(null)
    }
  }

  private fun getValidTypes(currentTypes: Array<String>): Array<String> {
    val validTypes: MutableList<String> = ArrayList()
    val mtm = MimeTypeMap.getSingleton()
    for (mime in currentTypes) {
      if (mime.startsWith(".")) {
        val extension = mime.substring(1)
        val extensionMime = mtm.getMimeTypeFromExtension(extension)
        if (extensionMime != null && !validTypes.contains(extensionMime)) {
          validTypes.add(extensionMime)
        }
      } else if (!validTypes.contains(mime)) {
        validTypes.add(mime)
      }
    }
    val validObj: Array<Any> = validTypes.toTypedArray()
    return Arrays.copyOf(
      validObj, validObj.size,
      Array<String>::class.java
    )
  }

  override fun onConsoleMessage(consoleMessage: ConsoleMessage): Boolean {
    val tag: String = Logger.tags("Console")
    if (consoleMessage.message() != null && isValidMsg(consoleMessage.message())) {
      val msg = String.format(
        "File: %s - Line %d - Msg: %s",
        consoleMessage.sourceId(),
        consoleMessage.lineNumber(),
        consoleMessage.message()
      )
      val level = consoleMessage.messageLevel().name
      if ("ERROR".equals(level, ignoreCase = true)) {
        Logger.error(tag, msg, null)
      } else if ("WARNING".equals(level, ignoreCase = true)) {
        Logger.warn(tag, msg)
      } else if ("TIP".equals(level, ignoreCase = true)) {
        Logger.debug(tag, msg)
      } else {
        Logger.info(tag, msg)
      }
    }
    return true
  }

  private fun isValidMsg(msg: String): Boolean {
    return !(msg.contains("%cresult %c") ||
      msg.contains("%cnative %c") ||
      msg.equals("[object Object]", ignoreCase = true) ||
      msg.equals("console.groupEnd", ignoreCase = true))
  }

  /**
   * MR Moqui patch (SEC-CLI-89, SEC-CLI-110): a new file for a camera capture (`prefix`, `suffix`: `JPEG_`, `.jpg`) in
   * the app's cache, `captures/`, which neither another app nor a backup reads, and the FileProvider's one path; a
   * cancelled capture's goes at once, a taken one at the next webview's start ([sweepCaptures]).
   */
  @Throws(IOException::class)
  private fun createCaptureFile(prefix: String, suffix: String): File {
    val dir = File(activity.cacheDir, CAPTURES)
    if (!dir.isDirectory && !dir.mkdirs()) {
      throw IOException("The folder $dir was not made")
    }
    val timeStamp = SimpleDateFormat("yyyyMMdd_HHmmss").format(Date())
    return File.createTempFile(prefix + timeStamp + "_", suffix, dir)
  }

  /** The `content:` URI of a capture file, which the camera app is let write. */
  private fun captureUri(file: File): Uri {
    return FileProvider.getUriForFile(activity, activity.packageName + ".fileprovider", file)
  }

  /** MR Moqui patch (SEC-CLI-89, SEC-CLI-110): the captures of the pages before this webview go. */
  private fun sweepCaptures() {
    try {
      File(activity.cacheDir, CAPTURES).listFiles()?.forEach { it.delete() }
    } catch (ex: SecurityException) {
      Logger.error("The camera captures were not deleted: " + ex.message)
    }
  }

  override fun onReceivedTitle(
      view: WebView,
      title: String
  ) {
    Rust.handleReceivedTitle((view as RustWebView).id, title)
  }
}
