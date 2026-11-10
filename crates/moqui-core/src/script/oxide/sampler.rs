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

//! The JavaScript sampling profiler (plans/Profiling2.md, R53): where the interpreter's time goes by script function
//! and line, which a native profile cannot show.
//!
//! Off unless `MOQUI_JS_SAMPLE` names a file. Then one thread sets a flag about every millisecond, each runtime samples
//! on it from its start (quickjs-oxide's `Runtime::start_sampling`: the next poll that finds the flag set counts the
//! running stack), and after each screen render [`flush`] appends the thread's samples to the file as folded stacks,
//! root first, under the render's path: `path;outer:12;inner:40 3`, which flame graph tools read. A frame's line is in
//! the source it ran: a unit's in its generated text, a mode runtime's in the macro's runtime (`MOQUI_UNIT_DUMP` writes
//! both).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use quickjs_oxide::engine::api::core::Runtime;

static FILE: LazyLock<Option<Mutex<File>>> = LazyLock::new(|| {
    let path = std::env::var_os("MOQUI_JS_SAMPLE")?;
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => Some(Mutex::new(file)),
        Err(err) => {
            tracing::error!("Could not open the script sample file {}: {err}", path.to_string_lossy());
            None
        }
    }
});

/// The flag every runtime samples on, set by the ticking thread started with it.
static FLAG: LazyLock<Arc<AtomicBool>> = LazyLock::new(|| {
    let flag = Arc::new(AtomicBool::new(false));
    let ticked = Arc::clone(&flag);
    let started = std::thread::Builder::new()
        .name("moqui-js-sampler".to_string())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(1));
            ticked.store(true, Ordering::Relaxed);
        });
    if let Err(err) = started {
        tracing::error!("Could not start the script sampler's thread: {err}");
    }
    flag
});

fn enabled() -> bool {
    FILE.is_some()
}

/// Sample `runtime` from now on, when the profiler is on.
pub(super) fn start(runtime: &Runtime) {
    if !enabled() {
        return;
    }
    if let Err(err) = runtime.start_sampling(Arc::clone(&FLAG)) {
        tracing::error!("Could not start sampling a script runtime: {err:?}");
    }
}

/// Append the samples this thread's runtime counted since the last flush, under `label` (a screen path).
pub(crate) fn flush(label: &str) {
    if !enabled() {
        return;
    }
    let Some(engine) = super::engine::current() else { return };
    let samples = match engine.runtime().take_samples() {
        Ok(samples) => samples,
        // in a run (the runtime is entered): the next flush takes them
        Err(_) => return,
    };
    if samples.is_empty() {
        return;
    }
    let label = label.replace([';', ' ', '\n'], "_");
    let mut text = String::new();
    for (stack, count) in samples {
        text.push_str(&label);
        // the engine counts the innermost frame first; folded stacks put the root first
        for frame in stack.rsplit(';') {
            text.push(';');
            text.push_str(&frame.replace(' ', "_"));
        }
        text.push(' ');
        text.push_str(&count.to_string());
        text.push('\n');
    }
    if let Some(file) = FILE.as_ref() {
        if let Ok(mut file) = file.lock() {
            if let Err(err) = file.write_all(text.as_bytes()) {
                tracing::error!("Could not write the script samples: {err}");
            }
        }
    }
}
