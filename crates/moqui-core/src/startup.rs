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

//! What each listener and integration did at startup, for the one report the host logs when the server is ready
//! (`moqui_host::startup_report`). A subsystem records its state here instead of logging its own "started" or "not
//! starting" line; its warnings and errors are still logged where they happen. The report is a factory extension, so
//! the second factory after an empty-database load starts with an empty one.

use std::sync::{Arc, Mutex};

use moqui_lib::sync::lock;

use crate::context::ExecutionContextFactory;

/// Where an entry goes in the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// A port this process opens.
    Listener,
    /// A tool, interface or integration that is on or off.
    Integration,
}

/// What became of a listener or integration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Running; for a listener, the protocol and the bound address, then any detail.
    On(String),
    /// Not configured to run: the setting that keeps it off.
    Off(String),
    /// Configured but did not start: a short reason (the error itself is logged where it happened).
    Failed(String),
    /// Configured but not tried: its port was busy at the start check.
    Skipped(String),
    /// This build leaves it out.
    NotBuilt,
}

impl State {
    /// `on: <detail>` for an integration that is running.
    pub fn on(detail: impl Into<String>) -> Self {
        Self::On(detail.into())
    }

    /// `off: <reason>`.
    pub fn off(reason: impl Into<String>) -> Self {
        Self::Off(reason.into())
    }

    /// `failed: <reason>`.
    pub fn failed(reason: impl Into<String>) -> Self {
        Self::Failed(reason.into())
    }
}

/// One line of the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub section: Section,
    pub name: String,
    pub state: State,
}

/// The entries recorded so far, in the order they were first recorded.
#[derive(Default)]
pub struct StartupReport {
    entries: Mutex<Vec<Entry>>,
}

impl StartupReport {
    /// Record `name`'s state. A second record of the same name in the same section replaces the first, so a
    /// subsystem can record `On` and later `Failed`.
    pub fn record(&self, section: Section, name: impl Into<String>, state: State) {
        let name = name.into();
        let mut entries = lock(&self.entries);
        match entries
            .iter_mut()
            .find(|entry| entry.section == section && entry.name == name)
        {
            Some(entry) => entry.state = state,
            None => entries.push(Entry { section, name, state }),
        }
    }

    /// Record `name` only when nothing has been recorded for it yet.
    pub fn record_default(&self, section: Section, name: impl Into<String>, state: State) {
        let name = name.into();
        let mut entries = lock(&self.entries);
        if !entries
            .iter()
            .any(|entry| entry.section == section && entry.name == name)
        {
            entries.push(Entry { section, name, state });
        }
    }

    /// A copy of the entries.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        lock(&self.entries).clone()
    }
}

/// The factory's report.
pub fn report(factory: &ExecutionContextFactory) -> Arc<StartupReport> {
    factory.extension(|_| StartupReport::default())
}

/// Record an integration's state in the factory's report.
pub fn integration(factory: &ExecutionContextFactory, name: impl Into<String>, state: State) {
    report(factory).record(Section::Integration, name, state);
}

/// Record a listener's state in the factory's report.
pub fn listener(factory: &ExecutionContextFactory, name: impl Into<String>, state: State) {
    report(factory).record(Section::Listener, name, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_record_replaces_the_first_and_order_is_kept() {
        let report = StartupReport::default();
        report.record(Section::Listener, "SSH", State::On("tcp 127.0.0.1:2222".into()));
        report.record(Section::Integration, "Search", State::on("postgres"));
        report.record(Section::Listener, "SSH", State::failed("bind"));
        report.record_default(Section::Integration, "Search", State::NotBuilt);
        let entries = report.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].state, State::failed("bind"));
        assert_eq!(entries[1].state, State::on("postgres"));
    }
}
