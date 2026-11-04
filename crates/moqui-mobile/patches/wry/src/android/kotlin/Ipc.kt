// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

@file:Suppress("unused")

package {{package}}

import android.webkit.*
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature

// MR Moqui patch (crates/moqui-mobile/patches/README.md): the IPC object is a web message listener for the main frame
// of the app's own origins only (RustWebView.PAGE_ORIGINS), not a JavaScript interface, which Android gives every
// frame, a cross-origin iframe of a server page too. A message from another frame or origin is dropped. A WebView
// without web message listeners (before about Chromium 84) gets no IPC at all.
class Ipc(val webView: RustWebView, val webViewClient: RustWebViewClient) {
    fun attach() {
        if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
            Logger.error("This WebView has no web message listeners: the app's pages have no IPC; update the WebView")
            return
        }
        WebViewCompat.addWebMessageListener(webView, "ipc", RustWebView.PAGE_ORIGINS) { _, message, sourceOrigin, isMainFrame, _ ->
            val data = message.data
            val origin = sourceOrigin.toString().trimEnd('/')
            if (data == null) {
                return@addWebMessageListener
            }
            if (!isMainFrame || !RustWebView.PAGE_ORIGINS.contains(origin)) {
                Logger.warn("IPC refused from a frame of $origin (main frame: $isMainFrame)")
                return@addWebMessageListener
            }
            // we're not using WebView::getUrl() here because it needs to be executed on the main thread
            // and it would slow down the Ipc
            // so instead we track the current URL on the webview client
            Rust.ipc(webView.id, webViewClient.currentUrl, data)
        }
    }

    {{class-extension}}
}
