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

//! The startup report: one block in the log when the server is ready, with every listener (the same ones the port
//! check named, `crate::port_check`), every integration, the tool factories, and what this build leaves out. Each
//! subsystem records its state in `moqui_core::startup` as it starts; this renders them. The block is a WARN when a
//! listener or integration that was configured did not start.

use moqui_core::context::ExecutionContextFactory;
use moqui_core::startup::{Entry, Ready, Section, State, LISTENER_ORDER};

/// What the report shows besides the entries.
pub(crate) struct Header {
    pub version: &'static str,
    pub package: &'static str,
    pub seconds: f64,
    pub purpose: String,
    pub role: String,
}

/// Record the host's integrations that have no start of their own, then log the report. `routes` is the HTTP
/// listener's routes, `listeners` false for a boot that opens none (`moqui tui`).
pub(crate) fn log(factory: &ExecutionContextFactory, routes: &str, listeners: bool) {
    record_integrations(factory);
    let property = |name: &str| moqui_lib::env::get_prop_or_env(name).trim().to_string();
    let header = Header {
        version: env!("CARGO_PKG_VERSION"),
        package: crate::build_package(),
        seconds: crate::process_started().elapsed().as_secs_f64(),
        purpose: property("instance_purpose"),
        role: property("node_role"),
    };
    let report = moqui_core::startup::report(factory);
    // scripts read the same report (ec.factory.getStartupReport(), plans/archive/Tools-Fancy.md B1)
    report.record_ready(Ready {
        seconds: header.seconds,
        routes: routes.to_string(),
        listeners,
    });
    let entries: Vec<Entry> = report
        .entries()
        .into_iter()
        .filter(|entry| listeners || entry.section != Section::Listener)
        .collect();
    let (text, warn) = render(&header, &entries, &factory.tool_names(), routes);
    if warn {
        tracing::warn!("{text}");
    } else {
        tracing::info!("{text}");
    }
}

/// The report's text, and whether something configured did not start.
pub(crate) fn render(header: &Header, entries: &[Entry], tools: &[String], routes: &str) -> (String, bool) {
    let mut lines = vec![format!(
        "Moqui {} ({}) ready in {:.3} seconds, instance purpose {}, node role {}",
        header.version,
        header.package,
        header.seconds,
        or_unset(&header.purpose, "unset"),
        or_unset(&header.role, "all")
    )];
    let built: Vec<&Entry> = entries.iter().filter(|entry| entry.state != State::NotBuilt).collect();
    let mut listeners: Vec<&Entry> = built
        .iter()
        .copied()
        .filter(|entry| entry.section == Section::Listener)
        .collect();
    listeners.sort_by_key(|entry| {
        LISTENER_ORDER
            .iter()
            .position(|name| *name == entry.name)
            .unwrap_or(LISTENER_ORDER.len())
    });
    let mut integrations: Vec<&Entry> = built
        .iter()
        .copied()
        .filter(|entry| entry.section == Section::Integration)
        .collect();
    integrations.sort_by_key(|entry| entry.name.to_lowercase());
    let width = built.iter().map(|entry| entry.name.len()).max().unwrap_or(0);
    for (title, section) in [("Listeners", &listeners), ("Integrations", &integrations)] {
        if section.is_empty() {
            continue;
        }
        lines.push(format!("  {title}"));
        for entry in section {
            lines.push(format!(
                "    {:<width$}  {}",
                entry.name,
                state_text(entry.section, &entry.state)
            ));
        }
    }
    if !tools.is_empty() {
        lines.push(format!("  Tools: {}", tools.join(", ")));
    }
    let mut not_built: Vec<&str> = entries
        .iter()
        .filter(|entry| entry.state == State::NotBuilt)
        .map(|entry| entry.name.as_str())
        .collect();
    not_built.sort_unstable();
    not_built.dedup();
    if !not_built.is_empty() {
        lines.push(format!("  Not in this build: {}", not_built.join(", ")));
    }
    if !routes.is_empty() {
        lines.push(format!("  Routes: {routes}"));
    }
    let warn = built
        .iter()
        .any(|entry| matches!(entry.state, State::Failed(_) | State::Skipped(_)));
    (lines.join("\n"), warn)
}

fn or_unset<'a>(value: &'a str, unset: &'a str) -> &'a str {
    if value.is_empty() {
        unset
    } else {
        value
    }
}

/// A listener that is on shows its address alone; an integration that is on says so.
fn state_text(section: Section, state: &State) -> String {
    match (section, state) {
        (Section::Listener, State::On(detail)) => detail.clone(),
        (Section::Integration, State::On(detail)) if detail.is_empty() => "on".to_string(),
        (Section::Integration, State::On(detail)) => format!("on: {detail}"),
        (_, State::Off(reason)) => format!("off: {reason}"),
        (_, State::Failed(reason)) => format!("failed: {reason}"),
        (_, State::Skipped(reason)) => format!("skipped: {reason}"),
        (_, State::NotBuilt) => "not in this build".to_string(),
    }
}

/// The host's integrations that are on by being built, or by a conf setting read when it is used, and the listeners
/// whose module is not in this build at all. A start that already recorded an entry keeps it.
fn record_integrations(factory: &ExecutionContextFactory) {
    let report = moqui_core::startup::report(factory);
    let built = |name: &str, on: bool| {
        report.record_default(
            Section::Integration,
            name,
            if on { State::on("") } else { State::NotBuilt },
        );
    };
    built("SSH client", cfg!(feature = "ssh"));
    built("SSO", cfg!(feature = "sso"));
    built("IPP printing", cfg!(feature = "print"));
    if !cfg!(feature = "pdf") {
        report.record_default(Section::Integration, "PDF", State::NotBuilt);
    }
    if !cfg!(feature = "grpc") {
        report.record_default(Section::Listener, "gRPC", State::NotBuilt);
    }
    if !cfg!(feature = "ssh") {
        report.record_default(Section::Listener, "SSH", State::NotBuilt);
    }
    if !cfg!(feature = "devtools") {
        report.record_default(Section::Integration, "Dev reload", State::NotBuilt);
    }
    if !cfg!(feature = "inbound-smtp") {
        report.record_default(Section::Listener, "SMTP", State::NotBuilt);
    }
    report.record_default(
        Section::Integration,
        "Telemetry",
        if !cfg!(feature = "telemetry") {
            State::NotBuilt
        } else if factory.conf_setting("telemetry", "enabled") == "false" {
            State::off("telemetry_enabled is false")
        } else {
            match factory.conf_setting("telemetry", "otlp-endpoint") {
                endpoint if endpoint.is_empty() => State::on("kept in the search store"),
                endpoint => State::on(format!("kept in the search store and sent to {endpoint}")),
            }
        },
    );
    record_agents(factory, &report);
    record_push(&report);
    record_calls(&report);
}

/// The LLM facade and the agent protocols on it: on by their `llm-facade` settings.
fn record_agents(factory: &ExecutionContextFactory, report: &moqui_core::startup::StartupReport) {
    let names = ["LLM", "MCP server", "A2A server", "LLM gateway"];
    if !moqui_core::llm_hooks::linked() {
        for name in names {
            report.record_default(Section::Integration, name, State::NotBuilt);
        }
        return;
    }
    let setting = |path: &str, name: &str| factory.conf_setting(path, name);
    let facade = setting("llm-facade", "enabled") != "false";
    let model = moqui_lib::env::get_prop_or_env("llm_openai_model").trim().to_string();
    report.record_default(
        Section::Integration,
        "LLM",
        match (facade, model.is_empty()) {
            (false, _) => State::off("llm-facade enabled is false"),
            (true, true) => State::on("no default model (llm_openai_model is not set)"),
            (true, false) => State::on(format!("default model {model}")),
        },
    );
    for (name, path, attribute, property) in [
        ("MCP server", "llm-facade/mcp", "server-enabled", "mcp_enabled"),
        ("A2A server", "llm-facade/a2a", "enabled", "a2a_enabled"),
        ("LLM gateway", "llm-facade/gateway", "enabled", "llm_gateway_enabled"),
    ] {
        let on = facade && setting(path, attribute) == "true";
        report.record_default(
            Section::Integration,
            name,
            if on {
                State::on("")
            } else if facade {
                State::off(format!("{property} is not true"))
            } else {
                State::off("llm-facade enabled is false")
            },
        );
    }
}

/// Web push and FCM: on when their keys are set.
fn record_push(report: &moqui_core::startup::StartupReport) {
    let set = |name: &str| {
        let value = moqui_lib::env::get_prop_or_env(name);
        !value.trim().is_empty() && !value.ends_with("CHANGEME")
    };
    report.record_default(
        Section::Integration,
        "Web push",
        if !moqui_core::build_info::has_feature("web-push") {
            State::NotBuilt
        } else if set("web_push_vapid_key") {
            State::on("VAPID key set")
        } else {
            State::off("web_push_vapid_key is not set")
        },
    );
    report.record_default(
        Section::Integration,
        "Mobile push (FCM)",
        if set("fcm_service_account") {
            State::on("")
        } else {
            State::off("fcm_service_account is not set")
        },
    );
}

/// Call recording and captions: on by their properties, while call media runs.
fn record_calls(report: &moqui_core::startup::StartupReport) {
    let names = [
        ("Call recording", "call_recording_enabled"),
        ("Call captions", "call_captions_enabled"),
    ];
    if !cfg!(feature = "rtc") {
        for (name, _) in names {
            report.record_default(Section::Integration, name, State::NotBuilt);
        }
        return;
    }
    let media = report
        .entries()
        .iter()
        .any(|entry| entry.name == "Call media" && matches!(entry.state, State::On(_)));
    for (name, property) in names {
        let on = moqui_lib::env::get_prop_or_env(property).trim() == "true";
        report.record_default(
            Section::Integration,
            name,
            match (on, media) {
                (false, _) => State::off(format!("{property} is not true")),
                (true, false) => State::off("call media is not running"),
                (true, true) => State::on(""),
            },
        );
    }
}

#[cfg(test)]
#[path = "startup_report_tests.rs"]
mod tests;
