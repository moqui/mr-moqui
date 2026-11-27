# Security Policy

## Supported Versions

The primary supported version for this repository is the latest commit in the master (primary) branch.

Moqui Ecosystem projects are maintained by volunteer contributors, primarily people who use and work with the code as part of their employment or professional services. There are periodic community releases but most distributions and releases involve custom code and are managed, internally or publicly, by third parties. Community releases are checkpoint releases, not maintained release branches, and are best for evaluation rather than production use.

Moqui uses a continuous release approach for managing code repositories. Aside from new (work-in-progress) and archived repositories, the master branch in each repository is considered production ready. Rather than running a centrally dictated release schedule and process, the focus is on keeping master branches in a production ready state so that users may use whatever release process and frequency they prefer.

For most use cases we recommend using code directly from the master branch in each repository. For stabilization and periodic updates (instead of continuous) we recommend using a fork for each git repository with an upstream remote pointing to the Moqui Ecosystem repository for easy upstream updates.

## Reporting a Vulnerability

**Before 0.1.0.** MR Moqui is not ready for production: run it only on development machines, with no real business
data or credentials. The first security review is finished (2026-11-27; its register is
[plans/archive/Security-Review.md](plans/archive/Security-Review.md), its map
[docs/security/](docs/security/Attack-Surface.md), and what it left in
[plans/Remaining-Work.md](plans/Remaining-Work.md#sessions-notifications-and-security)). Until 0.1.0 findings, open
vulnerabilities included, are recorded openly in this repository. With no production systems to protect, the
repository is the project's internal disclosure, and a finding may be reported in an issue or a pull request. From
0.1.0 the policy below applies, as it does for Moqui Java: a private report to the board, kept in the board's private
repository until it is fixed.

To report security issues that should not be disclosed publicly before they are fixed, please use the private **[moqui-board@googlegroups.com](mailto:moqui-board@googlegroups.com)** mailing list. Anyone can send messages to it. Only members of the group can read them.

## Issues and Pull Requests

For more information on submitting issues and pull requests please see the [Issue and Pull Request Guide](https://moqui.org/m/docs/moqui/Issue+and+Pull+Request+Guide) on moqui.org. Code contributions also need a signature in [AUTHORS.md](AUTHORS.md); its Contributing section says what happens to a pull request from an account that has not signed.

## Attack surface

A map of the HTTP routes, screens, REST, WebSocket, and the other listeners in this repository starts at [Attack-Surface.md](docs/security/Attack-Surface.md), which holds the lock-down options and the table of the map's files. Each area has its own file, `docs/security/Attack-Surface-*.md`: web authentication and controls, routes and REST, administration, tools, the LLM gateway (`/llm/v1`), A2A (default off), MCP and Assist, git, mail, DAV and SFTP, SSH, calls and messaging, the cluster and edge node, clients, stored secrets, and the BOSS and PASS applications.

Proof tests (what is covered, and how to run them with `moqui test-client`): [Proof-Tests.md](docs/security/Proof-Tests.md).
