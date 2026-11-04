// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

@file:Suppress("unused", "SetJavaScriptEnabled")

package {{package}}

import android.annotation.SuppressLint
import android.webkit.*
import android.content.Context
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import kotlin.collections.Map

@SuppressLint("RestrictedApi")
class RustWebView(context: Context, val initScripts: Array<String>, val id: String): WebView(context) {
    val isDocumentStartScriptEnabled: Boolean

    init {
        settings.javaScriptEnabled = true
        settings.domStorageEnabled = true
        settings.setGeolocationEnabled(true)
        settings.databaseEnabled = true
        settings.mediaPlaybackRequiresUserGesture = false
        // MR Moqui patch: no window.open without the user's gesture (wry sets true), so a page cannot send the phone to
        // the web or another app on its own
        settings.javaScriptCanOpenWindowsAutomatically = false

        if (WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) {
            isDocumentStartScriptEnabled = true
            // MR Moqui patch: the initialization scripts (Tauri's IPC and its invoke key among them) run in the app's own
            // origins only, never in a frame of another origin a server page has
            for (script in initScripts) {
                WebViewCompat.addDocumentStartJavaScript(this, script, PAGE_ORIGINS);
            }
        } else {
          isDocumentStartScriptEnabled = false
        }

        {{class-init}}
    }

    companion object {
        // MR Moqui patch (crates/moqui-mobile/patches/README.md): the app's own origins, its shell's and its server
        // pages' app schemes, the only ones that get the initialization scripts and the IPC.
        val PAGE_ORIGINS = setOf("http://moqui-shell.localhost", "http://moqui-app.localhost")
    }

    fun loadUrlMainThread(url: String) {
        post {
          loadUrl(url)
        }
    }

    fun loadUrlMainThread(url: String, additionalHttpHeaders: Map<String, String>) {
        post {
          loadUrl(url, additionalHttpHeaders)
        }
    }

    override fun loadUrl(url: String) {
        if (!Rust.shouldOverride(id, url)) {
            super.loadUrl(url);
        }
    }

    override fun loadUrl(url: String, additionalHttpHeaders: Map<String, String>) {
        if (!Rust.shouldOverride(id, url)) {
            super.loadUrl(url, additionalHttpHeaders);
        }
    }

    fun loadHTMLMainThread(html: String) {
        post {
          // A null base URL gives about:blank, which IPC accepts as a request URI.
          super.loadDataWithBaseURL(null, html, "text/html", "UTF-8", null)
        }
    }

    fun evalScript(id: Int, script: String) {
        post {
            super.evaluateJavascript(script) { result ->
                Rust.onEval(this.id, id, result)
            }
        }
    }

    fun clearAllBrowsingData() {
        try {
            super.getContext().deleteDatabase("webviewCache.db")
            super.getContext().deleteDatabase("webview.db")
            super.clearCache(true)
            super.clearHistory()
            super.clearFormData()
        } catch (ex: Exception) {
            Logger.error("Unable to create temporary media capture file: " + ex.message)
        }
    }

    fun getCookies(url: String): String {
        val cookieManager = CookieManager.getInstance()
        return cookieManager.getCookie(url)
    }

    {{class-extension}}
}
