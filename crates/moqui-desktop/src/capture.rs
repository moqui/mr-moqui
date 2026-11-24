/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! Sharing a screen or a window from the app's pages (plans/archive/Comms-Whiteboard.md §3.5, docs/ui/Desktop-Client.md
//! "Screen share").
//!
//! On Linux and Windows the webview is CEF's Chrome style, and a page's `getDisplayMedia` shows Chromium's own picker:
//! [`answer`] leaves the request to it for the app's own pages and refuses it to any other. On macOS every webview is
//! Alloy style (tauri-runtime-cef forces it for a native parent), which has no picker and refuses desktop capture, so
//! the app shows its own: [`capture_sources`] lists the displays and the other applications' windows, the page shows
//! them, and [`capture_choose`] takes the one the user picked. The page then asks for that source by its desktop media
//! id (`getUserMedia` with `chromeMediaSource: "desktop"`), and the patched runtime grants that one request
//! (`patches/tauri-runtime-cef`, [`GRANT_FOR`]). A choice not used within [`GRANT_FOR`] lapses; a request no choice
//! preceded is refused. The OS still asks the user, once, for the screen-recording permission.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value as Json};
use tauri::webview::PermissionResponse;
use tauri::{AppHandle, Manager, Webview};

/// How long a source the user picked may be asked for.
pub(crate) const GRANT_FOR: Duration = Duration::from_secs(15);
/// The most sources listed.
const MAX_SOURCES: usize = 200;

/// A display or a window the page may offer: its desktop media id (`screen:<display>:0` or `window:<window>:0`, as
/// Chromium parses one), whether it is a screen, and its name (the display's size, or the application and the window's
/// title).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Source {
    pub id: String,
    pub screen: bool,
    pub name: String,
    /// Where it is on the desktop, in points: what the annotation overlay covers.
    pub bounds: Option<(f64, f64, f64, f64)>,
}

impl Source {
    /// As the page gets it.
    fn json(&self) -> Json {
        json!({"id": self.id, "screen": self.screen, "name": self.name,
            "bounds": self.bounds.map(|(x, y, w, h)| json!({"x": x, "y": y, "width": w, "height": h}))})
    }
}

/// What the page was offered last, and the choice waiting to be asked for: by window label.
#[derive(Default)]
pub(crate) struct Capture {
    offered: Mutex<Vec<Source>>,
    chosen: Mutex<Option<Chosen>>,
}

#[derive(Clone, Debug)]
struct Chosen {
    window: String,
    source: Source,
    at: Instant,
}

/// Whether `id` is a desktop media id the app makes: `screen:` or `window:`, a number, `:0`.
pub(crate) fn valid_id(id: &str) -> bool {
    let Some((kind, rest)) = id.split_once(':') else {
        return false;
    };
    let Some((number, tail)) = rest.split_once(':') else {
        return false;
    };
    matches!(kind, "screen" | "window")
        && !number.is_empty()
        && number.len() <= 20
        && number.bytes().all(|b| b.is_ascii_digit())
        && tail == "0"
}

impl Capture {
    /// The user picked `id` of what `window` was offered: it may be asked for within [`GRANT_FOR`].
    fn choose(&self, window: &str, id: &str, now: Instant) -> Result<Source, String> {
        if !valid_id(id) {
            return Err("not a screen or window".into());
        }
        let source = moqui_lib::sync::lock(&self.offered)
            .iter()
            .find(|source| source.id == id)
            .cloned()
            .ok_or("that screen or window is not offered")?;
        *moqui_lib::sync::lock(&self.chosen) = Some(Chosen {
            window: window.to_string(),
            source: source.clone(),
            at: now,
        });
        Ok(source)
    }

    /// The choice of `window`, if it was made within [`GRANT_FOR`]: taken, so one choice grants one request.
    fn take(&self, window: &str, now: Instant) -> Option<Source> {
        let mut chosen = moqui_lib::sync::lock(&self.chosen);
        let fresh = chosen
            .as_ref()
            .is_some_and(|c| c.window == window && now.saturating_duration_since(c.at) <= GRANT_FOR);
        if fresh {
            chosen.take().map(|c| c.source)
        } else {
            None
        }
    }
}

/// The answer to a page's request to capture the screen, for one of the app's own pages (`allowed`, the bridge's
/// origin check) or not. Off macOS the runtime's: Chromium's picker. On macOS an `Allow` when the user just picked a
/// source in the app's picker, which the patched runtime turns into a grant of that one request; else the runtime's
/// refusal.
pub(crate) fn answer(app: &AppHandle, webview: &Webview, allowed: bool) -> PermissionResponse {
    if !allowed {
        return PermissionResponse::Deny;
    }
    if !cfg!(target_os = "macos") {
        return PermissionResponse::Default;
    }
    let capture = app.state::<Capture>();
    match capture.take(webview.label(), Instant::now()) {
        Some(_) => {
            #[cfg(target_os = "macos")]
            tauri_runtime_cef::grant_next_desktop_capture(GRANT_FOR);
            PermissionResponse::Allow
        }
        None => PermissionResponse::Default,
    }
}

/// The displays and the other applications' windows the page may offer for sharing (macOS; elsewhere the browser's
/// picker is used and this answers an error). Asks for the screen-recording permission first; without it the OS gives
/// no window titles and no picture, and the answer says so.
#[tauri::command]
pub(crate) fn capture_sources(app: AppHandle) -> Result<Vec<Json>, String> {
    let sources = os::sources()?;
    let sources: Vec<Source> = sources
        .into_iter()
        .filter(|s| valid_id(&s.id))
        .take(MAX_SOURCES)
        .collect();
    let listed = sources.iter().map(Source::json).collect();
    *moqui_lib::sync::lock(&app.state::<Capture>().offered) = sources;
    Ok(listed)
}

/// The user picked `id` among the sources last listed: the page asks for it next (`getUserMedia` with that desktop
/// media id), within [`GRANT_FOR`]. Out: the source, for its overlay.
#[tauri::command]
pub(crate) fn capture_choose(app: AppHandle, webview: Webview, id: String) -> Result<Json, String> {
    app.state::<Capture>()
        .choose(webview.label(), &id, Instant::now())
        .map(|source| source.json())
}

/// The overlay window's label.
const OVERLAY: &str = "capture-overlay";
/// The most strokes the overlay draws (as the media node keeps, rtc/ink.rs).
const MAX_STROKES: usize = 128;

/// Whether this desktop can show the overlay: not a Wayland session, which gives a window no place of its own on the
/// desktop and no way to stay above the others.
fn overlay_possible() -> bool {
    !(cfg!(all(unix, not(target_os = "macos")))
        && std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind == "wayland"))
}

/// Where the overlay goes: the shared source's bounds (`x`, `y`, `width`, `height` in points, which the macOS picker
/// knows), else, with one monitor, that monitor (the browser's picker on Windows and X11 says only that a screen is
/// shared, not which); none when neither is known.
fn overlay_bounds(app: &AppHandle, bounds: &Json) -> Option<(f64, f64, f64, f64)> {
    let field = |name: &str| bounds[name].as_f64().filter(|n| n.is_finite());
    if let (Some(x), Some(y), Some(w), Some(h)) = (field("x"), field("y"), field("width"), field("height")) {
        return (w >= 1.0 && h >= 1.0).then_some((x, y, w, h));
    }
    let monitors = app.available_monitors().ok()?;
    let [monitor] = monitors.as_slice() else { return None };
    let scale = monitor.scale_factor();
    let (position, size) = (monitor.position(), monitor.size());
    Some((
        f64::from(position.x) / scale,
        f64::from(position.y) / scale,
        f64::from(size.width) / scale,
        f64::from(size.height) / scale,
    ))
}

/// Show the annotation overlay over the shared window or screen (§3.3): transparent, above the other windows, and
/// click-through, so the presenter's strokes and the others' show on what is shared and in the capture. Out: whether it
/// is shown; off on Wayland and where the place is not known.
#[tauri::command]
pub(crate) fn capture_overlay(app: AppHandle, bounds: Json) -> bool {
    if !overlay_possible() {
        return false;
    }
    let Some((x, y, width, height)) = overlay_bounds(&app, &bounds) else {
        return false;
    };
    let window = match app.get_webview_window(OVERLAY) {
        Some(window) => window,
        None => {
            let built = tauri::WebviewWindowBuilder::new(&app, OVERLAY, tauri::WebviewUrl::App("overlay.html".into()))
                .title("MR Moqui annotations")
                .transparent(true)
                .decorations(false)
                .shadow(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .focused(false)
                .resizable(false)
                .visible(false)
                .build();
            match built {
                Ok(window) => window,
                Err(err) => {
                    tracing::warn!("The annotation overlay did not open: {err}");
                    return false;
                }
            }
        }
    };
    let placed = window
        .set_position(tauri::LogicalPosition::new(x, y))
        .and_then(|()| window.set_size(tauri::LogicalSize::new(width, height)))
        .and_then(|()| window.set_ignore_cursor_events(true))
        .and_then(|()| window.show());
    if let Err(err) = placed {
        tracing::warn!("The annotation overlay was not placed: {err}");
        let _ = window.close();
        return false;
    }
    true
}

/// The strokes the overlay draws now (the page's, in fractions of the shared picture); an empty list clears it.
#[tauri::command]
pub(crate) fn capture_ink(app: AppHandle, strokes: Json) {
    let Some(window) = app.get_webview_window(OVERLAY) else {
        return;
    };
    let strokes: Vec<Json> = strokes
        .as_array()
        .map_or_else(Vec::new, |list| list.iter().take(MAX_STROKES).cloned().collect());
    // the strokes go in as JSON data, never as script text
    let script = format!("window.drawInk && window.drawInk({})", Json::Array(strokes));
    if let Err(err) = window.eval(script) {
        tracing::warn!("The annotation overlay was not drawn: {err}");
    }
}

/// The share ended: the overlay goes.
#[tauri::command]
pub(crate) fn capture_overlay_close(app: AppHandle) {
    if let Some(window) = app.get_webview_window(OVERLAY) {
        if let Err(err) = window.close() {
            tracing::warn!("The annotation overlay did not close: {err}");
        }
    }
}

#[cfg(target_os = "macos")]
mod os {
    //! CoreGraphics: the active displays, and the windows of other applications on screen at the normal level.
    use std::ffi::{c_char, c_void, CStr};

    use super::Source;

    type CFTypeRef = *const c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGDisplayBounds(display: u32) -> CGRect;
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFTypeRef;
        fn CGRectMakeWithDictionaryRepresentation(dict: CFTypeRef, rect: *mut CGRect) -> bool;
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
        static kCGWindowNumber: CFTypeRef;
        static kCGWindowOwnerName: CFTypeRef;
        static kCGWindowOwnerPID: CFTypeRef;
        static kCGWindowName: CFTypeRef;
        static kCGWindowLayer: CFTypeRef;
        static kCGWindowBounds: CFTypeRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(array: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
        fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFNumberGetValue(number: CFTypeRef, kind: isize, out: *mut c_void) -> bool;
        fn CFStringGetCString(string: CFTypeRef, buffer: *mut c_char, size: isize, encoding: u32) -> bool;
        fn CFRelease(object: CFTypeRef);
    }

    const ON_SCREEN_ONLY: u32 = 1 << 0;
    const EXCLUDE_DESKTOP: u32 = 1 << 4;
    const SINT64: isize = 4;
    const UTF8: u32 = 0x0800_0100;

    fn number(dict: CFTypeRef, key: CFTypeRef) -> Option<i64> {
        // SAFETY: dict is a CFDictionary of the window list, key a CoreGraphics key constant; the value, when present,
        // is a CFNumber the dictionary owns.
        unsafe {
            let value = CFDictionaryGetValue(dict, key);
            let mut out = 0i64;
            (!value.is_null() && CFNumberGetValue(value, SINT64, (&mut out as *mut i64).cast())).then_some(out)
        }
    }

    fn text(dict: CFTypeRef, key: CFTypeRef) -> Option<String> {
        // SAFETY: as for number; the value is a CFString, copied into the buffer.
        unsafe {
            let value = CFDictionaryGetValue(dict, key);
            if value.is_null() {
                return None;
            }
            let mut buffer = [0 as c_char; 512];
            CFStringGetCString(value, buffer.as_mut_ptr(), buffer.len() as isize, UTF8)
                .then(|| CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned())
        }
    }

    pub(super) fn sources() -> Result<Vec<Source>, String> {
        // SAFETY: plain CoreGraphics calls; the window list is released after it is read.
        unsafe {
            if !CGPreflightScreenCaptureAccess() && !CGRequestScreenCaptureAccess() {
                return Err("Allow MR Moqui to record the screen in System Settings, Privacy & Security, Screen & System Audio Recording, then share again".into());
            }
            let mut displays = [0u32; 16];
            let mut count = 0u32;
            let mut sources = Vec::new();
            if CGGetActiveDisplayList(displays.len() as u32, displays.as_mut_ptr(), &mut count) == 0 {
                for (index, display) in displays.iter().take(count as usize).enumerate() {
                    let bounds = CGDisplayBounds(*display);
                    sources.push(Source {
                        id: format!("screen:{display}:0"),
                        screen: true,
                        name: format!(
                            "Screen {} ({} × {})",
                            index + 1,
                            bounds.size.width as i64,
                            bounds.size.height as i64
                        ),
                        bounds: Some((bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height)),
                    });
                }
            }
            let list = CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP, 0);
            if list.is_null() {
                return Ok(sources);
            }
            let own = i64::from(std::process::id());
            for index in 0..CFArrayGetCount(list) {
                let window = CFArrayGetValueAtIndex(list, index);
                if number(window, kCGWindowLayer) != Some(0) || number(window, kCGWindowOwnerPID) == Some(own) {
                    continue;
                }
                let Some(id) = number(window, kCGWindowNumber) else {
                    continue;
                };
                let owner = text(window, kCGWindowOwnerName).unwrap_or_default();
                let title = text(window, kCGWindowName).unwrap_or_default();
                let mut rect = CGRect::default();
                let bounds_dict = CFDictionaryGetValue(window, kCGWindowBounds);
                let bounds = (!bounds_dict.is_null() && CGRectMakeWithDictionaryRepresentation(bounds_dict, &mut rect))
                    .then_some((rect.origin.x, rect.origin.y, rect.size.width, rect.size.height));
                // a window too small to read is not offered (menu extras, panels)
                if bounds.is_some_and(|(_, _, w, h)| w < 80.0 || h < 60.0) {
                    continue;
                }
                let name = match (owner.is_empty(), title.is_empty()) {
                    (false, false) => format!("{owner}: {title}"),
                    (false, true) => owner,
                    (true, false) => title,
                    (true, true) => continue,
                };
                sources.push(Source {
                    id: format!("window:{id}:0"),
                    screen: false,
                    name,
                    bounds,
                });
            }
            CFRelease(list);
            Ok(sources)
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod os {
    use super::Source;

    /// Off macOS the page uses the browser's own picker (`getDisplayMedia`).
    pub(super) fn sources() -> Result<Vec<Source>, String> {
        Err("this desktop shares with the browser's own picker".into())
    }
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
