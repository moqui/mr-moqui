# Modern Rust Moqui Framework - MR Moqui

## Quick Start

```sh
./moq build   # The Dev package (docs/Build-Test.md); --lite without git, SSH and dev tools; --release for every feature
./moq load    # Load all data, including demo; the first run on an empty database loads it too
./moq run     # Start moqui; logs to console, ctrl-c to stop; default ports 8080 and 8443
```

It needs Rust 1.95 or newer and bash, on Linux or macOS ([Build](docs/Build-Test.md#build)). This builds the Dev binary (target/develop/moqui: the web server on embedded Postgres, PDF, SMTP, git hosting, SSH and the dev tools) and uses it to load data and run. Add `--release` to each of the 3 commands to build and use the release binary with the full toolset included: SSH/SFTP, OAuth2/OIDC, SMTP/IMAP/POP, Git, XLSX, and image processing. To deploy to a server, a container or a cluster, see [Run and Deploy](docs/Run-Deploy.md).

## Status

The framework is built and the applications below run on it. Work continues on Moqui Java functionality not yet
covered, features new to MR Moqui, tests and fixes, and performance. Mantle is converted from Groovy to JavaScript
(`mr-mantle`); Simple Screens and Marble ERP are being converted (`mr-marble`); IronStone BOSS (`mr-boss`), starting
with the Commons & Comms groupware and communication apps, and IronStoa PASS (`mr-pass`) are being designed and built.
What is open is in [plans/Remaining-Work.md](plans/Remaining-Work.md).

The `moq` script replaces our old Gradle tasks, and getting components is similar:

```sh
./moq component-get apps      # Get all components in the 'apps' set
./moq component-get mr-boss   # Get only the 'mr-boss' component
```

## Repositories

MR Moqui spans five repositories in the [moqui](https://github.com/moqui) organization on GitHub. Component repositories use matching repository, `runtime/component` directory and component.xml names.

| Repository | What it is | Where it came from |
|---|---|---|
| [mr-moqui](https://github.com/moqui/mr-moqui) | The framework, the `moq` script, the base components (`runtime/base-component`: framework, tools, webroot), and the dev components (`runtime/dev-component`: dev-aide, example, the demo lock-down, and the component template) | A Rust rebuild of Moqui Java ([moqui-framework](https://github.com/moqui/moqui-framework) and [moqui-runtime](https://github.com/moqui/moqui-runtime)), started 2026-10-09 |
| [mr-mantle](https://github.com/moqui/mr-mantle) | Mantle Business Artifacts: the data model (entities and data) and the services | The Java components [mantle-udm](https://github.com/moqui/mantle-udm) and [mantle-usl](https://github.com/moqui/mantle-usl). They were forked as `mr-mantle-udm` and `mr-mantle-usl` and converted from Groovy to JavaScript. Then `mr-mantle-usl` was renamed and `mr-mantle-udm` was merged into it |
| [mr-marble](https://github.com/moqui/mr-marble) | Marble ERP and the Simple Screens library it is built from, with My Account | The Java components [SimpleScreens](https://github.com/moqui/SimpleScreens) and [MarbleERP](https://github.com/moqui/MarbleERP). They were forked as `MrSimpleScreens` and `MrMarbleERP` for the port. Then `MrSimpleScreens` was renamed and `MrMarbleERP` was merged into it |
| [mr-boss](https://github.com/moqui/mr-boss) | IronStone BOSS: Commons & Comms (groupware and communication), Projects, and the other BOSS applications | New for MR Moqui. The repository was named `ironstone-boss` until 2026-10-25 |
| [mr-pass](https://github.com/moqui/mr-pass) | IronStoa PASS: the public-facing companion suite for Forum, Blog/Publishing and Commerce | New for MR Moqui; design stage |

The merges into `mr-mantle` and `mr-marble` kept the commit history of both repositories. On GitHub the renamed repositories redirect from their old names. A Java component that depends on `mantle-udm`, `mantle-usl`, `SimpleScreens` or `MarbleERP` now depends on `mr-mantle` or `mr-marble`, and its `component://` locations change the same way. See [Moqui Java migration notes](docs/dev/Moqui-Java-Migration.md).

## Goals

- Keep XML as the spec language for configuration, entities, services, screens, SECAs, and data files, with only the adaptations required by a Rust runtime.
- Move the heavy lifting (entity facade, service facade, screen rendering, tooling, and performance-sensitive service implementations) into Rust.
- Use one language (JavaScript) for scripts and templates on the server, and for inline expressions on the client, so that XML Screens can become client-renderable. The engine is QuickJS.
- Default to an embedded Postgres (`postgresql_embedded`, theseus) when no external database is configured, and use an external Postgres when one is. The same engine backs the sim-mode overlay.
- Make OpenSearch optional from day one by planning a search interface that can be satisfied by Postgres full-text search and JSON operators.
- Preserve the ExecutionContext / facade shape that makes Moqui’s artifact model work.

## Significant Redesigns vs Moqui Java

Most of MR-Moqui is a port of the Java framework as it exists when the port begins. These are the places that change on purpose.

1. **JavaScript for scripts and templates.** Scripts, XML actions, screen macros and text templates are JavaScript on QuickJS; `ec.resource.expand()` is a template literal, and a small `if`/`list`/`assign` renderer (with a locked-down sandbox variant) handles structured text. Plan: [Architecture-Proposal](plans/archive/Architecture-Proposal.md) (Templating).
2. **SearchFacade replaces ElasticFacade.** Postgres full-text and JSON are the default backend; OpenSearch is optional. Plan: [SearchFacade](plans/archive/SearchFacade.md).
3. **qvt can be server-static.** With `server-static` set for `qvt`, the server sends a data-free compiled template and the client merges the data. Opt-in. Plan: [QvtServerStatic](plans/archive/QvtServerStatic.md).
4. **Print HTML is its own render mode.** `html-print` (print CSS and `@page`, also selected by the old `xsl-fo` name) is printed to PDF by one long-lived external Chromium; `/fop/<path>` sends a screen as PDF. Plan: [Architecture-Proposal](plans/archive/Architecture-Proposal.md) (Tool components and print).
5. **Cluster coordination uses the shared database.** Members find each other in a table; cache invalidation and notifications fan out over client-certificate HTTPS; no Hazelcast or Redis. Plan: [Cluster](plans/archive/Cluster.md).
6. **Sessions live in the database** (`moqui.server.HttpSession`, through the entity cache), with an optional guarantee mode so a load balancer without stickiness can fail over. Plan: [Architecture-Proposal](plans/archive/Architecture-Proposal.md) (Sessions and login).
7. **Keys are configuration, and unset secrets are generated.** Secrets are `default-property`s set by conf file or environment, never database rows; one still at its `CHANGEME` default is generated and saved to `runtime/.env`, which is loaded at startup like environment variables. Plan: [MimoReview](plans/archive/MimoReview.md) (Decisions 1–3).
8. **Feed registration commits with the entity change.** Search document keys are registered in the entity transaction; assembly and indexing run after commit, and an index failure does not roll back the change. Plan: [SearchFacade](plans/archive/SearchFacade.md).

## Native components

Components can supply linked Rust services, tools, resource schemes, template renderers and custom service runners,
as a Gradle build did for a Java Moqui component: a `Cargo.toml` and `src/` beside the `component.xml`, which
`./moq build` links into the binary. Service XML still defines services; component configuration selects
implementations, and scripts get tools with `ec.factory.getTool("Name")`. See
[Native component usage](docs/dev/Native-Components.md), the [plan](plans/archive/Native-Components.md), and
`runtime/dev-component/example`, which also holds the screen archetype gallery (`/qapps/gallery`); it loads on a dev
instance, and `./moq run -D example_disabled=true` leaves it out.

## Documentation

Reference guides for built features are in [`docs/`](docs/), by tier and area with an index in [`docs/README.md`](docs/README.md), including [building and testing](docs/Build-Test.md), [running and deploying](docs/Run-Deploy.md) and the practices for [application development](docs/dev/Practices-App-Dev.md) and [Rust](docs/dev/Practices-Rust.md). The [security policy](SECURITY.md) links to the attack-surface map and the HTTP proof tests. For a Moqui Java deployment or component moving to MR Moqui, start with the [Moqui Java migration notes](docs/dev/Moqui-Java-Migration.md). Design, status and review documents are in [`plans/`](plans/), indexed by [`plans/README.md`](plans/README.md): what is still open, what comes next, the proposals, and the archive of built designs and finished reviews.

## Build, Test, and Run

Building from source, the build packages and their features, the `./moq` tasks, and the Rust tests and component specs
are in [`docs/Build-Test.md`](docs/Build-Test.md). Running and deploying (the runtime directory and conf lookup, HTTPS
certificates and ports, production archives, clusters, and PASS and team topologies) are in
[`docs/Run-Deploy.md`](docs/Run-Deploy.md); Podman and systemd are in [`podman/README.md`](podman/README.md).

## AI/LLM Policy

Moqui was originally designed and built largely to reduce the human pain and suffering involved with building and maintaining enterprise applications. The same philosophy applies to the world of AI, from LLMs to harnesses, and from harnesses used to make Moqui to those built into it.

This project is an AI-assisted rebuild of the manually written Moqui Framework in Rust. It would likely never be done without AI assistance migrating and filling in code. AI assistance to migrate existing components made for Moqui is also recommended. Without that being available, migration from Groovy to JavaScript would be a much more expensive thing to do and it is much less likely we'd ever do it.

LLMs are considered tools used by people. The person using the tool is just as responsible as with a text editor and keyboard. LLMs are the functional and moral equivalent of a hammer: they do not shake themselves, and are not a substitute or replacement for a responsible party.

Don't trust, verify. Use AI to help verify, but remember who is responsible, and who is a who.
