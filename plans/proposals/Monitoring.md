# Monitoring Requirements

## Objective and scope

Provide a unified, technology-independent system that connects software changes, execution context, telemetry, source-level diagnostics, user and business impact, and recovery decisions.

Support backend services, browser, mobile and desktop applications, background workers, operating-system processes, containers, virtual machines, physical infrastructure, databases, networks, and AI-powered applications. Monitoring, diagnosis, and recovery must continue throughout each deployed version's lifetime, not only during releases.

## 1. Identify software, changes, and runtime exposure

- Record application/component identity, owner, team, source repository and revision, software version, immutable build artifact identity, build configuration, dependencies, and debugging symbols/source maps.
- Record every deployment, configuration, dependency, infrastructure, feature flag, AI model, and prompt change, including when it took effect.
- Track deployment environment, previous and candidate versions, rollout strategy, version overlap, and applicable recovery policy.
- Identify runtime instances and their lifecycle, host/device, operating system, infrastructure, executable version, and effective configuration and dependencies.
- Track which requests, users, sessions, and traffic populations encounter each version where measurable.
- Maintain authoritative identities and relationships so observations can be attributed to the exact software, deployment, runtime, and infrastructure without duplicating all metadata on every signal. Explicitly identify attribution uncertainty.

## 2. Collect and correlate telemetry

- Collect metrics, structured logs, distributed traces, profiles, stack traces, crash diagnostics, application/infrastructure events, real-user monitoring, session replay, business/product analytics, AI/LLM telemetry, network flows, network packet data, synthetic checks, security signals, audit records, change records, resource/topology data, and physical/device telemetry where applicable and supported.
- Capture relevant timestamps, identities, and context; normalize timestamps and account for clock differences.
- Preserve request, transaction, session, and causal relationships across clients, services, dependencies, networks, and infrastructure, including asynchronous tasks and message propagation where available.
- Support real-time analysis and historical investigation, including version-to-version comparisons.
- Validate schema compatibility, instrumentation coverage, and data quality; detect missing telemetry and broken correlation links.
- Apply sensitive-data filtering/masking, access controls, retention policies, and auditability throughout collection, investigation, and recovery.
- Support correlation of high-cardinality identities without requiring them as labels on every metric.

## 3. Capture failures and code-level performance

- Capture handled and unhandled exceptions, nested causes, hangs, deadlocks, resource exhaustion, crashes, and process termination reasons.
- Preserve available function/frame/module/source locations, asynchronous context, thread/task states, crash artifacts, and fault state. Retain symbols and source maps needed to resolve executable addresses to source code.
- Collect continuous or periodic CPU, memory, allocation, contention, and other supported profiles to attribute resource consumption to functions, threads, and execution paths.
- Link failures to profiles, logs, traces, active requests, dependency behavior, runtime state, and host/resource health before, during, and after the incident where possible.
- Connect frontend failures to application builds, devices, user sessions, and interaction history; connect backend failures to upstream callers and downstream dependencies.
- Use external diagnostics when a failing process cannot report, including abrupt termination, native crashes, out-of-memory kills, and host failures. Preserve partial evidence and explicitly show missing diagnostics.

## 4. Detect and quantify regressions

- Compare candidate versions against historical baselines, stable versions, or suitable control groups, using equivalent traffic, user populations, environments, and infrastructure where possible.
- Evaluate reliability, latency, resource consumption, security, user experience, business outcomes, and AI quality, latency, cost, and failure rates.
- Determine regression magnitude, duration, affected versions/instances/populations, severity, confidence, and data quality.
- Distinguish localized runtime/infrastructure faults from widespread version regressions, accounting for concurrent traffic, dependency, configuration, and infrastructure changes.
- Detect insufficient sample sizes and missing telemetry rather than interpreting them as evidence of health.
- Detect significant regressions without requiring a confirmed root cause first.

## 5. Diagnose probable root causes

- Identify the first observed deviation and preceding changes.
- Compare representative failing and healthy transactions, stacks, execution paths, logs, profiles, dependencies, resource usage, and source/configuration differences.
- Determine whether a fault originates in the affected component or propagates from a dependency, and identify affected user journeys and business transactions.
- Rank probable causes with supporting and contradicting evidence, relevant source revisions/functions/frames, timelines, and impact.
- Distinguish confirmed causes from correlation-based hypotheses and show unresolved uncertainty or missing evidence.
- Recommend relevant investigation or recovery actions.

## 6. Execute safe recovery

- Support continued observation, promotion, rollout pause, alerting, instance isolation, feature/configuration disablement, restart, rollback, and escalation for human approval.
- Select the least disruptive authorized action based on impact, confidence, recovery policy, permissions, sample volume, blast radius, and dependencies. Prefer targeted recovery for isolated faults.
- Permit automatic rollback only when sufficiently supported by evidence and verified as compatible with database schemas, persisted data, external interfaces, and dependent services.
- Escalate when no safe, authorized automatic action is available.
- Prevent repeated reversals, cascading actions, and conflicts between controllers.
- Preserve incident evidence and record the decision, rationale, policy, actor, authorization, execution history, and resulting deployment/runtime state.
- Execute recovery through existing deployment or operational controls.

## 7. Verify recovery and retain outcomes

- Verify that the intended action took effect and technical, user, and business indicators returned to acceptable behavior.
- Detect incomplete, failed, or harmful recovery; reopen or escalate unresolved incidents.
- Record the incident timeline, recovery outcome, resolution, corrective code changes, and confirmed root cause or unresolved status.
- Retain evidence for investigation, improvement, and compliance.
- Use incident findings to improve instrumentation, baselines, detection conditions, and recovery policies.

## 8. Provide a unified investigation experience

- Allow investigations to start from a version, regression, error, or incident rather than a telemetry storage type.
- Present severity, preceding changes, affected versions and infrastructure, technical/business impact, ranked causes, code-level evidence, distributed execution history, user experience, safe recovery controls, and recovery verification together.
- Allow direct navigation between errors, stacks, source code, profiles, traces, hosts, and deployment differences while preserving investigation context.
- Avoid requiring developers to manually combine evidence from independent monitoring systems.

## 9. Measure effectiveness

- Track detection time, cause-identification time, verified recovery time, regression detection precision/recall, automatic recovery success, false rollback rate, attribution coverage, symbolication coverage, profiling coverage, investigation effort, and business impact avoided.

---

## Operator UX: a 3 a.m. incident

- Usually no wake-up: authorized, reversible failures are contained or recovered automatically and verified against business health. Deduplicate/group alerts, suppress transient noise, and send a morning summary; page for critical integrity/security failures, unresolved business impact, failed recovery, or unsafe/unauthorized actions. Missing telemetry is not evidence of recovery.
- The page states what failed, severity, affected ERP operations/tenants, business impact, onset, current status, and recovery attempted/blocked, with one authenticated incident link and no sensitive document contents.
- That link opens one timeline: preceding build/config changes, affected versus healthy instances, baseline comparison, transaction outcomes, errors/stacks, granular JS hot functions/blocks, database/host evidence, ranked causes with uncertainty, and missing telemetry. Trace/profile/source links retain the incident context.
- Optional AI-assisted search: the operator asks “what changed, where is the JS bottleneck, and what is safe to do?” AI searches only authorized, tenant-scoped, redacted incident evidence and approved runbooks/source excerpts; answers cite evidence and flag uncertainty. Enforce filtering before indexing/model access, not just in prompts; exclude document contents, replay/raw dumps, credentials, arguments/locals, and secret-bearing source/config. Use pseudonymous references, audit searches/access, and never send raw ERP data to external models. AI has read-only scoped tools, no recovery authority; ordinary search and deterministic diagnosis remain available without AI.
- Show the next safe action, approval/compatibility checks, blast radius, and why automation stopped. The operator can pause/isolate/rollback through existing controls, then watch technical and business verification; never offer automatic reversal of committed business transactions.
- Faster than manually joining logs, dashboards, deployment history, and source: identity and causal links are already assembled, comparisons narrow the fault scope, and source-mapped evidence identifies where to investigate. Measure page-to-diagnosis and verified recovery time; do not promise instant fixes or hide uncertainty.

## Implementation delta

Assume Moqui and the other repository plans are implemented. Add only:

Suggested dependencies: `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp` (HTTP/Protobuf), and `tracing-opentelemetry` bridging existing `tracing`; `opentelemetry-proto` for direct OTLP decoding if needed. Reuse existing database/HTTP/UI libraries and OS crash tools. Add `pprof` for supported Rust/Linux CPU profiling and `rrweb` for browser replay only when enabled; OpenTelemetry Collector remains an optional external dependency. Pin compatible versions and review licenses; no additional servers required by default.

- Build/deployment/runtime history and source/symbol references; attach identity to existing artifact spans, hit records, and job runs.
- OpenTelemetry bridge and W3C context propagation across HTTP, async services, and jobs; bounded OTLP/HTTP Protobuf export/ingestion with redaction, backpressure, and loss counters. No synchronous telemetry writes in business transactions.
- Isolated telemetry schema/roles/pool, quotas, aggregation, and retention; restricted local artifact storage. Reuse PostgreSQL where configured without changing the ERP database; Collector optional.
- Error fingerprints, linked stacks, native profiles, external crash metadata, and source navigation; browser RUM and optional masked, bounded rrweb replay.
- Granular server-JS profiling: instrument generated JS at function and selected block/loop boundaries; transform handwritten JS with an AST parser (e.g. build-time `@babel/parser`, `@babel/traverse`, `@babel/generator`). Record call counts, inclusive/self elapsed time, host-call time, and sampled block counts through bounded hooks; preserve recursion, exceptions, returns, and async semantics. Map stable function/location IDs to JS/XML source and build; correlate selected spans/errors with the active trace, without capturing arguments/locals. Gate detailed instrumentation by script/request and time/overhead budget; timings are wall time, not CPU samples. Native `pprof` cannot resolve interpreted JS frames; true JS CPU/line sampling requires a separate QuickJS-NG engine-hook prototype or a verified runtime profiler.
- Stable business-operation names and actual commit/rollback/retry/pending outcomes from transaction hooks; authorized correlation references, not document contents or metric labels.
- Persisted baselines, regressions, incidents, and evidence; deterministic comparisons and cause ranking with sample/quality gates and explicit uncertainty.
- Incident/release/operation XML Screens linking existing tools to traces, stacks, profiles, source, and replay while preserving tenant/build/runtime/time context; separate diagnostic/recovery permissions and access auditing.
- Durable recovery policy/decision/execution/verification state machine and deployment-controller adapter; persist idempotent actions before execution, one active action per scope, compatibility checks, approvals, cooldowns, deadlines, emergency stop, and business-health verification. Default to advisory; never reverse committed business transactions automatically.
- Implement in the order above using existing crates, facades, entities, services, jobs, and UI. Gate completion on correlated failures, tenant/privacy isolation, telemetry-outage tolerance, measured overhead, regression detection, and safe/retried/failed recovery tests; defer other signal types and specialized infrastructure until needed.
