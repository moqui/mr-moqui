# docs

Reference guides for built features. The two guides most people start with are at the top; the rest are by tier and
area. Designs, status and reviews are in [`plans/`](../plans/README.md). The security policy is
[SECURITY.md](../SECURITY.md) at the repository root.

| Guide | What it covers |
|---|---|
| [Build-Test.md](Build-Test.md) | Building from source, the build packages and their features, the `./moq` tasks, the Rust tests and the component specs of `moqui test` |
| [Run-Deploy.md](Run-Deploy.md) | Running and deploying: the runtime and conf, data and databases, HTTPS, one server, containers, clusters, PASS servers with a BOSS server, and dev instances with a group server |

## data: Data and Resources

Entities, data files, search, resources and content.

| Guide | What it covers |
|---|---|
| [Revision-Management.md](data/Revision-Management.md) | Versions of `dbresource://` files and `git://` locations behind the resource API |
| [Barcodes.md](data/Barcodes.md) | `ec.resource` barcode SVGs and the GS1 check digit and element string helpers |

## logic: Logic and Services

Services, scripts, ECA rules and jobs.

| Guide | What it covers |
|---|---|
| [Services.md](logic/Services.md) | Service calls from scripts: errors, transactions, and calls that may fail without failing the caller |

## ui: User Interface

Screens, forms, templates and the clients.

| Guide | What it covers |
|---|---|
| [Qvt-Client.md](ui/Qvt-Client.md) | The client of the `qvt` render mode and the `/qapps` shell: page shell, screen loading, the `m-*` widgets |
| [TUI.md](ui/TUI.md) | The terminal UI: `moqui tui`, the `tui` render mode, the apps over SSH, and agents driving it |

## interfaces: System Interfaces

What other systems, devices and clients reach, and what Moqui calls.

| Guide | What it covers |
|---|---|
| [Web-Scripts.md](interfaces/Web-Scripts.md) | Servlets, filters, listeners and lifecycle actions in JavaScript, and the outbound REST client |
| [Git-Hosting.md](interfaces/Git-Hosting.md) | Hosted git repositories over HTTPS and SSH, merge requests, gitECA rules, LFS |
| [Sync.md](interfaces/Sync.md) | CalDAV, CardDAV, WebDAV and SFTP for phones and desktop apps, with app passwords |
| [Mail-Inbound.md](interfaces/Mail-Inbound.md) | Hosted and shared mail accounts through the SMTP listener behind an edge gateway |
| [Mail-OAuth.md](interfaces/Mail-OAuth.md) | Mail accounts that sign in to Google and Microsoft with OAuth |

## ai: Agents and Models

| Guide | What it covers |
|---|---|
| [Assist.md](ai/Assist.md) | The Assist drawer and its server: the agent runtime, tools, approvals, skills and canvases |
| [A2A.md](ai/A2A.md) | The agent-to-agent server and client (A2A 1.0) |
| [MCP.md](ai/MCP.md) | The MCP tool server for AI agents and coding assistants, and the MCP client |
| [Gateway.md](ai/Gateway.md) | The OpenAI-compatible LLM gateway at `/llm/v1`, and the LLM embedder |

## tools: The Tools and System Apps

| Guide | What it covers |
|---|---|
| [Monitoring.md](tools/Monitoring.md) | Telemetry, OTLP in and out, the Monitor and Dashboards apps, and reports |

## security

| Guide | What it covers |
|---|---|
| [Attack-Surface.md](security/Attack-Surface.md) | The HTTP routes, screens, REST, WebSockets and other listeners, the controls, and the lock-down options |
| [Proof-Tests.md](security/Proof-Tests.md) | The named HTTP security proofs and how to run them with `moqui test-client` |

## dev: Developing on MR Moqui

| Guide | What it covers |
|---|---|
| [Practices-App-Dev.md](dev/Practices-App-Dev.md) | Practices for building applications: entities, services, screens, `.qvue` files and specs |
| [Practices-Rust.md](dev/Practices-Rust.md) | Practices for Rust: crates, organization, tests and idiom |
| [Native-Components.md](dev/Native-Components.md) | A component with Rust code: linked services, tools, resource schemes and template renderers |
| [Client-Testing.md](dev/Client-Testing.md) | Testing a running server from outside with `moqui test-client`: HTTP, WebSocket, Chromium and load |
| [Moqui-Java-Migration.md](dev/Moqui-Java-Migration.md) | Moving a Moqui Java deployment or component to MR Moqui |
