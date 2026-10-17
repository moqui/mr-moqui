# Implementation Plan: Runtime-Reflective FlatBuffers for Moqui

Architecture specification · Rust server · Browser client · Dynamic schemas · Binary WebSockets · Large-scale fan-out

## 1. Executive decision

Build a schema-driven binary document system for internal communication using FlatBuffers, with runtime schema reflection, immutable schema versions, and shared message buffers.

FlatBuffers is a first-class request/response format for any Moqui application endpoint, not an allowlist of routes. Service calls, screen transitions, REST APIs, `/actions` and document subscriptions are examples of that universal endpoint contract. Binary callers send and receive FlatBuffers instead of JSON, using the same authoritative operations and precached JavaScript. Existing REST routes, JSON and other supported formats remain available for compatibility; clients choosing those formats do not require `.bfbs` reflection.

The application will support:

- New Moqui Data Document types created without redeploying the server or client.
- Arbitrarily deep, nested, strictly typed document structures, subject to configurable safety limits.
- Separate read and write schema compatibility policies.
- Direct, zero-copy reads of FlatBuffers from received binary buffers.
- Runtime schema synchronization between Rust and browser clients.
- 100,000 incoming document updates per second.
- Fan-out toward as many as 10 million concurrent browser connections.
- Minimal handwritten protocol code, using existing FlatBuffers tooling wherever practical.

### Proposed technology stack

| Component                | Decision                                                |
| ------------------------ | ------------------------------------------------------- |
| Application server       | Rust                                                    |
| Async networking         | Tokio                                                   |
| HTTP/REST/WebSocket      | Axum, initially; preserve existing Moqui REST contracts  |
| Binary representation    | FlatBuffers                                             |
| Runtime schema           | FlatBuffers `.bfbs`                                     |
| Schema compiler          | `flatc`                                                 |
| Rust reflection          | `flatbuffers-reflection`, extended only where necessary |
| Browser client           | TypeScript UI + Emscripten-generated JavaScript bindings |
| Browser reflection       | Shared Rust reader/writer compiled to Emscripten WASM    |
| Schema registry          | Authoritative database + immutable in-memory caches     |
| Fan-out payload          | `bytes::Bytes` with shared ownership                    |
| Document synchronization | Snapshots + revisioned patches                          |
| Schema synchronization   | Versioned publication and on-demand retrieval           |

The key architectural decision is to separate the stable networking protocol from dynamically defined document and service-parameter schemas, and keep transport/format adapters separate from application operations. Internal service callers natively encode declared inputs as FlatBuffers; service workers expose verified inputs through lazy QuickJS host getters and execute precached JavaScript without first materializing a parameter map or JSON object graph. Declared outputs are encoded natively as FlatBuffers.

FlatBuffers officially supports binary schemas as a runtime intermediate representation. Its Rust reflection support exists, but is still described as experimental, so production qualification of that dependency is a required milestone—not something to assume complete.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://flatbuffers.dev\&sz=32)

FlatBuffers Docs

+1



## 2. Non-negotiable architectural constraints

These requirements determine which implementations are acceptable.

| Constraint             | Required behavior                                            |
| ---------------------- | ------------------------------------------------------------ |
| REST compatibility    | Preserve REST APIs, JSON and other supported formats         |
| Zero-copy data access  | Read fields directly from internal binary buffers            |
| Runtime type creation  | New document schema available without recompilation          |
| Strict types           | Server verifies exact field types and constraints            |
| Schema consistency     | A schema ID always identifies the same immutable definition  |
| Browser compatibility  | No native browser extensions or required C++ runtime         |
| Production safety      | Untrusted messages fully verified before field access        |
| Bounded memory         | No unbounded message, subscription or write queues           |
| Backward compatibility | Version-aware reading and controlled write migration         |
| Large-scale fan-out    | Encode an update once per payload variant, not per recipient |

### Existing execution model

Follow `plans/archive/Architecture-Proposal.md`, `plans/archive/Crate-Layout.md`, and `plans/archive/ContextObjects.md`. Tokio/Axum in `moqui-host` owns connections, framing and outbound queues. Application operations go through the factory's bounded service worker pool; each worker uses its thread-local `ExecutionContext` and QuickJS runtime, and framework calls remain synchronous. No database or JavaScript business logic runs on Tokio tasks. A connection never retains an execution context, transaction or JavaScript value. Queue saturation produces an explicit overload response.

### Important limits

There are four constraints I would explicitly document.

First: zero-copy means zero-copy structured reads, not guaranteed zero-copy networking. Browser `ArrayBuffer` delivery, TLS encryption and kernel networking can involve copies.

Second: FlatBuffers are not mutable general-purpose object graphs. Some scalar fields can be changed in place, but adding nested objects or resizing strings usually requires rebuilding the message.

Third: schema reflection does not eliminate schema validation. A `.bfbs` descriptor describes a binary structure. Moqui constraints, authorization and write semantics still need enforcement.

Fourth: 100,000 incoming updates per second is fundamentally different from 100,000 updates delivered to millions of recipients. The latter can generate billions of network deliveries per second.

These constraints must remain visible throughout implementation.

## 3. Design the system as two planes

Control plane

Infrequent, consistency-critical

Schema definitions

Validation metadata

Version publication

Schema compatibility

Client readiness

Data plane

High-frequency, latency-critical

WebSocket messages

Document reads/writes

Subscriptions

Binary fan-out

Backpressure

Immutable schema IDs connect both planes

A schema update should never force the entire high-throughput data path to pause.

Conversely, the data plane must never process a message using a schema version that is missing, unverified, or ambiguous.

### System topology

Moqui metadata and document definitions

Authoritative field and relationship information

Schema publisher — Rust

Generate .fbs · Compile .bfbs · Validate · Publish

Immutable schema registry

Schema IDs, versions, fingerprints, constraints

Rust Gateway 1

Schema cache, routing, WebSocket fan-out

Rust Gateway 2

Schema cache, routing, WebSocket fan-out

Rust Gateway N

Schema cache, routing, WebSocket fan-out

Binary WebSockets

Browser clients

Emscripten WASM reflection · Schema cache · Lazy document views

Do not introduce a separate distributed schema-registry service initially unless the existing database cannot provide atomic schema publication. A versioned table and process-local immutable caches are sufficient to begin.

## 4. Repository and module design

Use the existing workspace and five-crate cut in `plans/archive/Crate-Layout.md`, not a separate messaging workspace.

```text
crates/moqui-lib/src/wire/        # codecs, descriptors, reflection, binary verification
crates/moqui-core/src/document/  # document operations, revisions, schema policy
crates/moqui-core/src/service/   # service schemas, parameter bindings, cached JS invocation
crates/moqui-host/src/gateway/   # sockets, routing, fan-out, backpressure, REST adapters
crates/moqui-host/src/schema/    # restricted flatc process execution
```

These are logical modules inside the existing crates, not new independently deployed services. Framework-independent descriptors and readers/writers belong in lib; Moqui metadata resolution, authorization, registry persistence and compatibility policy belong in core. Host supplies compiler process execution. Schema publication is a command of the existing `moqui` binary, not a separate executable. Browser JavaScript bindings, worker/host adapters and document-store modules live in the existing browser source tree; the shared reflection implementation stays in Rust lib and is compiled into the WASM module. Keep protocol schemas, cross-language fixtures and benchmarks alongside those modules.

The essential interfaces should be small:

```

pub struct SchemaId(pub u64);

pub struct DocumentRevision(pub u64);

pub trait SchemaRegistry {
    fn get(&self, id: SchemaId)
        -> Option<&RuntimeSchema>;
}

pub trait DocumentReader {
    fn get_field(&self, field_id: u16)
        -> Result<FieldView<'_>, ReadError>;
}

pub trait DocumentWriter {
    fn encode(
        &self,
        schema: &RuntimeSchema,
        value: &DocumentInput,
    ) -> Result<bytes::Bytes, WriteError>;
}

```

These are proposed interfaces, not existing FlatBuffers APIs. In practice, `DocumentReader` may need to be implemented on a table-view type rather than a whole document, and `SchemaRegistry` may use owned/shared handles to accommodate cache reloads.

Keep the application unaware of raw FlatBuffers offsets. Only the wire and reflection modules should manipulate them.

## 5. Schema representation: actual `.bfbs`, not generic document values

This is the most consequential decision.

Use dynamically generated, specialized FlatBuffers tables for Moqui documents.

Do not encode every field through a generic structure such as:

```
Field
  name
  type
  value
```

That would reintroduce per-field metadata, type dispatch and additional pointer traversal.

Instead, when Moqui defines:

```
Order
  orderId: String
  total: Decimal
  items: List<OrderItem>
```

Generate a genuine FlatBuffers schema:

```

namespace Moqui;

table Decimal {
  coefficient_low:ulong;
  coefficient_high:uint;
  negative:bool;
  scale:ubyte;
}

table OrderItem {
  productId:string (required);
  quantity:long;
  amount:Decimal;
}

table Order {
  orderId:string (required);
  total:Decimal;
  items:[OrderItem];
}

root_type Order;

```

This gives every document a real binary layout. `Decimal` matches `rust_decimal`: unsigned 96-bit coefficient (`high << 64 | low`), sign, and scale 0–28; TypeScript uses `bigint` for the coefficient.

### Type mappings

| Moqui semantic type         | FlatBuffers representation                              |
| --------------------------- | ------------------------------------------------------- |
| String / ID                 | `string`                                                |
| Integer                     | `int`, `long`, etc.                                     |
| Boolean                     | `bool`                                                  |
| Floating point              | `float` / `double`                                      |
| Exact decimal               | `Decimal` table: 96-bit coefficient, sign, scale 0–28   |
| Date                        | Signed integer days since epoch                         |
| Timestamp                   | Signed integer microseconds since Unix epoch            |
| Enum                        | FlatBuffers `enum`, if definitions are stable           |
| Nested object               | `table`                                                 |
| List of objects             | Vector of tables                                        |
| Optional scalar             | Optional scalar supported by chosen FlatBuffers version |
| Polymorphic object          | `union`                                                 |
| Arbitrary binary data       | `[ubyte]`                                               |
| Reference to another entity | Explicit typed reference field/table                    |

Dates, decimals and references need application-level semantic definitions, because their meaning is not fully captured by primitive FlatBuffers types.

For money, never default to `float` or `double` when exact decimal arithmetic is required.

### Moqui integration

Moqui stores Data Document field paths and relationship aliases. The source definitions can also contain expressions.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://github.com\&sz=32)

GitHub



I would map them as follows:

- A scalar field path maps to a schema field with an assigned persistent ID.
- A one-to-one relationship maps to a nested table.
- A one-to-many relationship maps to a vector of nested tables.
- Aliases affect display names, not the persistent field's identity.
- Computed expressions require an explicitly declared result type.
- Document fields unsupported by the runtime type system are rejected during schema publication.

Do not infer permanent type identities from display names or relationship aliases. Those may change while the field's semantic identity stays the same.

### Service invocation schemas

Service XML and its JavaScript XML-action implementation jointly define the service contract. Generate separate immutable input and output schemas from declared service parameters, including nested maps/lists where their structure is declared, and reconcile them with the parameter reads, assignments, result writes and service calls emitted by the JavaScript XML-action macros. Schema generation runs after action expansion, not independently of it. The macros emit contract metadata alongside generated JavaScript; use the existing JavaScript AST validation pass to check that generated accesses agree with that metadata. Carry parameter constraints as validation metadata; do not infer permanent types from observed requests or assume arbitrary JavaScript can be statically typed. Undeclared fields or ambiguous types require an explicit declaration or supported typed-value representation before a specialized binary contract is published. Unspecified nested structures otherwise remain on the existing map/JSON path.

An invocation identifies the service, its input schema and requested output schema, and the request ID. A schema ID alone is not authority to invoke a service. Validate that both schemas belong to the selected service and are accepted by its current policy. Defaults and declared conversions retain existing service semantics; strict wire-type validation does not remove those semantics. Authenticate and authorize through the existing service facade.

Native Rust callers encode from typed Rust values through the schema-driven builder, without an intermediate JSON representation. Browser callers use the dynamic writer. Trusted local callers may retain the existing direct invocation path; binary encoding is not mandatory for every in-process facade call.

### Stable field IDs

Every schema publisher must preserve numeric field IDs.

FlatBuffers permits explicit field IDs and uses those IDs to support controlled reordering. Its schema-evolution documentation specifically recommends explicit IDs when concurrent schema changes are a concern.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://flatbuffers.dev\&sz=32)

FlatBuffers Docs



Maintain an ID registry so a deleted field's ID can never be recycled.

## 6. Schema publication and live evolution

### Authoritative registry model

Use two records conceptually:

```

CREATE TABLE document_schema (
    schema_id BIGINT PRIMARY KEY,
    document_type TEXT NOT NULL,
    version INTEGER NOT NULL,
    fingerprint BYTEA NOT NULL UNIQUE,
    bfbs BYTEA NOT NULL,
    constraints BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    UNIQUE(document_type, version)
);

CREATE TABLE document_schema_policy (
    document_type TEXT PRIMARY KEY,
    active_read_schema BIGINT NOT NULL,
    active_write_schema BIGINT NOT NULL,
    policy_generation BIGINT NOT NULL
);

```

Use separate immutable compatibility-policy records or a versioned policy table for accepted historical versions, migrations and rollout state. The two tables above are only the core schema storage. Extend registry identity and policy to include schema kind (document, service input, service output) and owner (document type or service name); service input/output schemas use the same immutable IDs, fingerprints and publication rules.

Store the `.fbs` source as well, for diagnostics, auditability and compatibility checks.

### Schema publication algorithm

1. Capture one consistent Moqui metadata snapshot.

   Resolve the document's fields, relationships, types, expressions and constraints before starting compilation.
2. Assign stable field identities.

   Preserve IDs from the previous version. Allocate new IDs transactionally for new fields.
3. Generate `.fbs` and compile with `flatc`.

   Produce a `.bfbs` descriptor using the pinned FlatBuffers compiler version.
4. Run compatibility checks.

   Use `flatc --conform` against the previous schema. Reject incompatible changes from the same compatibility lineage.
5. Validate the descriptor and application constraints.

   Check supported features, recursion limits, field counts, nesting, schema size and domain types.
6. Publish atomically.

   Persist the immutable `.bfbs`, associated constraints and fingerprint. Then atomically advance the active-schema policy.
7. Notify gateway caches.

   Gateways fetch and install the new descriptor. They do not regenerate code or restart.
8. Notify only relevant browsers.

   Subscribers load the descriptor, prepare the runtime view, then acknowledge readiness.

The compiler supports `.bfbs` emission and the `--conform` compatibility check directly.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://github.com\&sz=32)

GitHub



For incompatible changes, create a new schema lineage or explicit migration, instead of forcing the new schema through `--conform`.

### Schema version transitions

A safe live deployment looks like this:

Example: Order schema v12 → v13

Before publication

Read v12

Write v12

Transition period

Read v12, v13

Write v12, v13 with policy

After migration and compatibility window

Preferred read/write v13

v12 retained only while historical data or clients require it

For incompatible write versions, do not silently reinterpret the bytes. Reject the write with a version-conflict error or run an explicitly defined migration.

### Avoid per-connection schema maps

Ten million connections do not need ten million copies of schema definitions.

Use:

- One immutable descriptor cache per gateway process.
- A small active-schema generation marker per connection.
- Lazy retrieval of actual schema descriptors.
- Compact schema identifiers in ordinary data messages.

If a schema update affects 100,000 users, one gateway should perform one registry fetch, not 100,000 independent fetches.

## 7. Implementing the Rust reflective reader

Use `flatbuffers-reflection::SafeBuffer` as the initial baseline.

The current implementation validates the bytes against a supplied runtime schema and provides a safe root-table view.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://docs.rs\&sz=32)

Rust

+1



Conceptually:

```

use flatbuffers_reflection::SafeBuffer;

fn validate_and_open<'a>(
    schema: &'a RuntimeSchema,
    bytes: &'a [u8],
) -> Result<ValidatedDocument<'a>, Error> {
    // 1. Verify FlatBuffers structure using
    //    schema.bfbs_descriptor().
    //
    // 2. Check exact Moqui field types and
    //    constraints using schema.constraints().
    //
    // 3. Return a view borrowing the bytes.
    //
    // No full document materialization.
    todo!()
}

```

This is architectural pseudocode; `RuntimeSchema` and `ValidatedDocument` are application types.

### Reader responsibilities

Implement:

| Function                 | Behavior                                          |
| ------------------------ | ------------------------------------------------- |
| `open(schema, bytes)`    | Verify a binary buffer                            |
| `get(field_id)`          | Read the exact declared field                     |
| `get_table(field_id)`    | Return a borrowed nested table view               |
| `get_vector(field_id)`   | Return a borrowed vector view                     |
| `get_string(field_id)`   | Return a borrowed UTF-8 string slice              |
| `get_decimal(field_id)`  | Read exact decimal representation                 |
| `is_present(field_id)`   | Distinguish omitted fields where permitted        |
| `validate_constraints()` | Enforce Moqui-specific structural and value rules |

Avoid coercing values through generic integer or floating-point accessor methods.

The official reflection API contains coercing `get_any_*` functions and exact-type variants. Your validator should prefer the latter.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://docs.rs\&sz=32)

Rust



### Reflection cache

The first time a gateway loads a schema, transform its reflection metadata into a compact cached descriptor.

For example:

```
CompiledSchema
  root_type
  fields_by_id
  fields_by_name
  field_types
  vtable_offsets
  nested_type_indices
  constraints
```

Do this once per schema version, not once per document.

The cache should be immutable and reference-counted, so live schema updates do not invalidate readers using older versions.

### Important performance detail

The current Rust `SafeBuffer` implementation constructs a `HashMap` when verifying a buffer, tracking mappings from buffer locations to reflected object indices.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://docs.rs\&sz=32)

source



That means zero-copy is not equivalent to zero allocation.

The proposed plan should benchmark this cost early. It may be acceptable for inbound writes, where validation is mandatory, but expensive for very high-frequency small messages.

Do not bypass verification merely to hit a throughput number. If necessary, optimize or replace the verification implementation while preserving its safety properties.

### Binary-backed parameters and precached JavaScript

The internal service path is:

```text
Native/browser FlatBuffers input
→ bounded worker handoff
→ structural verification against cached service schema
→ service validation, authorization and default handling
→ lazy parameter bindings on the worker's QuickJS context
→ precached service JavaScript
→ native FlatBuffers output encoding
```

Apply framing and message-size limits before enqueueing. Complete structural verification before any field access; expensive verification must not block the socket loop. The worker owns the immutable input buffer and schema throughout execution. Cache descriptors process-wide, but cache JavaScript functions in each worker's own QuickJS runtime; functions and JS values never cross threads. Track dependencies on service XML, imported action macro modules and script bodies. When an XML-action JavaScript implementation changes, regenerate affected actions and contract metadata, reconcile and compile their FlatBuffers schemas, run compatibility checks, and compile the affected service functions. Publish a coherent service generation linking generated JavaScript, input/output schema IDs and binding metadata. Workers switch generations only at safe invocation boundaries; in-flight calls retain their original generation, and a failed rebuild leaves the last valid generation active. Unchanged contracts may reuse immutable schema IDs, but changed layouts receive new IDs. Never execute new JavaScript against stale parameter bindings.

Bind input parameter names through the existing context-stack mechanism, preserving bare-name resolution, enumeration and missing-name behavior. Nested tables and vectors are lazy, read-only host views. Accessors perform declared exact-type reads and expose decimals and timestamps as the existing `rust_decimal` and `jiff` host values, not JavaScript numbers. Strings still require decoding into JavaScript strings, and view wrappers may allocate; this is allocation-reduced access, not completely zero-copy JavaScript execution.

FlatBuffers input is immutable. Assignments write the normal top-map overlay, which takes precedence over binary-backed input. Defaults and validated conversions live in that overlay rather than changing received bytes. Whole-parameter assignment is supported; nested mutation must use an explicit materialized copy rather than silently changing a binary view. Adapters materialize values when an existing facade requires owned maps/lists. `EntityValue`, `EntityList`, and the live context remain their existing Rust-backed host objects, not FlatBuffers tables.

Bindings are scoped to the service call and restored when it returns, including nested service calls and error paths. Clear them during context cleanup. Any view retained by application JavaScript must either own shared references to the verified bytes and descriptor or become explicitly invalid after the call; it must never borrow freed request memory. Bound retained-input memory under the runtime's configured resource budgets.

Validate declared outputs and encode directly from returned Rust/QuickJS values using the native builder, without routing through JSON. Encoding may allocate and traverse the result; FlatBuffers does not make arbitrary JavaScript result objects zero-copy. Binary REST, screen-transition and `/actions` callers use this same binding and encoding path. Existing JSON callers retain their parameter and result contracts through the same authoritative facades.

## 8. Implementing the dynamic browser reader and writer

### Shared Rust runtime through Emscripten

Follow `plans/archive/Wasm.md`: compile the portable Rust core with `wasm32-unknown-emscripten`, and use Emscripten-generated JavaScript bindings for browser host integration, following the SQLite WASM pattern. Share the schema-driven FlatBuffers verifier, reader, writer and declarative constraints with the server rather than maintaining a separate TypeScript reflection implementation. TypeScript remains the UI and transport integration layer.

Run the module in a dedicated Web Worker so verification, encoding and local framework execution do not block the UI. The flow is:

```text
Browser WebSocket/fetch → bounded worker message queue
→ copy binary input into WASM-owned memory
→ shared Rust schema verification and lazy field views
→ local operation or UI projection
→ Rust FlatBuffers encoding → JavaScript transport → authoritative server
```

Transfer `ArrayBuffer`s between workers where possible; transferring ownership does not eliminate the copy into WASM linear memory. Keep verified buffers and immutable descriptors alive behind explicit handles, expose field/batch access through generated bindings, and release handles when consumers finish. Do not retain JavaScript typed-array views across WASM memory growth without refreshing them. Structured reads are zero-copy within WASM memory, not end-to-end from browser networking; string decoding and UI projection still allocate. Bound worker queues, retained buffers and schema/storage caches.

Emscripten supplies the interop machinery, not automatic portability of Tokio, native database drivers or thread-local transactions. Browser host adapters must explicitly provide:

- **Networking:** JavaScript WebSocket/fetch adapters carry the same binary envelopes, schema IDs, request IDs and revision rules as native callers. Tokio/Axum remains server-only.
- **Async execution:** Asyncify or JSPI suspends/resumes WASM when an adapter awaits a browser operation. Qualify browser support and build settings; do not assume native Tokio spawning works. Keep each execution context and transaction owned by its worker, serialize access while suspended, and clean up on completion, cancellation or failure.
- **Storage:** An explicit filesystem/database adapter uses OPFS for local schema caches, snapshots and optional offline state, following the SQLite WASM pattern. Emscripten does not automatically make every filesystem write durable in OPFS. Define flush, quota, eviction and recovery behavior; local transactions are not server transactions.
- **Threads:** Start with one execution worker. If pthreads are required, qualify Rust dependencies and Emscripten proxying, and serve the cross-origin-isolation headers needed for shared memory; retain a non-pthread path for unsupported deployments. JavaScript and execution-context values never cross workers directly.

Only browser-portable framework operations run locally; server-only facades return explicit unsupported-operation errors or invoke authorized remote operations. Local validation and cached state never grant authority to commit writes: the server repeats verification, authorization, revision checks and transactional validation. Offline mutations are pending requests, not committed revisions; reconnect submits them through the existing conflict/idempotency path. `flatc` and schema publication remain server-side; browsers load immutable `.bfbs` without rebuilding WASM.

### Reader and writer bindings

The browser package needs two distinct capabilities.

### Reader

Load `.bfbs` through the shared Rust descriptor parser in WASM. Generated JavaScript bindings expose schema and document handles, not generated bindings for each dynamic document.

The browser reader then performs these operations:

```
const schema = await registry.get(schemaId);
const document = runtime.open(schema, arrayBuffer);

// Direct views over the received buffer
const name = document.getString("name");
const items = document.getVector("items");

const quantity = items.getTable(0).getInt64("quantity");
```

This is the desired custom API.

Do not generate a JavaScript object graph during `open()`.

The WASM runtime stores the buffer, table location and schema descriptor; JavaScript holds an opaque document handle. A nested field access creates a lightweight view. The example API is an asynchronous worker facade; batch field reads where possible to avoid one worker round trip per scalar.

### Writer

A dynamic writer is more involved because FlatBuffers construction involves offsets and reverse-order buffer building.

I would implement a schema-driven recursive encoder:

```

encode(table_type, input):
    validate input against runtime schema
    encode child strings and nested tables
    encode vectors of children
    start FlatBuffers table
    write scalar fields
    write child offsets
    finish table
    return table offset

```

The shared Rust FlatBuffers builder performs binary layout construction in both native and WASM builds. The custom layer decides which field operations to invoke based on `.bfbs`; JavaScript bindings pass declared inputs without a JSON serialization intermediate.

### Limit the supported features deliberately

For the first production version, support:

- Tables, including nested tables and recursive references.
- UTF-8 strings.
- Boolean, signed/unsigned integers, floats and doubles.
- Optional values and explicitly defined null semantics.
- Vectors of scalars, strings and tables.
- Explicit decimal and timestamp conventions.
- Enumerations.
- Controlled unions for known polymorphic relationships.

Postpone rarely needed features such as arbitrary unions, specialized fixed-layout structs, arrays and 64-bit vector layouts until the real Moqui metadata requires them.

For large integer values, JavaScript bindings must preserve `bigint`, not coerce to JavaScript `number`; qualify Emscripten's 64-bit interop settings. Rust retains exact integer and decimal types inside WASM.

### Existing screen compatibility

Binary service calls and document subscriptions do not replace the existing `qvt` package or its plain-data projection. Wire views retain exact types internally; data entering the existing screen model is explicitly projected into its established representation, including decimal strings and ISO-8601 timestamps. The document store owns revision tracking and reactive notifications; patches update store state rather than mutate received FlatBuffers. The browser gains neither server facade access nor implicit decimal arithmetic.

### Runtime type validation

The browser and server must apply equivalent structural rules, but the Rust server remains authoritative.

Keep a shared, declarative constraint specification for such things as:

- Required versus optional fields.
- Nullable versus absent values.
- Numeric ranges and precision.
- String lengths and patterns.
- Maximum list lengths.
- Legal enum values.
- Allowed nested relationships.

Authorization, database integrity and transactional constraints belong on the server.

Do not try to treat a JavaScript runtime descriptor as a new compile-time TypeScript interface. The browser gets runtime-validated types, not automatically generated static types.

## 9. WebSocket protocol

Keep the WebSocket protocol much smaller and more stable than the document schemas.

Use one binary WebSocket message per application frame, with explicit batching for multiple logical operations.

### Recommended envelope

Use a fixed FlatBuffers envelope for control messages, but avoid copying arbitrary document payloads into a second FlatBuffer byte vector merely to wrap them.

For the high-throughput data path, a compact fixed header followed by a complete root FlatBuffer is simpler and avoids that extra payload-copy opportunity.

Application binary message

Kind

1 B

Flags

1 B

Schema

8 B

Revision

8 B

FlatBuffers root buffer

Variable length

Conceptual 18-byte data header, with unsigned 64-bit schema ID and revision encoded little-endian. It can be reduced for message classes that do not need revisions. A full request identifier, document identifier or sequence must be included wherever the operation semantics require it.

The binary payload remains a normal FlatBuffer that can be sliced or viewed without creating a new copy.

Use a stable per-deployment numeric schema ID. Use 64 bits throughout, restricted to 1..=i64::MAX for PostgreSQL `BIGINT`; TypeScript uses `bigint`. Registry IDs must never be reused for different definitions. If multiple independent registries can issue IDs, add a registry namespace or negotiate per-connection IDs.

### Message types

| Message             | Direction        | Purpose                                       |
| ------------------- | ---------------- | --------------------------------------------- |
| `HELLO`             | Both             | Protocol negotiation and reconnect state      |
| `SCHEMA_AVAILABLE`  | Server → browser | Notify client that a new schema exists        |
| `SCHEMA_GET`        | Browser → server | Request missing immutable descriptor          |
| `SCHEMA_DEFINITION` | Server → browser | Deliver `.bfbs` + constraints                 |
| `SCHEMA_READY`      | Browser → server | Confirm validated schema installation         |
| `SERVICE_CALL`      | Client → server  | Invoke named service with binary input and requested output schema |
| `SERVICE_RESULT`    | Server → client  | Return binary output correlated to the request ID |
| `SUBSCRIBE`         | Browser → server | Subscribe to documents or topics              |
| `SNAPSHOT`          | Server → browser | Complete current document                     |
| `PATCH`             | Both             | Revisioned document changes                   |
| `ACK`               | Server → browser | Confirm committed write or accepted operation |
| `ERROR`             | Both             | Typed failure response                        |
| `RESYNC_REQUIRED`   | Server → browser | Recover after missed or incompatible updates  |

Keep the control messages in one statically generated schema, where normal FlatBuffers binding generation is reliable.

### Avoid duplicating application headers

For example, do not add a separate payload length when WebSocket framing already provides message boundaries.

Do not include a 64-bit revision in all message types just because snapshots and patches need one.

Use the smallest header that actually enforces the required semantics.

### HTTP endpoint exposure and format compatibility

Any Moqui application endpoint can expose typed inputs and outputs as FlatBuffers through the shared format adapter. REST, screen transitions, `/actions` and WebSocket messaging are examples, not an exhaustive list. New endpoints inherit this capability rather than implementing a separate binary handler. These adapters invoke the same authoritative Moqui operations, not separate implementations of business logic. Static assets, rendered HTML, file downloads and raw streams retain their native representations; FlatBuffers represents structured operation parameters and results, not an obligatory wrapper around every HTTP byte stream.

Expose immutable input/output descriptors and constraints for each authorized callable contract through the schema retrieval mechanism. Identify contracts by endpoint operation: service name, REST resource/method, screen/transition path, or action identity. Route resolution determines the operation; a supplied schema ID must match its accepted policy and never selects or authorizes another operation. Descriptor retrieval is subject to access policy and must not expose private definitions.

- **REST:** FlatBuffers request bodies bind directly to operation parameters, and responses encode declared results directly. No JSON intermediate is required.
- **Screen transitions:** Generate contracts from screen parameter declarations and the expanded transition XML-action JavaScript, using the same macro metadata and coherent reload rules as services. Preserve transition validation, messages, errors, redirects, session behavior and transaction semantics. Represent non-data outcomes with a stable typed HTTP result contract while retaining existing HTTP status and redirect behavior.
- **`/actions`:** Expose FlatBuffers inputs and outputs for the endpoint's existing supported operations. Derive contracts from their definitions and expanded XML-action JavaScript; retain existing dispatch, ordering, transaction and error semantics. If the endpoint accepts multiple actions, its binary contract must explicitly describe that request/result structure rather than invent new batch semantics.

All application HTTP endpoints use the factory's bounded request handoff and the existing operation-specific worker dispatch before executing framework code. Preserve authentication, artifact authorization, CSRF/session-token checks, parameter precedence and error handling. Binary bodies are not an alternate security boundary.

- Preserve existing REST routes, methods, authentication, authorization, response shapes and supported media types. Continue accepting and returning JSON and other existing formats; do not force binary envelopes or schema handshakes on REST clients.
- Use HTTP `Content-Type` for request decoding and `Accept` for response negotiation, retaining existing defaults. Return `415` for unsupported request media types and `406` when no acceptable response format is available.
- Support FlatBuffers over HTTP for browser, internal and integration callers using `application/vnd.moqui.flatbuffers` and documented input/output schema identifiers in HTTP metadata. Binary clients select this format through `Content-Type` and `Accept`, receiving FlatBuffers rather than JSON. Binary HTTP requests require the same verification and immutable-schema rules as binary WebSocket messages. Unknown or incompatible schemas receive a deterministic typed error; never silently decode with the latest schema or fall back to JSON.
- Bind each format to the same authoritative operation and apply the same semantic validation, authorization, transactional rules, revision checks and idempotency behavior. Binary parameters remain lazy views with overlays where supported, rather than eagerly decoding into a shared map. Preserve existing REST concurrency contracts; map new revision-aware operations explicitly to HTTP preconditions and conflict responses.
- Keep JSON conversion at REST/format boundaries, not in the internal binary fan-out path. JSON parsing and serialization allocate; zero-copy requirements apply to binary structured reads, not every supported API format.
- Define lossless JSON mappings for exact decimals, 64-bit integers, dates, timestamps, binary values, and absent versus null fields for new document APIs, without changing existing REST contracts.
- REST writes and internal message writes must publish through the same committed-update path so subscriptions observe changes regardless of entry transport. REST responses use HTTP status codes and the API's error representation, not WebSocket control frames.

## 10. Snapshots versus patches

For complicated nested documents, this is as important as choosing FlatBuffers.

### Snapshot

A snapshot is a complete document encoded under one exact immutable `.bfbs` schema version.

Send snapshots for initial subscriptions, reconnect recovery and major structural changes.

### Patch

A patch describes an operation against a known revision.

For example:

```
Document ID: Order-123
Base revision: 101
New revision: 102

Operations:
  SET field 7 = "Approved"
  SET items[2].quantity = 5
  REMOVE items[3]
```

The patch format should be a separate stable schema.

Use numeric field IDs and an explicit path representation rather than sending arbitrary string paths on every update.

Define an immutable patch model supporting:

- Set value.
- Clear optional value.
- Replace subtree.
- Insert/remove list element.
- Replace list.
- Delete document.

For lists whose items represent entities, prefer stable item IDs over positional indices. Otherwise, concurrent inserts and removes can make patches target the wrong element.

Critical design choice: A snapshot has a specialized FlatBuffers layout; a patch may need a generic typed-value representation because it targets arbitrary fields.

That is acceptable. Small generic patches are often much cheaper than rebuilding and transmitting a large specialized snapshot.

### Write conflicts and idempotency

Every modifying operation should include:

```
client_request_id
document_id
base_revision
schema_id
patch
```

On the server:

1. Verify the binary structure and schema.
2. Authorize the requested mutation.
3. Check the base revision and any idempotency key.
4. Apply the change transactionally through the authoritative document service.
5. Publish a new committed revision.
6. Acknowledge using the original request ID.

On reconnect, the client sends the last committed document revision it processed. The server resumes from retained history if available, otherwise sends a snapshot.

Do not treat WebSocket delivery as exactly-once database execution.

## 11. Rust fan-out architecture

For 10 million clients, the networking architecture needs to minimize per-connection allocations.

### Use a sharded ownership model

Each gateway manages a subset of connections.

Each connection has one owning task or event-loop actor that exclusively handles its socket writes. Other tasks send references to immutable messages to that owner.

Avoid multiple tasks writing directly to the same WebSocket.

One validated and encoded update

Shared immutable `Bytes`

One payload allocation for many recipients

Subscribers A

Shared byte references

Subscribers B

Shared byte references

Subscribers C

Shared byte references

Per-connection WebSocket/TLS writes

Still requires independent transport processing

Use `bytes::Bytes` for shareable immutable payloads.

Be aware that merely calling `.clone()` on `Bytes` avoids cloning the payload data but still increments reference counts and creates per-recipient metadata.

For enormous fan-out, batching references by gateway shard can reduce contention.

### Avoid per-connection message channels when idle

A conventional Tokio task with its own large channel and read/write buffers may be reasonable at 10,000 connections but expensive at 10 million.

Start with Axum and Tokio to establish correctness and baseline performance.

If profiling shows task/channel memory is excessive, replace the internal gateway transport implementation with a more compact readiness/event-loop design. Keep the schema and document layers unchanged.

### Subscriptions

Store subscriptions in two directions:

```
topic -> sharded connection IDs
connection -> compact topic IDs
```

The first supports fan-out; the second supports efficient disconnection cleanup.

Use numeric subscription IDs and compressed/segmented membership representations where they outperform hash sets.

Do not scan 10 million connections for every update.

### Backpressure

Apply hard limits to:

- Queued bytes per connection.
- Number of pending updates.
- Maximum message size.
- Maximum subscriptions.
- Maximum in-flight writes.
- Maximum age of undelivered state.

When a connection is slow, coalesce replaceable state updates. If a lossless event stream is required, use a durable cursor and force resynchronization rather than silently dropping events.

Do not let slow readers retain gigabytes of old shared buffers.

## 12. Realistic capacity model

The dominant scaling variable is not simply messages received per second.

It is:

\\[ \text{Outbound deliveries/sec} = \text{updates/sec} \times \text{average recipients/update} \\]

Assuming 100,000 updates/sec and an average 512-byte application payload:

| Average recipients per update | Outbound deliveries/s | Payload egress |
| ----------------------------- | --------------------- | -------------- |
| 1                             | 100K                  | 0.41 Gbps      |
| 10                            | 1M                    | 4.10 Gbps      |
| 100                           | 10M                   | 40.96 Gbps     |
| 1,000                         | 100M                  | 409.6 Gbps     |
| 10,000                        | 1B                    | 4.10 Tbps      |
| 1,000,000                     | 100B                  | 409.6 Tbps     |

These are payload-only figures; actual egress includes WebSocket/TCP/IP/TLS overhead, retransmission and protocol metadata.

### The 10-million-recipient extreme

If every one of the 100,000 updates/sec were delivered to 10 million clients, that would be one trillion deliveries per second.

At 512 bytes per delivery, payload traffic alone would be approximately 4.096 petabits per second.

That is not a practical direct fan-out design for a conventional application deployment.

For large subscriber groups, you must coalesce state and deliver periodic snapshots or batches rather than mechanically forwarding every intermediate mutation.

### Baseline gateway capacity

I would begin capacity tests with the following planning scenarios, not promises:

| Cluster scenario | Connections/node | Node count |
| ---------------- | ---------------- | ---------- |
| Conservative     | 25,000           | 400        |
| Intermediate     | 50,000           | 200        |
| Aggressive       | 100,000          | 100        |

The appropriate density depends on TLS, kernel memory, activity patterns and network load. These are starting points for experiments, not empirically proven capacities.

For example, 100,000 connections at 16 KiB of total attributable memory each use approximately 1.53 GiB before additional application overhead.

Actual Linux socket, TLS and process memory must be measured under realistic traffic and slow-client conditions.

## 13. Performance targets and acceptance gates

These should be treated as initial engineering targets, not FlatBuffers guarantees. Record observed baselines and revise them with production-like document distributions.

### A. Serialization and reflection

Run these benchmarks on a specified, pinned server CPU and recent Chromium browser. Use documents of 1 KiB, 10 KiB and 100 KiB, including deeply nested types and mixed scalar/vector fields.

| Metric                                      | Initial target             | What it measures                          |
| ------------------------------------------- | -------------------------- | ----------------------------------------- |
| Server reflective read of one cached scalar | p99 under 1 µs             | Access after verification                 |
| Browser reflective scalar access            | p99 under 2 µs             | Access after schema lookup and validation |
| Rust 1 KiB encode                           | p99 under 25 µs            | Runtime-schema-driven builder             |
| Rust 1 KiB verification                     | p99 under 25 µs            | Full structural verification              |
| Browser 1 KiB dynamic encode                | p99 under 50 µs            | Schema-driven builder                     |
| Full-document materialization on receive    | 0 by default               | Must remain a lazy view                   |
| Inbound validation                          | 100% of untrusted messages | No unchecked access path                  |
| Per-recipient payload re-encoding           | 0 for identical payloads   | Shared fan-out                            |

These targets are intentionally ambitious, and the reflection/verification paths may require optimization to achieve them.

A more useful result than a single microbenchmark is the performance multiplier against JSON for the same workload, including actual field access and validation.

I would require the chosen FlatBuffers implementation to beat a well-optimized JSON baseline in either end-to-end CPU usage or allocation rate on representative traffic. If it does not, investigate before investing further in custom reflection code.

### B. Server ingest

| Metric                               | Target                                           |
| ------------------------------------ | ------------------------------------------------ |
| Aggregate inbound traffic            | 100,000 document operations/sec                  |
| Binary validation errors             | Handled without crashes or process termination   |
| Ingest p99 latency                   | Under 10 ms through routing and validation       |
| End-to-end committed write p99       | Set separately after benchmarking Moqui/database |
| Queue growth under steady-state load | None                                             |
| Duplicate request behavior           | Idempotent where operations require it           |
| Invalid schema writes                | Deterministic rejection                          |

The 10 ms target excludes database commit time. You cannot responsibly assign a database p99 target before profiling actual Moqui transactions, indexing and business rules. Ingest is not the 100k commits/reads target in `plans/Performance.md`.

### C. Fan-out

| Metric                                                | Initial target                                           |
| ----------------------------------------------------- | -------------------------------------------------------- |
| Encode operations per logical update                  | 1 per required payload variant                           |
| Per-recipient full payload copies in application code | 0                                                        |
| Per-connection unbounded queues                       | 0                                                        |
| Gateway memory budget                                 | Configured and measured                                  |
| Normal internal dispatch p99                          | Under 5 ms at tested load                                |
| Subscription routing cost                             | Proportional to relevant recipients, not all connections |
| Slow-client handling                                  | Bounded queue, coalescing or resync                      |
| Recovery after gateway restart                        | Reconnect and state resynchronization                    |

No single per-node outbound-message target should be committed until fan-out size and message-size distributions are measured.

### D. Schema synchronization

| Metric                                         | Initial target                           |
| ---------------------------------------------- | ---------------------------------------- |
| New schema publication                         | p99 under 2 seconds for ordinary schemas |
| Cache installation after registry notification | p99 under 1 second per healthy gateway   |
| Descriptor bytes sent per normal data message  | 0                                        |
| Rebuilding client application for new type     | Never                                    |
| Restarting Rust gateways for new type          | Never                                    |
| Schema fingerprint mismatch accepted           | Never                                    |
| Data processed against unknown schema          | Never                                    |
| Incompatible write silently accepted           | Never                                    |

These are healthy-system targets and exclude offline clients. For 10 million connections, total rollout time is determined by subscriptions, network congestion and connected-client responsiveness.

### E. Memory

Set measured process budgets for:

- Connection metadata.
- Tokio tasks or gateway actors.
- WebSocket parsing state.
- TLS session state.
- Kernel TCP buffers.
- Subscription indexes.
- Pending writes and queued updates.
- Shared FlatBuffers.
- Schema caches.

I would target no significant steady-state heap growth during a multi-hour 100K/s load test.

An idle connection memory target of 16–32 KiB, including amortized server and kernel costs, is worth testing initially. If the stack cannot reach that range, do not assume custom serialization will fix it.

## 14. Benchmark workloads

Don't test only a single flat `Order` object.

Use at least these five representative document cases:

| Fixture         | Characteristics                                   | Main risk                    |
| --------------- | ------------------------------------------------- | ---------------------------- |
| Small update    | 3–10 changed scalar fields                        | Per-message overhead         |
| Nested order    | 20–100 fields, 10–50 line items                   | Pointer traversal            |
| Large document  | 100 KiB+, multiple nested collections             | Size, allocation, throughput |
| Sparse document | Hundreds of possible fields, few present          | Schema and default handling  |
| Dynamic schema  | New fields and nested types during active traffic | Version synchronization      |

For every fixture, benchmark:

1. Rust construction.
2. Rust verification.
3. Rust reflective field access.
4. Browser receipt and validation.
5. Browser reflective field access.
6. Browser dynamic construction.
7. Round-trip equality and compatibility.
8. Memory allocated per stage.
9. Delivery across many subscribers.

Benchmarks must include both successful and malformed payloads. Browser measurements include JavaScript-to-worker transfer, copying into/out of WASM memory, verification, binding calls, batched field reads and UI projection—not only Rust field access. Test memory growth, handle cleanup, async cancellation and OPFS quota/recovery failures.

Add binary and JSON compatibility tests for REST, screen transitions and `/actions`, including descriptor retrieval, content negotiation, input/output schema mismatches, authentication/authorization, CSRF, parameter precedence, errors, redirects and revision conflicts. Verify that binary calls reach the same cached JavaScript and authoritative operations without a JSON intermediate. Cross-transport tests must verify that REST writes reach subscribers and that REST reads observe committed internal-message writes. Test lossless JSON/binary round trips for the new document APIs.

Also test a heavy-tailed message-size distribution rather than assuming every message is exactly 1 KiB.

## 15. Security and correctness requirements

These are release blockers, not optional later work.

### Schema publication

Only trusted operators or authorized Moqui services may publish new definitions.

Run `flatc` out of the data-plane process in a restricted worker with filesystem, CPU, memory, execution-time and input-size limits.

Require the compiler and runtimes to recognize every `.bfbs` advanced feature flag they encounter. The FlatBuffers intermediate representation explicitly requires unknown advanced features to be rejected.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://flatbuffers.dev\&sz=32)

FlatBuffers Docs



### Message verification

Enforce:

- Maximum message size.
- Maximum nesting depth.
- Maximum table count.
- Maximum vector length.
- Maximum string length.
- Valid offsets and alignment.
- UTF-8 requirements.
- Exact semantic field types.
- Supported schema identifier and fingerprint.
- Schema-dependent required fields.
- Resource budgets for recursive validation.

Avoid `unsafe` reflective reads on untrusted bytes until verification has completed successfully.

### Browser/client trust

A client claiming `SCHEMA_READY` is not evidence that it can be trusted.

The server still validates all writes, including schema versions, authorization and document revisions.

### Fuzz testing

Use `cargo-fuzz` or equivalent fuzzing against the Rust reflective decoder and verifier.

Fuzz malformed buffers, malformed `.bfbs` files, deeply recursive descriptors, vector lengths, invalid unions and schema/version mismatches.

Native/WASM property tests should ensure that every valid document encoded in native Rust can be read by the browser WASM runtime and vice versa, including JavaScript binding round trips.

## 16. Explicit alternatives and decision rationale

| Architecture                                      | Why it might win                                           | Why I'm not choosing it                                         |
| ------------------------------------------------- | ---------------------------------------------------------- | --------------------------------------------------------------- |
| FlatBuffers `.bfbs` reflection                    | Specialized zero-copy layouts and runtime-defined schemas  | Chosen, despite implementation work                             |
| FlatBuffers generic typed values                  | Simpler runtime evolution                                  | Additional type tags, generic lookup and larger messages        |
| FlexBuffers + schema registry                     | Simplest schema-less direct-access solution                | Does not specialize the binary layout for each document         |
| Cap’n Proto dynamic schemas                       | Excellent zero-copy format and reflection concepts         | More complex Rust/browser runtime integration                   |
| Protobuf dynamic descriptors                      | Very mature dynamic-schema tooling                         | Ordinary decoding typically constructs values                   |
| Apache Avro                                       | Schema evolution is a core design feature                  | Not a general zero-copy object-traversal format                 |
| Apache Arrow IPC                                  | Excellent columnar and batched zero-copy processing        | Poor fit for large numbers of small document mutations          |
| Custom binary protocol                            | Maximum control and possibly less wire overhead            | Too much custom implementation for nested runtime-defined types |
| Rust/Emscripten WASM reflection shared by server and browser | One shared verifier, reader, writer and constraint implementation | Chosen browser runtime; measure memory copies and host-call overhead |

### Why FlatBuffers wins here

Its primary advantage is the combination of specialized per-document binary layouts and runtime-readable schema descriptors.

FlexBuffers has the flexibility but not the specialized layout.

Protobuf and Avro have runtime schema facilities but normally require more deserialization work.

Cap’n Proto is technically appealing, but the end-to-end Rust/browser implementation is not as straightforward.

FlatBuffers is therefore a defensible architecture—not necessarily the shortest implementation, but a good match for your stated absolute requirements.

The key risk remains the completeness and performance of the reflection stack, especially dynamic writing and browser verification. Resolve that before committing to the full deployment.

## 17. Implementation roadmap

I would organize the work around demonstrable milestones rather than integrating everything at once.

Implementation checklist

Estimated single-developer engineering effort; external integration and testing can extend it.

0/6 phases

Phase 0 — Feasibility spike1–2 weeks

- Pin compatible FlatBuffers, Rust, Emscripten and TypeScript versions
- Compile a nested Moqui schema to .bfbs at runtime
- Verify and read a document reflectively in native Rust
- Compile the shared reader/writer to `wasm32-unknown-emscripten` and read the same buffer through generated JavaScript bindings in a browser worker
- Create compatible documents from native Rust and browser WASM
- Measure transfer/copy costs, host-call overhead, memory growth and retained-handle cleanup
- Prove async adapter suspension/cancellation and OPFS persistence/recovery; test non-pthread browser support
- Benchmark the safe verification and dynamic builder paths
- Generate input/output schemas for one real service, bind verified inputs lazily in QuickJS, execute its precached JavaScript on a service worker, and encode its result natively
- Test defaults, context assignments, nested read-only parameters, nested calls, cleanup and retained-view lifetime
- Change an XML-action JavaScript macro and verify regeneration of affected schemas and cached service functions as one coherent generation; test incompatible changes, failed reloads and in-flight calls retaining their original generation
- Compare verified binary-input → cached JS → binary-output against map-backed invocation with equally cached JS, including a large input whose service reads only a few fields

Exit gate: New runtime-created nested type is encoded and read in native Rust and browser WASM without recompilation, and a real service executes from binary-backed parameters without eager parameter-graph materialization or per-call JS compilation. Measure verification, host access, materialization fallbacks and output encoding; do not credit existing script caching or shared fan-out to FlatBuffers.

Phase 1 — Schema compiler and registry2–3 weeks

Phase 2 — Production reflection runtime3–5 weeks

Phase 3 — Document synchronization and REST adapters2–3 weeks

- Implement shared binary input/output exposure for any application endpoint, including authorized descriptor retrieval; exercise REST, screen transitions, `/actions` and a newly registered endpoint
- Share authoritative operations and lazy binary parameter bindings between HTTP and internal messaging
- Preserve existing JSON and other format contracts
- Verify content negotiation and cross-transport consistency
- Prove browser binary service call → bounded worker handoff → cached QuickJS → database commit → binary result and committed subscriber update
- Verify exact decimals, timestamps, 64-bit integers, absent/null and defaults across service bindings; reject overload without unbounded queues
- Keep an existing `qvt` screen working unchanged through its established projection

Phase 4 — Gateway and fan-out3–5 weeks

Phase 5 — Scale and production hardening3–6 weeks

&#x20;Copy implementation checklist

The total is roughly 14–24 engineering weeks for a prototype through production hardening, assuming one experienced engineer, existing Moqui integration access and no major surprises. A smaller team can parallelize later work, but the feasibility spike and shared format semantics are sequential dependencies.

A genuinely production-ready reflective protocol is not realistically a few hundred lines of handwritten code. The goal should instead be to keep all custom complexity inside two reusable components: a schema-driven reader and a schema-driven writer.

## 18. What not to do

| Avoid                                         | Because                                           | Do instead                         |
| --------------------------------------------- | ------------------------------------------------- | ---------------------------------- |
| Generate and hot-load new Rust binaries       | Complex deployment, unsafe operational boundary   | Load immutable `.bfbs` descriptors |
| Generate new browser JS per schema            | Code execution and cache complexity               | Use runtime reflection             |
| Embed schemas in every message                | Repeated bandwidth and allocations                | Send schema IDs                    |
| Convert internal binary fan-out to JSON        | Destroys zero-copy benefits                       | Convert only at format boundaries  |
| Decode entire document graphs eagerly         | Extra allocation and CPU                          | Lazy nested field access           |
| Bypass verification                           | Malformed offsets can become safety hazards       | Verify before access               |
| Rebuild message for every subscriber          | Scales encoder CPU with fan-out                   | Share immutable encoded payloads   |
| Reuse field IDs after deletion                | Old messages become ambiguous                     | Reserve old IDs permanently        |
| Treat old schema writes as automatically safe | Can overwrite newer state                         | Revision checks and migrations     |
| Broadcast every schema to 10M browsers        | Unnecessary network traffic                       | Target affected subscribers        |
| Allow unlimited outbound queues               | Slow clients can exhaust memory                   | Bounded queues and resync          |
| Start by optimizing the serializer alone      | Networking, fan-out and Moqui writes may dominate | Benchmark end-to-end               |

## 19. Critical go/no-go decisions

Before building the full registry and gateways, I would insist on four proof points.

Proof 1: A new type appears at runtime

A Rust process and an already-running browser successfully read a newly introduced nested document using a freshly published `.bfbs`, without recompilation.

Proof 2: Both sides can write

Native Rust and browser Emscripten WASM independently encode a complex nested document that the other side can verify and read using the identical schema, through the generated JavaScript bindings.

Proof 3: Schema versions coexist

A v12 browser can continue its session while v13 is published. Compatible changes work; incompatible changes are rejected or migrated deliberately.

Proof 4: Real throughput advantage

FlatBuffers beats your JSON or conventional binary baseline on the metrics that matter: allocation, CPU, browser access latency, and per-update fan-out cost.

If Proof 2 requires an excessive amount of custom writer implementation, reconsider the design before continuing. A stable generic FlatBuffers schema with external runtime descriptors is the fallback; it preserves strict typing and direct buffer reads while sacrificing specialized per-type layouts.

## 20. Final architectural decision

I would approve this architecture conditionally:

Selected production architecture

## FlatBuffers + `.bfbs` + Rust + Emscripten WASM + TypeScript UI

Schema control plane

Moqui metadata → Schema publisher → Immutable `.bfbs` registry → Gateway/browser caches

Document data plane

Internal clients/browser runtime ↔ Binary WebSocket or opt-in binary HTTP ↔ Rust host gateway ↔ Bounded service workers ↔ Synchronous Moqui services

Service execution

Immutable service input schema + verified buffer → Lazy QuickJS parameter bindings → Precached JavaScript → Native FlatBuffers result encoding

REST API plane

Browser/integration clients ↔ HTTP (FlatBuffers, JSON and other supported formats) ↔ REST / screen-transition / `/actions` adapters ↔ Same authoritative Moqui operations

Performance strategy

Reflective, verified zero-copy reads · Cached descriptors · Revisioned patches · Shared immutable fan-out buffers

The most important decision is to prove the shared dynamic writer and safe reflective reader in native Rust and Emscripten WASM early. FlatBuffers provides the binary foundations, not a complete browser host adapter; qualify generated bindings, worker ownership, async suspension and OPFS separately.

Once those are demonstrated, the remaining work becomes conventional systems engineering: schema publication, validation, synchronization, routing, backpressure and performance testing.

### Primary technical references

- [FlatBuffers binary schema and reflection representation](https://flatbuffers.dev/intermediate_representation/)
- [FlatBuffers Rust reflection documentation](https://flatbuffers.dev/languages/rust/)
- [Rust reflection APIs](https://docs.rs/flatbuffers-reflection/latest/flatbuffers_reflection/)
- [FlatBuffers schema evolution and compatibility](https://flatbuffers.dev/evolution/)
- [Official reflection meta-schema](https://github.com/google/flatbuffers/blob/master/reflection/reflection.fbs)
- [WebSocket protocol specification](https://www.rfc-editor.org/rfc/rfc6455)

Implementation priority: Get one complicated, dynamically defined Moqui document working bidirectionally under runtime `.bfbs` reflection before building any of the 10-million-connection infrastructure. That is the highest-risk technical dependency, and validating it first prevents spending weeks optimizing a gateway around an unproven document format.