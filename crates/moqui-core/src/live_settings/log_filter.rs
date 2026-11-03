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

//! The process's log filter (plans/Monitor-Settings.md, Applying each group: Logging): the global level, the console
//! level and the target rules, as one snapshot the host's subscriber reads. The host installs the levels it started
//! with ([`install`]); the live settings replace the snapshot ([`set`]) and rebuild the callsites' cached interest.
//! The subscriber is the process's, so with two factories in one process the last change of either wins.

use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;
use tracing::Level;

/// Where the lines go on stderr.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Console {
    /// No stderr (`moqui tui`).
    Off,
    /// Every line the filter takes.
    All,
    /// The lines at this level and above.
    AtLeast(Level),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogFilter {
    pub level: Level,
    pub console: Console,
    /// Target prefixes and their least level, longest first, so the first match is the most specific.
    pub targets: Vec<(String, Level)>,
}

/// rsipstack logs each SIP message whole at INFO: SDES keys (a=crypto), digest answers, numbers. Its warnings and
/// errors carry none of them, so only those are taken, at any level and whatever a rule says.
pub const RSIPSTACK: &str = "rsipstack";

impl LogFilter {
    pub fn new(level: Level, console: Console) -> Self {
        Self {
            level,
            console,
            targets: Vec::new(),
        }
    }

    /// Whether a line of `target` at `level` is taken.
    pub fn enabled(&self, target: &str, level: Level) -> bool {
        if target.starts_with(RSIPSTACK) && level > Level::WARN {
            return false;
        }
        level <= self.least_for(target)
    }

    /// The least level `target` takes: its longest matching rule's, else the global one.
    pub fn least_for(&self, target: &str) -> Level {
        self.targets
            .iter()
            .find(|(prefix, _)| is_under(target, prefix))
            .map_or(self.level, |(_, level)| *level)
    }

    /// Whether a taken line at `level` is also written to stderr.
    pub fn to_console(&self, level: Level) -> bool {
        match self.console {
            Console::Off => false,
            Console::All => true,
            Console::AtLeast(least) => level <= least,
        }
    }

    /// The most verbose level any target takes, for the subscriber's `max_level_hint`.
    pub fn max_level(&self) -> Level {
        self.targets
            .iter()
            .map(|(_, level)| *level)
            .fold(self.level, std::cmp::max)
    }

    /// Put the target rules in match order: longest prefix first.
    pub fn sort_targets(&mut self) {
        self.targets
            .sort_by(|(a, _), (b, _)| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    }
}

/// `target` is `prefix` or below it: `moqui_core::service` covers `moqui_core::service::call`, and
/// `moqui.monitor` covers `moqui.monitor.audit`.
fn is_under(target: &str, prefix: &str) -> bool {
    match target.strip_prefix(prefix) {
        Some(rest) => rest.is_empty() || rest.starts_with("::") || rest.starts_with('.'),
        None => false,
    }
}

/// A level name as `moqui_log_level` and the settings write it; anything else is None.
pub fn parse_level(text: &str) -> Option<Level> {
    match text.trim().to_ascii_lowercase().as_str() {
        "trace" => Some(Level::TRACE),
        "debug" => Some(Level::DEBUG),
        "info" => Some(Level::INFO),
        "warn" | "warning" => Some(Level::WARN),
        "error" => Some(Level::ERROR),
        _ => None,
    }
}

pub fn level_name(level: Level) -> &'static str {
    match level {
        Level::TRACE => "trace",
        Level::DEBUG => "debug",
        Level::INFO => "info",
        Level::WARN => "warn",
        Level::ERROR => "error",
    }
}

/// `moqui_log_level`, default `info`.
pub fn level_from_env() -> Level {
    parse_level(&moqui_lib::env::get_prop_or_env("moqui_log_level")).unwrap_or(Level::INFO)
}

static DISK: OnceLock<LogFilter> = OnceLock::new();

fn live() -> &'static ArcSwap<LogFilter> {
    static LIVE: OnceLock<ArcSwap<LogFilter>> = OnceLock::new();
    LIVE.get_or_init(|| ArcSwap::from_pointee(disk()))
}

/// Record the levels the subscriber starts with. Only the first call counts, as only the first subscriber installs.
pub fn install(level: Level, console: Console) {
    if DISK.set(LogFilter::new(level, console)).is_ok() {
        live().store(Arc::new(disk()));
    }
}

/// The filter the process started with: what [`install`] recorded, or `moqui_log_level` on stderr before that.
pub fn disk() -> LogFilter {
    DISK.get()
        .cloned()
        .unwrap_or_else(|| LogFilter::new(level_from_env(), Console::All))
}

/// The filter now. Cheap: the subscriber calls it for each callsite it registers.
pub fn current() -> arc_swap::Guard<Arc<LogFilter>> {
    live().load()
}

/// Replace the filter, then have every callsite ask the subscriber again: it caches "always" or "never" per callsite.
pub(crate) fn set(mut filter: LogFilter) {
    filter.sort_targets();
    if **live().load() == filter {
        return;
    }
    live().store(Arc::new(filter));
    tracing::callsite::rebuild_interest_cache();
}
