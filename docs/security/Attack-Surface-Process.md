# Attack surface: processes and build

Processes the server starts, the startup port check, and the build features that decide what is compiled in. This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## Other local processes

The `pdf` feature, which every server package includes, starts headless Chromium when a binary is found, with `--remote-debugging-address=127.0.0.1` and `--remote-debugging-port=0`. This tree does not add its own authentication on that socket. If no Chromium binary is found, the server still starts and PDF is unavailable.

An OpenSearch child starts only when `search_backend` is opensearch, `elasticsearch_url` is loopback (the property default is `http://127.0.0.1:9200`), nothing is already answering, `runtime/opensearch/bin` exists, `opensearch_run` is not `false` (default `true`), and there is no pid file. The default `search_backend` is `postgres`, so the child stays off. User and password properties default to empty.

## Startup port check

`/metrics` is on the public HTTP listener. It is not its own port. `port_check` probes HTTP, HTTPS, cluster, and SSH before the database opens. It does not probe SMTP.

## Build features

Default Cargo features are `maximal` and `devtools` (`crates/moqui-host/Cargo.toml`). `maximal` includes `enterprise`, which includes `git`, `llm`, `tui`, and `ssh`. `lite` includes `llm` and `tui`. `git` and `ssh` are on the `dev`, `enterprise`, and `maximal` sets. A default `cargo build` therefore compiles `/git`, `/termws`, and the LLM routes. `inbound-smtp` is on Enterprise and Maximal. `embedded-postgres` is on Dev, Lite, and Maximal. Enterprise leaves that feature out and uses an external database.
