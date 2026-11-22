# agent-code

Agent Code: coding agents (MR Moqui's own engine, Codex, Claude and others) on `AgentConversation`, from the web,
desktop, mobile and server clients. A port of T3 Code's features ([NOTICE](NOTICE)); the plan is
[plans/agent-code](../../../../plans/agent-code/README.md), and the user docs are
[docs/ai/Agent-Code.md](../../../../docs/ai/Agent-Code.md).

What is here:

- `entity/`: the Agent Code entities (`moqui.agent.code`) and their fields on the `moqui.agent` ones.
- `data/`: the enums, the permissions and groups (`AgentCodeUse`, `AgentCodeHost`, `AgentCodeAdmin`; the last two
  are sealed), the `AGENT_CODE_APP` artifact group and the effect and cleanup jobs.
- `service/org/moqui/code/`: the service definitions. The code is the Rust crate
  [crates/moqui-code](../../../../crates/moqui-code); `settings-schema.json` is the settings schema it reads.
- The socket `/agentws` and `GET /.well-known/moqui/agent-code` are the host's (`crates/moqui-host/src/agentws.rs`).
  They are on where the `/agentws` endpoint of the webroot webapp is (`enabled`: `true`, `false`, or `dev`, the
  default, for a dev instance only).

Test: `./moq test agent-code` (the regression specs in `test/`), and `cargo test -p moqui-code`.
