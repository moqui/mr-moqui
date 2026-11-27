package org.moqui.startproto;

import android.app.Activity;
import android.graphics.Color;
import android.os.Bundle;
import android.view.View;
import android.view.ViewGroup;
import android.webkit.JavascriptInterface;
import android.webkit.WebView;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;
import java.io.File;
import java.nio.file.Files;
import java.util.List;

/** Start-time probe. Extra "mode": native = a native list only; web = WebView only;
 *  s7 = the saved snapshot drawn natively, then the WebView over it when it has painted.
 *  Boolean extra "sync": make the WebView before the first frame.
 *  "Fully drawn" (reportFullyDrawn) marks content from the WebView. */
public class Main extends Activity {
    static final int BG = Color.rgb(18, 18, 18);
    FrameLayout root;
    View snapshot;
    WebView web;

    @Override protected void onCreate(Bundle b) {
        super.onCreate(b);
        String mode = getIntent().getStringExtra("mode");
        if (mode == null) mode = "s7";
        String url = getIntent().getStringExtra("url");
        if (url == null) url = "http://localhost:18777/b/";
        root = new FrameLayout(this);
        root.setBackgroundColor(BG);
        if (mode.equals("native")) {
            String[] rows = new String[15];
            for (int i = 0; i < 15; i++) rows[i] = "Item " + (i + 1);
            root.addView(list(java.util.Arrays.asList(rows)));
            setContentView(root);
            return;
        }
        if (mode.equals("s7")) {
            File f = new File(getFilesDir(), "snapshot.txt");
            try {
                if (f.exists()) { snapshot = list(Files.readAllLines(f.toPath())); root.addView(snapshot); }
            } catch (Exception e) { }
        }
        setContentView(root);
        final String u = url;
        // sync: make the WebView before the first frame. Else after the first frame, so the first frame does not
        // wait for the WebView to load.
        if (getIntent().getBooleanExtra("sync", false)) { web(u); return; }
        root.getViewTreeObserver().addOnDrawListener(new android.view.ViewTreeObserver.OnDrawListener() {
            boolean done;
            @Override public void onDraw() {
                if (done) return; done = true;
                root.post(() -> web(u));
            }
        });
    }

    void web(String url) {
        web = new WebView(this);
        web.setBackgroundColor(BG);
        web.getSettings().setJavaScriptEnabled(true);
        web.getSettings().setDomStorageEnabled(true);
        web.addJavascriptInterface(this, "probe");
        root.addView(web, 0, new FrameLayout.LayoutParams(-1, -1));
        web.loadUrl(url);
    }

    /** The page calls this after the frame with its first content row. */
    @JavascriptInterface public void shown(final String rows) {
        runOnUiThread(() -> {
            reportFullyDrawn();
            try { Files.write(new File(getFilesDir(), "snapshot.txt").toPath(), rows.getBytes()); } catch (Exception e) { }
            if (snapshot != null) snapshot.setVisibility(View.GONE);
        });
    }

    View list(List<String> rows) {
        LinearLayout l = new LinearLayout(this);
        l.setOrientation(LinearLayout.VERTICAL);
        l.setBackgroundColor(BG);
        l.setPadding(32, 48, 32, 0);
        for (String r : rows) {
            TextView t = new TextView(this);
            t.setText(r); t.setTextColor(Color.WHITE); t.setTextSize(18); t.setPadding(0, 16, 0, 16);
            l.addView(t);
        }
        return l;
    }
}
