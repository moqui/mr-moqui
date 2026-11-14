# docs

Reference guides for built features. The two guides most people start with are at the top; the rest are by tier and
area. Designs, status and reviews are in [`plans/`](../plans/README.md). The security policy is
[SECURITY.md](../SECURITY.md) at the repository root.

| Guide | What it covers |
|---|---|
| [Build-Test.md](Build-Test.md) | Building from source, native code and C libraries, Windows, the build packages and their features, the `./moq` tasks, the Rust tests and the component specs of `moqui test` |
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
| [Screen-Output.md](ui/Screen-Output.md) | A screen's response kept whole or sent as it renders: the `stream-threshold`, what streams, headers and failures after the head, list rows read from a cursor as they are written, templates that write as they render |
| [Qvt-Client.md](ui/Qvt-Client.md) | The client of the `qvt` render mode and the `/qapps` shell: page shell, screen loading, the `m-*` widgets |
| [Semantic-Find.md](ui/Semantic-Find.md) | Ctrl+F by meaning: `MoquiFind.js`, `/find/rank`, `ec.search.rankTexts`, and the embedding model it needs |
| [Desktop-Client.md](ui/Desktop-Client.md) | The desktop app as a client of a server: accounts, device modes and `<client-conf>`, the offline store, the outbox, the bundled Comms screens, crypto and calls |
| [Mobile-Client.md](ui/Mobile-Client.md) | The Android app as a client of a server: what differs from the desktop client |
| [TUI.md](ui/TUI.md) | The terminal UI: `moqui tui`, the `tui` render mode, the apps over SSH, and agents driving it |
| [Terminal.md](ui/Terminal.md) | The browser terminal: sessions in tabs and splits that outlive the page, the `moqui>` command line and shells on the server on a dev instance, floating terminals, attention from agents, the apps and the edge node |
| [Db-Forms.md](ui/Db-Forms.md) | Forms kept in the database: dated fields and options read as of a date, drafts, successor fields, and dependencies as `visible-when` and `require-when` |
| [Themes.md](ui/Themes.md) | Merchant themes on the locked renderer: the package, `ec.resource.theme` validate and render, typed settings, the markup, URL and CSS checks, and the budgets |

## interfaces: System Interfaces

What other systems, devices and clients reach, and what Moqui calls.

| Guide | What it covers |
|---|---|
| [Web-Scripts.md](interfaces/Web-Scripts.md) | Servlets, filters, listeners and lifecycle actions in JavaScript, and the outbound REST client |
| [Git-Hosting.md](interfaces/Git-Hosting.md) | Hosted git repositories over HTTPS and SSH, merge requests, gitECA rules, LFS |
| [Sync.md](interfaces/Sync.md) | CalDAV, CardDAV, WebDAV and SFTP for phones and desktop apps, with app passwords |
| [Mail-Inbound.md](interfaces/Mail-Inbound.md) | Hosted and shared mail accounts through the SMTP listener behind an edge gateway |
| [Call-Recording.md](interfaces/Call-Recording.md) | Recording a call: turning it on, the notice to every leg, the files and who may play them |
| [Mail-OAuth.md](interfaces/Mail-OAuth.md) | Mail accounts that sign in to Google and Microsoft with OAuth |
| [WebRTC.md](interfaces/WebRTC.md) | Calls: the media port and the SFU, signaling on the notification socket, guests by link, the cluster |
| [TURN.md](interfaces/TURN.md) | ICE-TCP and the embedded TURN server for browsers behind NAT and firewalls; coturn instead |
| [SIP.md](interfaces/SIP.md) | Phone calls through SIP trunks: the settings, provider profiles (Telnyx), fraud controls, emergency readiness |
| [Chat-Keys.md](interfaces/Chat-Keys.md) | Browsers' crypto devices for end-to-end encryption: devices, keys and Olm to-device messages with Matrix's payloads, who reaches whom, revocation, the browser API |
| [Voicemail.md](interfaces/Voicemail.md) | Voicemail on the SIP engine: when a call goes there, the greeting and recording, phone access with a PIN and its limits, notices, transcription, the prompts an admin supplies |
| [Chat-Bridges.md](interfaces/Chat-Bridges.md) | Chat actors and the bridge layer: the adapter contract, input stored before it is acknowledged, durable delivery with leases and idempotency keys, the C1 migration |
| [SMS.md](interfaces/SMS.md) | SMS and MMS on Telnyx: setup and 10DLC, the number pair's room shared with calls and voicemail, signed webhooks, the checks before a send, opt-out, the system's texts |
| [XMPP.md](interfaces/XMPP.md) | XMPP apps on Moqui chat: the listeners and WebSocket, sign-in with app passwords (SCRAM-PLUS, SASL2, FAST), rooms as MUC, history, PEP and OMEMO, uploads, what is served |

## ai: Agents and Models

| Guide | What it covers |
|---|---|
| [Assist.md](ai/Assist.md) | The Assist drawer and its server: the agent runtime, tools, approvals, skills and canvases |
| [Constitution.md](ai/Constitution.md) | The Constitution: shared guidance for people and agent runs, its sources, editing, prompt section and references |
| [A2A.md](ai/A2A.md) | The agent-to-agent server and client (A2A 1.0) |
| [MCP.md](ai/MCP.md) | The MCP tool server for AI agents and coding assistants, and the MCP client |
| [Gateway.md](ai/Gateway.md) | The OpenAI-compatible LLM gateway at `/llm/v1`, and the LLM embedder |

## tools: The Framework Apps

| Guide | What it covers |
|---|---|
| [Comms.md](tools/Comms.md) | BasicComms: the Inbox, Compose, chat and the mail settings, and how an app adds to them |
| [Component-Deploy.md](tools/Component-Deploy.md) | Viewing the merged conf with each value's file and editing a conf file (System › Conf), reloading the conf on a running server (SIGHUP, the dev watcher), what changes live and what waits for a restart, and loading or deploying a component from git without a restart (System › Components) |
| [Database.md](tools/Database.md) | The Tools app's Data, SQL, Schema, ETL and Database sections: the data grid, SQL runs, schema plans and jobs, ETL mappings, external databases, the conf settings and the permissions |
| [Load-Runner.md](tools/Load-Runner.md) | Load scripts of virtual users over HTTP or this server's services: writing one, running it from Tools › Load or `moqui test-load`, reading and comparing results, and what a run may reach |
| [Monitoring.md](tools/Monitoring.md) | Telemetry, OTLP in and out, Monitor (a section of System) and the Dashboards app, and reports |

## security

| Guide | What it covers |
|---|---|
| [Attack-Surface.md](security/Attack-Surface.md) | The HTTP routes, screens, REST, WebSockets and other listeners, the controls, and the lock-down options |
| [Recent-Auth.md](security/Recent-Auth.md) | Actions that need a sign-in a moment ago: what counts as one, and the check a sensitive action uses |
| [Proof-Tests.md](security/Proof-Tests.md) | The named HTTP security proofs and how to run them with `moqui test-client` |

## dev: Developing on MR Moqui

| Guide | What it covers |
|---|---|
| [Practices-App-Dev.md](dev/Practices-App-Dev.md) | Practices for building applications: entities, services, screens, `.qvue` files and specs |
| [UI-Review.md](dev/UI-Review.md) | Activity-based UI review: isolated environments, visual and interaction criteria, evidence, refinement and verification |
| [Practices-Rust.md](dev/Practices-Rust.md) | Practices for Rust: crates, organization, tests and idiom |
| [Autodoc.md](dev/Autodoc.md) | Markdown docs of the artifacts with Mermaid diagrams: the Dev AIDE's Docs and Export docs, `moqui autodoc`, the templates |
| [Native-Components.md](dev/Native-Components.md) | A component with Rust code: linked services, tools, resource schemes and template renderers |
| [JS-Core-API.md](dev/JS-Core-API.md) | The server `ec.*` JavaScript API: every facade and the objects it returns, the differences from Moqui Java, what is not available |
| [JS-Globals.md](dev/JS-Globals.md) | What a script has besides `ec`: bare names, decimals and dates, `compare`, the helper objects (`StringUtilities` and the rest), `MNode`, `EdiHandler`, `SftpClient` |
| [JS-Service-Scripts.md](dev/JS-Service-Scripts.md) | JavaScript in a service: implementation types, what the body sees, how results are returned, script modules, errors, transactions, parameter types |
| [JS-XML-Actions.md](dev/JS-XML-Actions.md) | XML actions, expressions and conditions as JavaScript: each action element, names, truth and comparison, Groovy that does not carry over, rule actions |
| [JS-Screen-Scripts.md](dev/JS-Screen-Scripts.md) | The JavaScript of a screen: actions, transitions and widget expressions on the server, server-static screens, the browser `ec` |
| [JS-Framework-Services.md](dev/JS-Framework-Services.md) | The services the framework ships, by group, with their main parameters; the implicit entity-auto services; where the complete list is |
| [JS-Fancy-Templates.md](dev/JS-Fancy-Templates.md) | The Fancy template dialect: syntax, escaping, trusted and locked templates, renderers by extension, porting from FreeMarker |
| [JS-Templates-Macros.md](dev/JS-Templates-Macros.md) | How screens render: the render modes and their macro modules, the macro model, the `sri` object, writing a macro or a render mode |
| [JS-Testkit.md](dev/JS-Testkit.md) | The server spec API of `moqui test`: `spec`, `feature`, conditions, `assert`, the helpers, the capture tools, agent specs, porting a Spock spec |
| [JS-Test-Client.md](dev/JS-Test-Client.md) | Testing a running server from outside with `moqui test-client`: HTTP, WebSocket, Chromium and load |
| [JS-Test-Load.md](dev/JS-Test-Load.md) | The load script API of `moqui test-load` and Tools › Load: `plan`, `scenario`, steps, rates and stages |
| [JS-Types-Tooling.md](dev/JS-Types-Tooling.md) | `moqui types` and the generated declarations, `moqui check`, `moqui lint`, `moqui lsp`, service coverage, converting Groovy |
| [Moqui-Java-Migration.md](dev/Moqui-Java-Migration.md) | Moving a Moqui Java deployment or component to MR Moqui |
| [Contributions.md](dev/Contributions.md) | Pull requests and AUTHORS.md: checking a signature, merging, and an unsigned pull request's ideas without its code |
