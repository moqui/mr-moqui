# Monitor Settings

Plan, not started: timed runtime overrides of logging and telemetry settings in Monitor › Settings. It extends [Monitoring](Monitoring.md); the guide for what exists is [docs/tools/Monitoring.md](../docs/tools/Monitoring.md).

## Context

Today every logging and telemetry setting is fixed at startup:
- The log level is read once from `moqui_log_level` into `LineSubscriber` (`crates/moqui-host/src/logging.rs:132`).
- The telemetry `Config` sits in a per-factory `OnceLock` (`telemetry::for_factory`, `crates/moqui-core/src/telemetry/mod.rs:475`).
- Browser and receiver settings are re-read from the conf when used (`factory.conf_setting`), but nothing can change them.

To get full traces or DEBUG logs from a running server, an admin has to edit the conf and restart.

The goal: an admin with a new permission can change a fixed set of these settings from Monitor, for a limited time.

- **In memory only.** Every override expires on its own and is gone on restart.
- **The on-disk value stays visible.** The screen always shows it (from the merged conf XML, or the env for the log level), with a revert to it.
- **Files are changed elsewhere.** On production that means a git commit and the normal test and deploy.
- **Out of scope:** pre-planned sampling runs.

Decisions taken:
- An override applies to **all running cluster instances**.
- It **always expires**, capped at 24 h and extendable.
- v1 covers logging levels; sampling and artifact types; browser RUM and replay; receiver limits, metric attributes and operations.

## Model

New module `moqui_core::live_settings` (`crates/moqui-core/src/live_settings/`: `mod.rs`, `catalog.rs`, `log_filter.rs`, tests in `live_settings_tests.rs`).
- It sits in core because logging (host), telemetry (core), and browser and receiver (report and host) all read it, and cluster fan-out lives in core.
- It does not depend on the `telemetry` feature: logging overrides work in every build. Telemetry keys are left out of the catalog when the feature is off.

**Catalog.** A fixed list of keys. Nothing outside it can be overridden. Each entry has:
- a key in conf form: `path@attr`, or `path[selector]@attr`
- a value type (level, ratio 0–1, whole number with min and max, bool, enum, attribute list)
- how its on-disk value is read
- what applying it does

**Override.** `{ key, value, expiresAt (UTC ms), setBy, setAt, reason }`.
- Held in a per-factory registry (`factory.extension`).
- `effective(key)` returns the override while it is active, else the on-disk value.
- The on-disk value comes from `factory.conf_setting(path, attr)`, which is the merged and env-expanded conf. The log level comes from `moqui_log_level`.
- Each on-disk value is reported with its source: `conf`, `default` (attribute absent) or `env moqui_log_level`.
- Validation: the value is parsed and range-checked per type. The duration must be one of 15 min, 1 h, 4 h or 24 h, and `expiresAt` may be at most now + 24 h. Extend sets a new expiry under the same cap. Each failing key is refused with a message.

**Expiry.**
- One `moqui-live-settings` thread per factory sleeps until the next `expiresAt`, using `moqui_lib::sync::wait_timeout` on a Condvar that is woken on each change.
- When an override expires, the thread removes it, re-applies its group, and logs on `moqui.monitor.audit`.
- Every node expires its own copies, using the absolute `expiresAt` carried in the message.

**Cluster.**
- New topic `RUNTIME_SETTINGS = "runtime-settings"` in `crates/moqui-core/src/notify/topic.rs`, added to the known topics in `cluster::apply_remote` (`crates/moqui-core/src/cluster/mod.rs:248`).
- The body is `{op: "set"|"revert"|"revertAll", override?}`.
- The origin node applies the change, then calls `cluster::publish`. Peers validate against their own catalog and apply. A key unknown on a peer is logged and skipped.
- A node that starts later has on-disk values only; the screen says so.

**Applying each group.** Each group is rebuilt from its on-disk value plus the active overrides whenever one changes.

1. **Logging.**
   - `LineSubscriber` gets a `LogFilter` snapshot: global level, console level, and target rules `(prefix, Level)` sorted longest first. The snapshot is held in an `ArcSwap`.
   - `enabled()` takes the longest matching target prefix, else the global level.
   - Add `max_level_hint`. After each change, call `tracing::callsite::rebuild_interest_cache()`: the default `register_callsite` caches always/never per callsite.
   - The `rsipstack` rule (WARN and above) stays a fixed floor that no override can lower.
   - Target rules have no on-disk value: reverting one removes it.
   - The global level stops at DEBUG. TRACE is allowed only on a target rule.
   - Targets are validated as module paths or dotted targets (`[A-Za-z0-9_:.]`).
   - DEBUG and TRACE still go only to the console and files, never to `otel_logs` (`logging.rs:570`); the screen says this.
2. **Telemetry `Config`.**
   - `Shared.config` becomes `disk: Config` plus `live: ArcSwap<Config>`. `arc-swap` is a new workspace dependency, chosen for the per-span hot path.
   - The `self.shared.config.*` reads (about 30, in `telemetry/mod.rs` and `sdk.rs`) go through `self.config()`.
   - Keys:
     - `telemetry@sample-ratio`
     - `telemetry@slow-root-ms`: a new conf attribute, default 1000, replacing the hard-coded `slow_root`, so that it has an on-disk value. It goes in the XSD and the docs.
     - `telemetry/artifact[TYPE]@spans|metrics|sample-ratio`
     - `telemetry/operations@record|max-names`: changing either clears the resolved cache in `operation::Registry`.
     - `telemetry/metric[NAME]@attributes`: the `allowed` map copied into `sdk::Runtime` at start (`sdk.rs:122`) moves behind an `ArcSwap` too.
   - `enabled`, capacity, retention and the exporter are left out.
3. **Conf-read keys** (`telemetry/browser@enabled|sample-ratio|error-sample-ratio|replay-enabled|replay-sample-ratio`, `telemetry/receiver@enabled|requests-per-minute|body-limit`).
   - `ExecutionContextFactory::conf_setting` (`context/factory.rs:798`) checks the overlay first, behind an `AtomicBool` "any overrides" fast path.
   - Add `conf_setting_on_disk` for the screen.
   - The readers in `moqui-report/src/browser.rs` and `moqui-host/src/monitor/receiver.rs` stay unchanged.
   - Browser changes reach a tab on its next page load: `qapps.xml:101` decides whether to load the RUM script.
   - `receiver@concurrent-requests` is left out: its semaphore is built once (`receiver.rs:45`).

**Audit.** Each set, extend, revert, revert-all and expiry:
- logs a WARN on `moqui.monitor.audit` with the user, key, on-disk value, new value, expiry and reason;
- writes a `config` annotation (`moqui-report/src/annotations.rs` `create_annotation`), tagged `runtime-override` plus the key, from the origin node only. A set writes a band from `time` to `timeEnd` = expiry; an early revert or an extend writes a point. Incidents then list it as a change within the lookback.

## Services and permission

- `MONITOR_SETTINGS`, "Change logging and telemetry settings for a while (Monitor › Settings)": in `runtime/base-component/tools/data/MonitoringSecurityData.xml` beside `MONITOR_RECOVER_ADMIN`, granted to ADMIN. Like the recover permissions, it is not granted in `runtime/dev-component/demo/data/DemoSecurityData.xml`.
- Native services in `crates/moqui-report/src/settings.rs`, declared in `runtime/base-component/framework/service/org/moqui/monitor/MonitorServices.xml`, following `recovery/mod.rs` (`require(...)`, audit target):
  - `list#RuntimeSettings`: the catalog grouped, each with its on-disk value and source, effective value and override. Also the instance id and cluster member count. Needs Monitor access (`MONITOR_APP`).
  - `set#RuntimeSetting` (`key`, `value`, `minutes`, `reason` required): set or extend. Needs `MONITOR_SETTINGS`.
  - `revert#RuntimeSetting` (`key`) and `revert#AllRuntimeSettings`. Need `MONITOR_SETTINGS`.

## Screen: `runtime/base-component/tools/screen/Monitor/Settings.xml`

An XML screen like `Recovery.xml`, `default-menu-title="Settings"`, `default-menu-index="18"`. Its actions call `list#RuntimeSettings` and set `settingsAllowed = ec.user.hasPermission('MONITOR_SETTINGS')`. Without that permission the screen is read-only: no Change, Revert or Revert all.

The transitions are `setSetting`, `revertSetting` and `revertAll`, all with `default-response url="."`.

```
Settings                                                     [Revert all]  (danger, confirm; only with overrides)
  Instance a1b2… · 3 cluster members · overrides apply to all running instances
  On-disk values come from the merged conf (MoquiDefaultConf, component MoquiConf, runtime conf) and moqui_log_level;
  change them there and deploy. Every change here is in memory, expires on its own, and is gone on restart.

[Active overrides]                       (hidden when none)
  Setting | On disk | Now | Set by · reason | Expires (time, "in 42 min") | [Extend] [Revert to on-disk]

[Logging]
  Each row: Name | On disk (value · source) | Now | Expires | [Change…] [Revert to on-disk]
    Global level               info · env moqui_log_level
    Console level              info · as started
  Target rules                 (target | level | expires | set by | [Revert])      [Add a target rule…]
  Note: rsipstack is held at WARN. DEBUG and TRACE go to the console and log files, not to Monitor › Logs.

[Traces and sampling]
    Sample ratio               0 · conf (roots mode)
    Slow root (ms)             1000 · default

[Artifact types]   form-list, one row per type (AT_SCREEN, AT_SCREEN_CONTENT, AT_TRANSITION, AT_SERVICE,
                   AT_REST_PATH, AT_ENTITY, AT_LLM):
  Type | Spans (disk → now) | Metrics (disk → now) | Sample ratio (disk → now) | Expires | [Change…] [Revert]

[Metrics]          one row per instrument (http.server.request.duration, moqui.artifact.duration,
                   db.client.operation.duration, moqui.job.runs, moqui.operation.runs, moqui.operation.duration):
  Instrument | Attributes on disk (or "global list") | Now | Expires | [Change…] [Revert]

[Operations]
    Record                     default · conf
    Max names                  500 · default

[Browser]
    RUM enabled · Sample ratio · Error sample ratio · Replay enabled · Replay sample ratio
  Note: a tab picks up a change on its next page load.

[Receiver]
    Enabled · Requests per minute · Body limit
```

**Change… dialog** (`container-dialog` per row, or one dialog with the key hidden). Its fields:
- the setting's name and on-disk value, shown read-only;
- **Value**, with a typed widget: drop-down for levels, bools and enums; text-line for ratios, numbers and attribute lists;
- **For**: 15 min, 1 h (default), 4 h or 24 h;
- **Reason**, required;
- a submit button.

The dialog for an existing override says it will replace it and set a new expiry.

**Revert to on-disk** is a link, shown only when the row has an override.

The page follows the screen archetype rules in `docs/dev/Practices-App-Dev.md` §5: one primary action, danger plus confirmation on Revert all, phone columns on each form-list, and empty boxes hidden.

## Files

- New: `crates/moqui-core/src/live_settings/{mod,catalog,log_filter}.rs` and the tests, `crates/moqui-report/src/settings.rs`, `runtime/base-component/tools/screen/Monitor/Settings.xml`.
- Changed:
  - `crates/moqui-core/src/telemetry/{mod,sdk,operation}.rs` (live config)
  - `context/factory.rs` (`conf_setting` overlay)
  - `notify/topic.rs` and `cluster/mod.rs` (topic)
  - `crates/moqui-host/src/logging.rs` (`LogFilter`, `max_level_hint`, interest rebuild)
  - `Cargo.toml` (`arc-swap`)
  - `MonitorServices.xml`
  - `MonitoringSecurityData.xml`
  - the conf XSD (`slow-root-ms`)
  - `docs/tools/Monitoring.md`: a new section, "Changing settings for a while".

## Verification

All of these are fast tests; nothing marked slow is run.

- **Core unit tests** (`live_settings_tests.rs`):
  - effective vs on-disk, and the on-disk source
  - validation and refusal per type
  - the 24 h cap and Extend
  - expiry reverting at its time (an injected clock or a short expiry)
  - applying a cluster message body, and skipping an unknown key
- **Telemetry tests** (`telemetry/tests.rs`): an override of `AT_SERVICE@sample-ratio` and `sample-ratio` changes the ratio `start_artifact_span` uses, and a revert restores the conf value. An artifact `spans=false` override stops spans. A metric attribute override changes the kept attributes.
- **Host logging tests** (`logging_tests.rs`, `tap_at`):
  - a target rule at DEBUG lets that target's DEBUG lines through after the interest rebuild; others stay at INFO;
  - its revert stops them;
  - an override cannot lower the `rsipstack` floor.
- **Server spec** for the services: refused without `MONITOR_SETTINGS`; `list` shows on-disk and effective; set, then revert. Also a render spec of `Monitor/Settings`, as ADMIN and as a Monitor user without the permission (no Change controls).
- `cargo lint`, `cargo lint-min` (telemetry feature off), and `rustfmt --edition 2021` on the changed files.
- **Live check on the dev server:**
  1. Set the global sample ratio to 1 for 15 minutes and see full traces in Monitor › Traces.
  2. Add `moqui_core::service=debug` and see the DEBUG lines in `moqui.log`.
  3. Watch both revert at expiry, with the audit lines and the annotation band on a dashboard panel.

## Not in v1

- `telemetry@enabled`: the worker starts only when it is enabled at startup.
- Queue capacity and `receiver@concurrent-requests`: both are fixed at start.
- Retention.
- The OTLP exporter settings.
- Targeting a single instance.
- Planned sampling runs.
