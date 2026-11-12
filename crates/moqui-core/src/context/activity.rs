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

//! What each thread is running now, for the Threads screen (plans/archive/Tools-Fancy.md §11): a mirror of the
//! thread's artifact stack that another thread can read. A Rust thread's own frames cannot be read from outside it
//! without `unsafe`, so this is the stack an administrator gets: the request, screen, transition, service, job and
//! entity operation a thread is in, how long each has run, its user and its trace.
//!
//! Each thread that pushes an artifact frame has one slot, registered once and dropped with the thread. The artifact
//! stack ([`super::artifact`]) writes each push and pop to it under the slot's own lock, which no other thread takes
//! except to read the list.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use moqui_lib::sync::lock;

/// One artifact frame as the mirror keeps it.
pub(super) struct MirrorFrame {
    pub(super) artifact_type: Cow<'static, str>,
    pub(super) name: Arc<str>,
    pub(super) action: Cow<'static, str>,
    pub(super) started: Instant,
}

#[derive(Default)]
struct Running {
    frames: Vec<MirrorFrame>,
    user_id: Option<String>,
    trace_id: Option<String>,
}

struct Slot {
    /// The kernel's id of the thread, which `/proc/self/task` lists; 0 where the platform does not tell it.
    thread_id: i64,
    thread_name: String,
    running: Mutex<Running>,
}

static SLOTS: Mutex<Vec<Weak<Slot>>> = Mutex::new(Vec::new());

thread_local! {
    static SLOT: Arc<Slot> = register();
}

fn register() -> Arc<Slot> {
    let slot = Arc::new(Slot {
        thread_id: kernel_thread_id(),
        thread_name: std::thread::current().name().unwrap_or_default().to_string(),
        running: Mutex::new(Running::default()),
    });
    let mut slots = lock(&SLOTS);
    // the slots of threads that ended go when a thread registers, so the list is bounded by the threads alive
    slots.retain(|known| known.strong_count() > 0);
    slots.push(Arc::downgrade(&slot));
    slot
}

#[cfg(target_os = "linux")]
fn kernel_thread_id() -> i64 {
    // "<pid>/task/<tid>"
    std::fs::read_link("/proc/thread-self")
        .ok()
        .and_then(|link| {
            link.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.parse().ok())
        })
        .unwrap_or(0)
}

#[cfg(not(target_os = "linux"))]
fn kernel_thread_id() -> i64 {
    0
}

/// Whether a frame pushed now should bring the thread's user: the first frames of a piece of work, until one is known.
const USER_DEPTH: usize = 3;

/// A frame pushed on this thread's artifact stack. `trace_id` is asked only while the work has no trace yet. Returns
/// whether the caller should tell the user ([`note_user`]).
pub(super) fn push(frame: MirrorFrame, trace_id: impl FnOnce() -> Option<String>) -> bool {
    SLOT.try_with(|slot| {
        let mut running = lock(&slot.running);
        if running.trace_id.is_none() {
            running.trace_id = trace_id();
        }
        running.frames.push(frame);
        running.user_id.is_none() && running.frames.len() <= USER_DEPTH
    })
    .unwrap_or(false)
}

/// The top frame popped. With none left the work is over, and its user and trace go with it.
pub(super) fn pop() {
    let _ = SLOT.try_with(|slot| {
        let mut running = lock(&slot.running);
        running.frames.pop();
        if running.frames.is_empty() {
            running.user_id = None;
            running.trace_id = None;
        }
    });
}

/// The user of the work this thread runs.
pub(super) fn note_user(user_id: Option<String>) {
    if user_id.is_none() {
        return;
    }
    let _ = SLOT.try_with(|slot| {
        let mut running = lock(&slot.running);
        if !running.frames.is_empty() {
            running.user_id = user_id;
        }
    });
}

/// One frame of a thread's work, as [`running`] answers it.
#[derive(Clone, Debug)]
pub struct ActivityFrame {
    pub artifact_type: String,
    pub name: String,
    pub action: String,
    /// How long the frame has been on the stack.
    pub seconds: f64,
}

/// What one thread is running.
#[derive(Clone, Debug)]
pub struct ThreadActivity {
    /// The kernel's thread id, 0 when the platform does not tell it.
    pub thread_id: i64,
    pub thread_name: String,
    pub user_id: Option<String>,
    pub trace_id: Option<String>,
    /// The artifact frames, the first pushed first.
    pub frames: Vec<ActivityFrame>,
}

/// The threads with an artifact frame on their stack now, and what each runs.
pub fn running() -> Vec<ThreadActivity> {
    let slots: Vec<Arc<Slot>> = lock(&SLOTS).iter().filter_map(Weak::upgrade).collect();
    let mut out = Vec::new();
    for slot in slots {
        let running = lock(&slot.running);
        if running.frames.is_empty() {
            continue;
        }
        let frames = running
            .frames
            .iter()
            .map(|frame| ActivityFrame {
                artifact_type: frame.artifact_type.to_string(),
                name: frame.name.to_string(),
                action: frame.action.to_string(),
                seconds: frame.started.elapsed().as_secs_f64(),
            })
            .collect();
        out.push(ThreadActivity {
            thread_id: slot.thread_id,
            thread_name: slot.thread_name.clone(),
            user_id: running.user_id.clone(),
            trace_id: running.trace_id.clone(),
            frames,
        });
    }
    out
}

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;
