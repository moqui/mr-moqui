# Native component extensions

> Archived 2026-10-16: built and merged (`9d2b1c1`, `ed04a7e`). The user guide is [docs/Native-Components.md](../../docs/Native-Components.md).

Implementation: `feat/native-components`. See the [guide](../../docs/Native-Components.md).

Service XML defines and discovers services. Linked Rust code supplies implementations; it must not define services by name or replace the XML invocation pipeline.

## Upstream model

- AWS: `tool-factory` creates shared S3/SNS clients; `resource-reference` maps `aws3`; service XML defines SMS/SNS operations which use those tools.
- SFTP: script-callable client and XML messaging services, not a ToolFactory.
- WikiText: `template-renderer` maps extensions to implementations, without services or tools.
- POI: `screen-output` selects a renderer. MR already has built-in XLSX rendering.
- FOP: tool, resource-facade integration and servlet. MR intentionally uses Chromium rather than FOP.
- Hazelcast: tools plus cache/topic/executor provider selection. Hazelcast replication remains deliberately out of scope.
- KIE: a tool obtained by application code; Drools itself remains out of scope.

Sources: sibling `moqui-framework/framework/xsd/{service-definition,moqui-conf}-3.xsd`, `ToolFactory.java`, and sibling `master/runtime/component/moqui-{aws,sftp,wikitext,poi,fop,hazelcast,kie}`.

## Implementation

1. A component owns a Cargo crate at its root. `./moq build` generates a binary crate in `target/moqui-app` that links the crates in `runtime/component` (`use my_component as _;`), as Gradle built Java components; an application can own that binary instead. A static linked catalogue (inventory) associates each crate's implementation-registration function with its component name. Only components discovered in the runtime contribute implementations, and a `component.xml` can be `disabled`. Rebuild for Rust changes; no dynamic loading or framework-source edits.
2. Build a factory-local immutable implementation registry. Support native function locations, custom service runners, tool constructors, resource providers and existing `TemplateRenderer` implementations. Reject duplicates. Catalogue entries supply code only: no service definitions, initialization, or handler selection.
3. Existing XML selects implementations. A tool factory can also bind a service location to a closure over its own state, on the factory, in `pre_facade_init`; it shares the lookup and the duplicate checks. Service XML remains authoritative for names, parameters, authentication, transactions, ECAs and outputs. Custom `service-type` uses a linked runner and receives location/method metadata. Keep framework Rust functions compatible.
4. `tool-factory` honors disabled and numeric priority, with deterministic ties. Run pre-init before entity attachment, init explicitly after facades are ready, and reverse-order shutdown. Tool state belongs to its factory. Rust retrieves typed instances; JS receives only explicit tool methods, properties and plain values, through a read-only object that holds the tool, so it survives the context stack and a method can return another tool. Support factory-presence checks separately from instance access.
5. `resource-reference` selects a linked provider for a scheme. Delegate reads, metadata, listing and writes to it, returning explicit unsupported-operation errors by default. `template-renderer` selects a linked renderer for an extension. Existing file/database/HTTP resources and built-in renderers remain intact.
6. Update local schemas/docs for native identifiers and arbitrary service types. Remove stale active Java-only tool declarations from MR defaults. Fail on unresolved custom implementations, rather than silently ignoring them.
7. Test from outside the core module: XML-defined native service without a tool; tool called from JS actions; resource/template provider without services; custom runner metadata; disabled tools, priority/order, duplicate/missing implementations, cleanup on failed init, isolated factories, and ordinary built-in service regressions. Use local fakes, not AWS credentials or external services.

## Verification

- `cargo test -p moqui-core`: 847 unit tests, 4 existing integration tests and 12 native-component tests pass; 1 pre-existing test ignored.
- `cargo check -p moqui-host -p moqui-browser --all-targets`: passes.
- `cargo clippy -p moqui-core -p moqui-host --lib -- -D warnings`: passes.
- `cargo clippy -p moqui-core --test native_components -- -D warnings`: passes.
- Updated XML/XSD files parse with `xmllint --noout`. Full schema compilation is blocked by existing nondeterministic content models in `xml-actions-3.xsd`, also present on the base branch.
- Full core all-targets Clippy encounters the existing `drop_non_drop` warning in `entity/sequence.rs:524`; no unrelated source changes are included for it.

## Boundaries

Keep XLSX, image, SFTP and IPP built in. This provides the extension points used by AWS-style components; it does not port AWS SDK calls, WikiText dialects, FOP/servlets, Drools or Hazelcast. Screen binary rendering retains MR's existing macro-based implementation rather than resurrecting Java widget classes. Native providers must enforce appropriate sim/outbound policies; do not expose arbitrary Rust objects to JavaScript.
