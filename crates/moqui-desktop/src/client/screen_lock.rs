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

//! The computer's screen lock and sleep, as the OS tells them, which lock the vault (plans/Vault-Instances.md §3.6: "a
//! vault locks ... on the device's lock").
//!
//! - **macOS:** the distributed notification `com.apple.screenIsLocked` (the lock screen, the screen saver with a
//!   password) and NSWorkspace's will-sleep, screens-did-sleep and session-resigned (fast user switching) notifications.
//! - **Linux:** logind over D-Bus, each on a thread of its own: the Manager's `PrepareForSleep(true)`, and this
//!   session's `Lock` signal and its `LockedHint` turning true (the session is logind's `auto`: the app's own, or the
//!   user's graphical one when the app runs outside it, as a desktop's launcher starts it). The session bus's
//!   `org.freedesktop.ScreenSaver` `ActiveChanged(true)` too, which GNOME and KDE send as the lock screen comes up.
//!   A bus that cannot be reached is logged once; the vault still locks on its idle time and the client's clock check.
//! - **Windows:** a hidden window of its own, on its own thread, registered with `WTSRegisterSessionNotification`:
//!   `WM_WTSSESSION_CHANGE` with `WTS_SESSION_LOCK`, and `WM_POWERBROADCAST` with `PBT_APMSUSPEND` (a broadcast, which
//!   a top-level window gets and a message-only one would not).

/// Run `lock` whenever the OS says the screen locked or the computer goes to sleep. Called once, on the main thread.
#[cfg(target_os = "macos")]
pub(crate) fn watch(lock: Box<dyn Fn() + Send + Sync>) {
    use std::ptr::NonNull;
    use std::sync::Arc;

    use block2::RcBlock;
    use objc2_app_kit::{
        NSWorkspace, NSWorkspaceScreensDidSleepNotification, NSWorkspaceSessionDidResignActiveNotification,
        NSWorkspaceWillSleepNotification,
    };
    use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSString};

    let lock: Arc<dyn Fn() + Send + Sync> = Arc::from(lock);
    let block = RcBlock::new(move |_note: NonNull<NSNotification>| {
        tracing::info!("The screen locked or the computer is going to sleep: the vault sessions are locked");
        lock();
    });
    let locked = NSString::from_str("com.apple.screenIsLocked");
    let distributed = NSDistributedNotificationCenter::defaultCenter();
    let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: the names are NSString constants AppKit exports (static for the process) or one made here; no object is
    // named; no queue, so the block runs on the thread that posts, and it only calls `lock`, which is Send + Sync. The
    // observers are kept for the life of the process (forgotten, never removed), as the block is.
    unsafe {
        std::mem::forget(distributed.addObserverForName_object_queue_usingBlock(Some(&locked), None, None, &block));
        for name in [
            NSWorkspaceWillSleepNotification,
            NSWorkspaceScreensDidSleepNotification,
            NSWorkspaceSessionDidResignActiveNotification,
        ] {
            std::mem::forget(workspace.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block));
        }
    }
}

/// What the OS said, of the signals the watch listens to.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos")))), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    /// logind's `PrepareForSleep`: true before a suspend, false after the wake.
    PrepareForSleep(bool),
    /// logind's session `Lock`.
    SessionLock,
    /// logind's session `LockedHint`, as it changed.
    LockedHint(bool),
    /// The session bus's `org.freedesktop.ScreenSaver` `ActiveChanged`.
    ScreenSaverActive(bool),
}

/// Whether `event` locks the vault: a sleep coming, the session locked, the lock screen up. A wake, an unlock and
/// the screen saver ending lock nothing.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos")))), allow(dead_code))]
fn locks(event: Event) -> bool {
    match event {
        Event::PrepareForSleep(going) | Event::LockedHint(going) | Event::ScreenSaverActive(going) => going,
        Event::SessionLock => true,
    }
}

/// Run `lock` whenever logind or the screen saver says the screen locked or the computer goes to sleep (see the
/// module's doc). Called once; each watch runs on a thread of its own.
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn watch(lock: Box<dyn Fn() + Send + Sync>) {
    use std::sync::Arc;

    let lock: Arc<dyn Fn() + Send + Sync> = Arc::from(lock);
    let on = move |event: Event| {
        if locks(event) {
            tracing::info!("The screen locked or the computer is going to sleep ({event:?}): the vault sessions are locked");
            lock();
        }
    };
    let on: Arc<dyn Fn(Event) + Send + Sync> = Arc::new(on);
    for (name, run) in [
        ("moqui-desktop-sleep", linux::prepare_for_sleep as fn(&dyn Fn(Event)) -> zbus::Result<()>),
        ("moqui-desktop-session-lock", linux::session_lock),
        ("moqui-desktop-locked-hint", linux::locked_hint),
        ("moqui-desktop-screensaver", linux::screensaver),
    ] {
        let on = Arc::clone(&on);
        let spawned = std::thread::Builder::new().name(name.to_string()).spawn(move || {
            if let Err(err) = run(&*on) {
                tracing::warn!("{name}: not watched ({err}); the vault still locks on its idle time");
            }
        });
        if let Err(err) = spawned {
            tracing::warn!("{name}: no thread for the watch: {err}");
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod linux {
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::OwnedObjectPath;

    use super::Event;

    const LOGIN1: &str = "org.freedesktop.login1";

    /// logind's Manager on the system bus.
    fn manager(bus: &Connection) -> zbus::Result<Proxy<'static>> {
        Proxy::new(bus, LOGIN1, "/org/freedesktop/login1", "org.freedesktop.login1.Manager")
    }

    /// This user's session: logind's `auto` is the caller's session, or the user's graphical one when the caller is in
    /// none (an app a desktop's launcher started runs in a scope of the user's manager, not the session's).
    fn session(bus: &Connection) -> zbus::Result<Proxy<'static>> {
        let path: OwnedObjectPath = manager(bus)?.call("GetSession", &("auto",))?;
        Proxy::new(bus, LOGIN1, path, "org.freedesktop.login1.Session")
    }

    pub(super) fn prepare_for_sleep(on: &dyn Fn(Event)) -> zbus::Result<()> {
        let bus = Connection::system()?;
        for message in manager(&bus)?.receive_signal("PrepareForSleep")? {
            if let Ok(going) = message.body().deserialize::<bool>() {
                on(Event::PrepareForSleep(going));
            }
        }
        Ok(())
    }

    pub(super) fn session_lock(on: &dyn Fn(Event)) -> zbus::Result<()> {
        let bus = Connection::system()?;
        for _message in session(&bus)?.receive_signal("Lock")? {
            on(Event::SessionLock);
        }
        Ok(())
    }

    pub(super) fn locked_hint(on: &dyn Fn(Event)) -> zbus::Result<()> {
        let bus = Connection::system()?;
        let session = session(&bus)?;
        for changed in session.receive_property_changed::<bool>("LockedHint") {
            if let Ok(locked) = changed.get() {
                on(Event::LockedHint(locked));
            }
        }
        Ok(())
    }

    pub(super) fn screensaver(on: &dyn Fn(Event)) -> zbus::Result<()> {
        let bus = Connection::session()?;
        let saver = Proxy::new(
            &bus,
            "org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
            "org.freedesktop.ScreenSaver",
        )?;
        for message in saver.receive_signal("ActiveChanged")? {
            if let Ok(active) = message.body().deserialize::<bool>() {
                on(Event::ScreenSaverActive(active));
            }
        }
        Ok(())
    }
}

/// Run `lock` whenever Windows says the session locked or the computer goes to sleep (see the module's doc). Called
/// once; the window and its message loop are on a thread of their own.
#[cfg(windows)]
pub(crate) fn watch(lock: Box<dyn Fn() + Send + Sync>) {
    if win::LOCK.set(lock).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("moqui-desktop-screen-lock".to_string())
        .spawn(|| {
            if let Err(err) = win::run() {
                tracing::warn!("The screen lock is not watched ({err}); the vault still locks on its idle time");
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("No thread for the screen lock's watch: {err}");
    }
}

#[cfg(windows)]
mod win {
    use std::sync::OnceLock;

    use windows::core::{w, Error};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage, MSG,
        PBT_APMSUSPEND, WINDOW_EX_STYLE, WM_POWERBROADCAST, WM_WTSSESSION_CHANGE, WNDCLASSW, WS_OVERLAPPED,
        WTS_SESSION_LOCK,
    };

    /// What a lock runs, set once by `watch`.
    pub(super) static LOCK: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

    unsafe extern "system" fn procedure(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let locks = (message == WM_WTSSESSION_CHANGE && wparam.0 == WTS_SESSION_LOCK as usize)
            || (message == WM_POWERBROADCAST && wparam.0 == PBT_APMSUSPEND as usize);
        if locks {
            tracing::info!("The session locked or the computer is going to sleep: the vault sessions are locked");
            if let Some(lock) = LOCK.get() {
                lock();
            }
        }
        // SAFETY: the arguments are the ones Windows passed this window procedure.
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    /// Make the hidden window, register it for the session's changes, and pump its messages until the process ends.
    pub(super) fn run() -> Result<(), Error> {
        let class = w!("MoquiDesktopScreenLock");
        // SAFETY: plain Win32 calls with a class and window made here, on this thread, which owns them and runs their
        // message loop for the life of the process; the window is never shown.
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let window_class = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassW(&window_class) == 0 {
                return Err(Error::from_thread());
            }
            let window = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                w!(""),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION)?;
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{locks, Event};

    /// A sleep coming, the session locking and the lock screen coming up lock the vault; the wake, an unlock and the
    /// screen saver ending do not.
    #[test]
    fn only_a_lock_or_a_sleep_coming_locks_the_vault() {
        for event in [
            Event::PrepareForSleep(true),
            Event::SessionLock,
            Event::LockedHint(true),
            Event::ScreenSaverActive(true),
        ] {
            assert!(locks(event), "{event:?}");
        }
        for event in [
            Event::PrepareForSleep(false),
            Event::LockedHint(false),
            Event::ScreenSaverActive(false),
        ] {
            assert!(!locks(event), "{event:?}");
        }
    }
}
