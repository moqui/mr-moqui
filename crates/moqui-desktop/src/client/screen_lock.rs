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
//! - **Linux and Windows:** not watched yet (logind's `PrepareForSleep` and the session's `Lock`, and
//!   `WM_POWERBROADCAST` with `WTSRegisterSessionNotification`, need an event loop of their own here). On Linux the
//!   client's clock check still catches a suspend (its monotonic clock stops); on Windows neither a suspend nor a screen
//!   lock locks the vault, only its idle time (`moqui_client::vault`), which the vault guide states.

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

/// Not watched on this OS yet (see the module's doc).
#[cfg(not(target_os = "macos"))]
pub(crate) fn watch(lock: Box<dyn Fn() + Send + Sync>) {
    let _ = lock;
    tracing::info!(
        "The screen lock is not watched on this OS: the vault locks on its idle time and the client's clocks"
    );
}
