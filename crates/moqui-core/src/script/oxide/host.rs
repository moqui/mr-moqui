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

//! What the oxide runtime asks of Moqui (`Host`): the clock and the local time zone as QuickJS has them (the fork's
//! default is UTC), the thread's script deadline as the interrupt, and `print` to the log.

use std::sync::LazyLock;
use std::time::Instant;

use quickjs_oxide::engine::api::core::{Host, Stream};

use crate::script::run::DEADLINE;

pub(super) struct MoquiHost;

/// The process's time zone, as QuickJS's `localtime_r` reads it (`TZ`, else the system's).
static LOCAL_ZONE: LazyLock<jiff::tz::TimeZone> = LazyLock::new(jiff::tz::TimeZone::system);

impl Host for MoquiHost {
    fn write(&mut self, stream: Stream, text: &[u8]) {
        let text = String::from_utf8_lossy(text);
        match stream {
            Stream::Stdout => tracing::info!(target: "script", "{}", text.trim_end()),
            Stream::Stderr => tracing::warn!(target: "script", "{}", text.trim_end()),
        }
    }

    fn now_ms(&mut self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |elapsed| elapsed.as_millis() as f64)
    }

    /// UTC minus local time in minutes at `utc_ms`, `getTimezoneOffset`'s sign, from the process's zone rules.
    fn local_tz_offset_minutes(&mut self, utc_ms: f64) -> i32 {
        local_offset_minutes(utc_ms)
    }

    /// The thread's deadline ([`crate::script::DeadlineGuard`], the default limit) passed: the script stops, as the
    /// QuickJS interrupt handler stops it, but not the runtime's start. So does a script that met what the bindings do
    /// not have and caught it ([`super::engine::fallback_pending`]): it falls back.
    fn interrupt(&mut self) -> bool {
        // the runtime's own start runs to its end: a script's deadline is for the script ([`super::engine::starting`])
        if super::engine::starting() {
            return false;
        }
        super::engine::fallback_pending()
            || DEADLINE
                .with(std::cell::Cell::get)
                .is_some_and(|deadline| Instant::now() >= deadline)
    }
}

pub(super) fn local_offset_minutes(utc_ms: f64) -> i32 {
    if !utc_ms.is_finite() {
        return 0;
    }
    #[allow(clippy::cast_possible_truncation)]
    let Ok(at) = jiff::Timestamp::from_millisecond(utc_ms as i64) else {
        return 0;
    };
    -(LOCAL_ZONE.to_offset(at).seconds() / 60)
}
