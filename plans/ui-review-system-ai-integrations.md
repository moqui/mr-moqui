# MR Moqui — UI review: System AI and integrations

**Active pass · November 26.** [AI-B03](#ai-b03--november-26-change-led-review) records the selected branches and their outcomes. Earlier batches retain their dated evidence.

**Review update — 2026-11-25.** AI-B02 records the latest live evidence. The activity ledger identifies the branches still open.

**Status — 2026-11-25, refinement.** All three AI-B01 findings are fixed (AI-UI-001–003); the Entity Sync loop (AI-UI-002) now stops after one failed run.

**Previous status — 2026-11-25.** [AI-B01](#ai-b01--first-browser-review-2026-11-25) reached all 22 activity entry points and recorded three findings. One failed Entity Sync run produced 772 history rows (P1). The Remote Agent creation form omitted the private-network choice needed for a local peer (P2). Connection detail fields shared the same accessible name (P3). The ledger names the last verified step and outstanding branches for each journey.

Contents: [scope](#1-purpose-scope-and-sources) · [method](#2-review-method-and-refinement-criteria) ·
[inventory](#3-screen-and-surface-inventory) · [activities](#4-end-to-end-activities) ·
[verification](#5-existing-checks-and-future-verification) · [record](#6-coverage-findings-and-running-record) ·
[decisions](#7-decisions).

## 1. Purpose, scope and sources

Review the administrator's configuration-to-use loop for AI, messaging, protocols, voice and edge connectivity.
A configured row is only the beginning: follow a local request through its consumer, status, evidence and recovery.
Keep each integration's successful path separate from its unavailable-provider presentation.

Sources: [tools README](../runtime/components/base/tools/README.md),
[BasicComms README](../runtime/components/base/basic-comms/README.md), [Constitution](../docs/ai/Constitution.md),
[Comms](../docs/tools/Comms.md), [SIP](../docs/interfaces/SIP.md), [Chat bridges](../docs/interfaces/Chat-Bridges.md),
[LLM/Assist design](LLM-A2A-Assist.md), [edge build record](archive/Edge-Node.md), [edge follow-up](Edge-Node2.md),
current System subtrees below, BasicComms's System mounts, and framework agent/admin/edge/voice/chat services.
Current source controls take precedence over design paragraphs still labelled unbuilt.

### Ownership and handoffs

| Surface | Owner / handoff |
| --- | --- |
| Llm, Constitution, Integrations, Protocols, EdgeClients, EdgeNode, EntitySync | This plan, under `/qapps/system` |
| Voice and ChatBridges | This plan; source belongs to basic-comms and is mounted by its MoquiConf.xml |
| Llm/Traces include of Monitor/AgentTraces | [Monitor](ui-review-system-monitoring.md) owns diagnostic interaction; AI-03 checks run/context handoff |
| Assist tool consent, chat/mail/call usage | [Shared/Gallery](ui-review-shared-gallery.md) and [BasicComms](ui-review-basic-comms.md); only inspect this plan's side of each handoff |
| Core configuration, certificates, web protection, jobs, audit and user sessions | [System](ui-review-system.md) |
| Developer group-server setup and hosting interaction | [Dev AIDE Workspace](ui-review-dev-aide-workspace.md), not an AI model endpoint |

ApprovalDetail is an audit view: decisions occur through the owning approval channel, not an invented admin Approve
button. Monitor diagnostic policy and agent tool policy are different controls and are reviewed separately.

## 2. Review method and refinement criteria

Apply the [common review protocol](ui-review-shared-gallery.md#common-review-protocol) and
[verification policy](ui-review-shared-gallery.md#verification-policy-for-later-execution). The details below add this
area's fixtures, dependencies and judgment criteria.

### Environment, identities and records

- Prepare instance administrator, view-only System reader, no-System user and narrow action holders from current
  service guards. Keep `MONITOR_EVAL` (edit/score) separate from `MONITOR_EVAL_RUN` (start/cancel experiments), and
  use an explicit `ADMIN_CONSTITUTION_GLOBAL` holder for global edits. Agent ownership/audited reads, gateway client
  identity and integration/voice authority must be recorded from their loaded grants, not inferred from navigation.
- Use deterministic local model/A2A/MCP/gateway endpoints, synthetic tool requests, a harmless trigger and a small
  service-target evaluation dataset. Prepare successful, failed, waiting-approval and canceled runs. No real token
  billing, internet tool or production endpoint is a fixture. Record credentials only in disposable secret storage.
- Prepare local message transport/mail capture, registered connections/message types, failed/unknown delivery,
  synthetic SIP/media/bridge counterparts, queue agents and callers, and two isolated runtimes for edge/sync. Capture
  fixture IDs and relationship ownership before a run; do not reuse a delivery or queue record across activities.
- Check binary features/listeners/provider registration before attempting success. Voice/emergency exercises never
  dial a real number. Certificate/ACME belongs to SYS-19. Network/discovery actions stay within the local fixture.
  Pairing, configuration apply, listener restart and shared policy changes are serialized; restore all and revoke
  fixture keys/pairing codes at the end.

Use the exact seeded capability names below when preparing the narrow actors; the System screen grant and the
service's record/owner checks are additional requirements, not replaced by these permissions.

| Proposed identity | Explicit capability / boundary |
| --- | --- |
| `review.ai.user` | `AgentUse`; own Assist/chat-agent runs |
| `review.ai.skill.author` | `AgentSkillAuthor` with AgentUse; SH-18's personal Save as skill flow, distinct from AgentAdmin's shared-skill management |
| `review.ai.admin` | `AgentAdmin`; model/agent/skill/quota configuration. This does not imply `AgentAudit` |
| `review.ai.auditor` | `AgentAudit`; logged reads of other users' conversations/runs/approvals |
| `review.ai.gateway` | `LlmGatewayUse`; separately configured gateway client and key |
| `review.ai.remote` | `A2aServerUse` or `McpServerUse` for the respective local inbound fixture; remote-client configuration remains an administrator task |
| `review.ai.eval` / `review.ai.evalrun` | `MONITOR_EVAL` versus `MONITOR_EVAL_RUN`, with the edit permission added when the runner also authors the dataset |
| `review.ai.constitution` | `ADMIN_CONSTITUTION_GLOBAL`; global document and trusted origins |
| `review.integration.connection` | `ADMIN_CONNECTION_VIEW`, `ADMIN_CONNECTION_MANAGE`; registered connections/relays |
| `review.integration.operations` | `ADMIN_OPERATION_VIEW`, `ADMIN_OPERATION_MANAGE`; registered message/mail recovery |
| `review.voice.admin` / `review.bridge.admin` | `VoiceAdmin` / `ChatBridgeAdmin`; ordinary callers/room members remain separate counterparts |

These names come from `AgentTypeData.xml`, `AgentEvalData.xml`, `SecurityTypeData.xml`, `VoiceSetupData.xml` and
`ChatBridgeSetupData.xml`. Record any additional operation-specific grant in the batch instead of broadening every
fixture to ADMIN.

### Area-specific criteria

- Effective configuration, capability, enabled state and tested success must be distinguishable. Secrets are never
  returned in edit forms, audit detail, prompts, errors or exports after their intended one-time reveal.
- A conversation/run/call/approval/trace keeps its identifiers and audience across links. Audit access to another
  user's content is explicit; a list of runs is not blanket permission to inspect their payloads.
- Recovery names the failed item, sender/receiver, attempt and outcome. Independent messages/targets can continue;
  a single approval, pairing or delivery must retain operation identity through retry.
- Phone routing must explain where a call will go and whose policy applies. Edge setup must show before/after
  topology, effective routes and unverified requirements; a paired link alone does not prove every protocol works.

### Ordered waves

| Wave | Activities | Continuation |
| --- | --- | --- |
| 1. Establish AI behavior | AI-01–07 | Model/binding → prompt/tool/skill → run/approval/audit |
| 2. Operate AI | AI-08–11 | Trigger/eval/gateway/usage/Constitution with local fixtures |
| 3. Messages and mail | AI-12–15 | Connection → message/delivery → protocol/queue recovery |
| 4. Voice and bridges | AI-16–18 | Configure → BasicComms consumer → status/failure |
| 5. Edge and synchronization | AI-19–21 | Pair/apply/test → route/queue → record sync |
| 6. Reconcile | AI-22 | Access, aliases, output and remaining helper branches |

Prepare tools/skills before generating their runs; capture the approval before deciding it through SH-13. Keep
fixture runs until eval/audit review finishes. Edge setup consumes pairing codes and can alter protocol configuration,
so it uses its own runtime or follows mail/voice restoration.

## 3. Screen and surface inventory

This inventory accounts for **102 XML definitions** assigned to this file; screen containers and helper
definitions are included. Routes below are Material paths unless labelled otherwise; app routes have Standard
`/apps` counterparts. A file's physical presence does not prove access or a supported interaction in every mode.
Each row maps to §4; helper inspection does not replace its parent workflow. Counts describe definitions, not user
activities. Dialogs, templates and non-XML surfaces follow the route tables.

### basic-comms: `System`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/ChatBridges` | [ChatBridges.xml](../runtime/components/base/basic-comms/screen/System/ChatBridges.xml) | Mounted into System by basic-comms/MoquiConf.xml. | AI-18 |
| `/qapps/system/Voice` | [Voice.xml](../runtime/components/base/basic-comms/screen/System/Voice.xml) | Default: `Status`. Mounted into System by basic-comms/MoquiConf.xml. | AI-16, AI-17 |

### basic-comms: `System/Voice`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Voice/CallerIds` | [CallerIds.xml](../runtime/components/base/basic-comms/screen/System/Voice/CallerIds.xml) | Mounted into System by basic-comms/MoquiConf.xml. | AI-16 |
| `/qapps/system/Voice/Emergency` | [Emergency.xml](../runtime/components/base/basic-comms/screen/System/Voice/Emergency.xml) | Mounted into System by basic-comms/MoquiConf.xml. | AI-16 |
| `/qapps/system/Voice/Fraud` | [Fraud.xml](../runtime/components/base/basic-comms/screen/System/Voice/Fraud.xml) | Mounted into System by basic-comms/MoquiConf.xml. | AI-16 |
| `/qapps/system/Voice/IvrMenus` | [IvrMenus.xml](../runtime/components/base/basic-comms/screen/System/Voice/IvrMenus.xml) | Mounted into System by basic-comms/MoquiConf.xml. | AI-17 |
| `/qapps/system/Voice/Numbers` | [Numbers.xml](../runtime/components/base/basic-comms/screen/System/Voice/Numbers.xml) | Mounted into System by basic-comms/MoquiConf.xml. | AI-16, AI-17 |
| `/qapps/system/Voice/Phones` | [Phones.xml](../runtime/components/base/basic-comms/screen/System/Voice/Phones.xml) | Mounted into System by basic-comms/MoquiConf.xml.  Context: `includeRevoked`. | AI-16 |
| `/qapps/system/Voice/Queues` | [Queues.xml](../runtime/components/base/basic-comms/screen/System/Voice/Queues.xml) | Mounted into System by basic-comms/MoquiConf.xml. | AI-17 |
| `/qapps/system/Voice/RingGroups` | [RingGroups.xml](../runtime/components/base/basic-comms/screen/System/Voice/RingGroups.xml) | Mounted into System by basic-comms/MoquiConf.xml. | AI-17 |
| `/qapps/system/Voice/Schedules` | [Schedules.xml](../runtime/components/base/basic-comms/screen/System/Voice/Schedules.xml) | Mounted into System by basic-comms/MoquiConf.xml. | AI-17 |
| `/qapps/system/Voice/Status` | [Status.xml](../runtime/components/base/basic-comms/screen/System/Voice/Status.xml) | Mounted into System by basic-comms/MoquiConf.xml. | AI-16 |
| `/qapps/system/Voice/Trunks` | [Trunks.xml](../runtime/components/base/basic-comms/screen/System/Voice/Trunks.xml) | Mounted into System by basic-comms/MoquiConf.xml. | AI-16 |

### tools: `System`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Constitution` | [Constitution.xml](../runtime/components/base/tools/screen/System/Constitution.xml) | Default: `Rules`. | AI-11 |
| `/qapps/system/EdgeClients` | [EdgeClients.xml](../runtime/components/base/tools/screen/System/EdgeClients.xml) | Default: `Clients`. | AI-20 |
| `/qapps/system/EdgeNode` | [EdgeNode.xml](../runtime/components/base/tools/screen/System/EdgeNode.xml) | Default: `Status`. | AI-19, AI-20 |
| `/qapps/system/EntitySync` | [EntitySync.xml](../runtime/components/base/tools/screen/System/EntitySync.xml) | Default: `EntitySyncList`. | AI-21 |
| `/qapps/system/Integrations` | [Integrations.xml](../runtime/components/base/tools/screen/System/Integrations.xml) | Default: `Connections`. | AI-12, AI-13 |
| `/qapps/system/Llm` | [Llm.xml](../runtime/components/base/tools/screen/System/Llm.xml) | Default: `Dashboard`. | AI-01 |
| `/qapps/system/Protocols` | [Protocols.xml](../runtime/components/base/tools/screen/System/Protocols.xml) | Default: `Home`. | AI-14, AI-15 |

### tools: `System/Constitution`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Constitution/Edit` | [Edit.xml](../runtime/components/base/tools/screen/System/Constitution/Edit.xml) | Context: `locale`, `ruleId`. | AI-11 |
| `/qapps/system/Constitution/Json` | [Json.xml](../runtime/components/base/tools/screen/System/Constitution/Json.xml) | Inspect the route and its visible controls through the activity. | AI-11 |
| `/qapps/system/Constitution/Origins` | [Origins.xml](../runtime/components/base/tools/screen/System/Constitution/Origins.xml) | Inspect the route and its visible controls through the activity. | AI-11 |
| `/qapps/system/Constitution/Preview` | [Preview.xml](../runtime/components/base/tools/screen/System/Constitution/Preview.xml) | Context: `locale`. | AI-11 |
| `/qapps/system/Constitution/Rules` | [Rules.xml](../runtime/components/base/tools/screen/System/Constitution/Rules.xml) | Authored text: `html,html-print,text`, `qvt`, `tui`. Context: `locale`. | AI-11 |

### tools: `System/EdgeClients`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/EdgeClients/Balancing` | [Balancing.xml](../runtime/components/base/tools/screen/System/EdgeClients/Balancing.xml) | Authored text: `qvt`, `tui`. | AI-20 |
| `/qapps/system/EdgeClients/Clients` | [Clients.xml](../runtime/components/base/tools/screen/System/EdgeClients/Clients.xml) | Authored text: `qvt`, `tui`. | AI-20 |
| `/qapps/system/EdgeClients/Domains` | [Domains.xml](../runtime/components/base/tools/screen/System/EdgeClients/Domains.xml) | Authored text: `qvt`, `tui`. | AI-20 |
| `/qapps/system/EdgeClients/EdgeCluster` | [EdgeCluster.xml](../runtime/components/base/tools/screen/System/EdgeClients/EdgeCluster.xml) | Authored text: `qvt`, `tui`. | AI-20 |
| `/qapps/system/EdgeClients/Ports` | [Ports.xml](../runtime/components/base/tools/screen/System/EdgeClients/Ports.xml) | Authored text: `qvt`, `tui`. | AI-20 |

### tools: `System/EdgeNode`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/EdgeNode/Pair` | [Pair.xml](../runtime/components/base/tools/screen/System/EdgeNode/Pair.xml) | Default: `EdgeUrl`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `url`. | AI-19 |
| `/qapps/system/EdgeNode/Status` | [Status.xml](../runtime/components/base/tools/screen/System/EdgeNode/Status.xml) | Authored text: `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. | AI-20 |

### tools: `System/EdgeNode/Pair`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/EdgeNode/Pair/Apply` | [Apply.xml](../runtime/components/base/tools/screen/System/EdgeNode/Pair/Apply.xml) | Authored text: `qvt`, `tui`. | AI-19 |
| `/qapps/system/EdgeNode/Pair/Connect` | [Connect.xml](../runtime/components/base/tools/screen/System/EdgeNode/Pair/Connect.xml) | Context: `url`. | AI-19 |
| `/qapps/system/EdgeNode/Pair/Discover` | [Discover.xml](../runtime/components/base/tools/screen/System/EdgeNode/Pair/Discover.xml) | Inspect the route and its visible controls through the activity. | AI-19 |
| `/qapps/system/EdgeNode/Pair/EdgeUrl` | [EdgeUrl.xml](../runtime/components/base/tools/screen/System/EdgeNode/Pair/EdgeUrl.xml) | Context: `url`. | AI-19 |
| `/qapps/system/EdgeNode/Pair/Test` | [Test.xml](../runtime/components/base/tools/screen/System/EdgeNode/Pair/Test.xml) | Authored text: `qvt`, `tui`. | AI-19 |

### tools: `System/EntitySync`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/EntitySync/EntitySyncDetail` | [EntitySyncDetail.xml](../runtime/components/base/tools/screen/System/EntitySync/EntitySyncDetail.xml) | Context: `entitySyncId`. | AI-21 |
| `/qapps/system/EntitySync/EntitySyncHistory` | [EntitySyncHistory.xml](../runtime/components/base/tools/screen/System/EntitySync/EntitySyncHistory.xml) | Context: `entitySyncId`. | AI-21 |
| `/qapps/system/EntitySync/EntitySyncList` | [EntitySyncList.xml](../runtime/components/base/tools/screen/System/EntitySync/EntitySyncList.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | AI-21 |

### tools: `System/Integrations`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Integrations/ConnectionDetail` | [ConnectionDetail.xml](../runtime/components/base/tools/screen/System/Integrations/ConnectionDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `adapterKey`, `connectionKey`. | AI-12 |
| `/qapps/system/Integrations/Connections` | [Connections.xml](../runtime/components/base/tools/screen/System/Integrations/Connections.xml) | Authored text: `html`, `qvt`, `tui`. Context: `text`, `adapterKey`, `state`, `pageIndex`. | AI-12 |
| `/qapps/system/Integrations/MessageTypeDetail` | [MessageTypeDetail.xml](../runtime/components/base/tools/screen/System/Integrations/MessageTypeDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `systemMessageTypeId`. | AI-12 |
| `/qapps/system/Integrations/MessageTypes` | [MessageTypes.xml](../runtime/components/base/tools/screen/System/Integrations/MessageTypes.xml) | Authored text: `html`, `qvt`, `tui`. Context: `term`, `direction`, `pageIndex`. | AI-12 |
| `/qapps/system/Integrations/SystemMessageDetail` | [SystemMessageDetail.xml](../runtime/components/base/tools/screen/System/Integrations/SystemMessageDetail.xml) | Authored text: `html`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `systemMessageId`, `showContent`. | AI-13 |
| `/qapps/system/Integrations/SystemMessages` | [SystemMessages.xml](../runtime/components/base/tools/screen/System/Integrations/SystemMessages.xml) | Authored text: `html`, `qvt`, `tui`. Context: `state`, `systemMessageTypeId`, `systemMessageRemoteId`, `direction`, `reference`, `fromDate`, `thruDate`, `pageIndex`, `bulkIds`, `bulkAction`, `bulkReason`. | AI-13 |

### tools: `System/Integrations/SystemMessageDetail`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Integrations/SystemMessageDetail/EditMessageText` | [EditMessageText.xml](../runtime/components/base/tools/screen/System/Integrations/SystemMessageDetail/EditMessageText.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `systemMessageId`. | AI-13 |

### tools: `System/Llm`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Llm/Approvals` | [Approvals.xml](../runtime/components/base/tools/screen/System/Llm/Approvals.xml) | Default: `ApprovalList`. | AI-04 |
| `/qapps/system/Llm/Calls` | [Calls.xml](../runtime/components/base/tools/screen/System/Llm/Calls.xml) | Default: `CallList`. | AI-03 |
| `/qapps/system/Llm/Conversations` | [Conversations.xml](../runtime/components/base/tools/screen/System/Llm/Conversations.xml) | Default: `ConversationList`. | AI-03 |
| `/qapps/system/Llm/Dashboard` | [Dashboard.xml](../runtime/components/base/tools/screen/System/Llm/Dashboard.xml) | Inspect the route and its visible controls through the activity. | AI-01 |
| `/qapps/system/Llm/Evals` | [Evals.xml](../runtime/components/base/tools/screen/System/Llm/Evals.xml) | Default: `Experiments`. | AI-09 |
| `/qapps/system/Llm/Gateway` | [Gateway.xml](../runtime/components/base/tools/screen/System/Llm/Gateway.xml) | Default: `ClientList`. | AI-10 |
| `/qapps/system/Llm/Inbox` | [Inbox.xml](../runtime/components/base/tools/screen/System/Llm/Inbox.xml) | Inspect the route and its visible controls through the activity. | AI-01, AI-08 |
| `/qapps/system/Llm/Lessons` | [Lessons.xml](../runtime/components/base/tools/screen/System/Llm/Lessons.xml) | Inspect the route and its visible controls through the activity. | AI-08 |
| `/qapps/system/Llm/Models` | [Models.xml](../runtime/components/base/tools/screen/System/Llm/Models.xml) | Default: `ModelList`. | AI-02 |
| `/qapps/system/Llm/PromptPreview` | [PromptPreview.xml](../runtime/components/base/tools/screen/System/Llm/PromptPreview.xml) | Context: `agentId`, `surface`, `variant`, `modelId`, `userId`, `samplePath`. | AI-02, AI-07, AI-11 |
| `/qapps/system/Llm/Remote` | [Remote.xml](../runtime/components/base/tools/screen/System/Llm/Remote.xml) | Default: `AgentList`. | AI-06 |
| `/qapps/system/Llm/Runs` | [Runs.xml](../runtime/components/base/tools/screen/System/Llm/Runs.xml) | Default: `RunList`. | AI-03 |
| `/qapps/system/Llm/Skills` | [Skills.xml](../runtime/components/base/tools/screen/System/Llm/Skills.xml) | Default: `SkillList`. | AI-05 |
| `/qapps/system/Llm/Tools` | [Tools.xml](../runtime/components/base/tools/screen/System/Llm/Tools.xml) | Default: `ExposureList`. | AI-07 |
| `/qapps/system/Llm/Traces` | [Traces.xml](../runtime/components/base/tools/screen/System/Llm/Traces.xml) | Wrapper of Monitor/AgentTraces; MON-12 owns diagnostics, AI-03 owns run/context handoff. Includes `component://tools/screen/System/Monitor/AgentTraces.xml`. Context: `from`, `thru`, `provider`, `model`, `agent`, `errorsOnly`. | AI-03, AI-22 |
| `/qapps/system/Llm/Triggers` | [Triggers.xml](../runtime/components/base/tools/screen/System/Llm/Triggers.xml) | Default: `TriggerList`. | AI-08 |
| `/qapps/system/Llm/Usage` | [Usage.xml](../runtime/components/base/tools/screen/System/Llm/Usage.xml) | Inspect the route and its visible controls through the activity. | AI-01, AI-10 |

### tools: `System/Llm/Approvals`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Llm/Approvals/ApprovalDetail` | [ApprovalDetail.xml](../runtime/components/base/tools/screen/System/Llm/Approvals/ApprovalDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `approvalId`. | AI-04 |
| `/qapps/system/Llm/Approvals/ApprovalList` | [ApprovalList.xml](../runtime/components/base/tools/screen/System/Llm/Approvals/ApprovalList.xml) | Inspect the route and its visible controls through the activity. | AI-04 |

### tools: `System/Llm/Calls`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Llm/Calls/CallDetail` | [CallDetail.xml](../runtime/components/base/tools/screen/System/Llm/Calls/CallDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `callId`. | AI-03 |
| `/qapps/system/Llm/Calls/CallList` | [CallList.xml](../runtime/components/base/tools/screen/System/Llm/Calls/CallList.xml) | Inspect the route and its visible controls through the activity. | AI-03 |

### tools: `System/Llm/Conversations`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Llm/Conversations/ConversationDetail` | [ConversationDetail.xml](../runtime/components/base/tools/screen/System/Llm/Conversations/ConversationDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `conversationId`. | AI-03 |
| `/qapps/system/Llm/Conversations/ConversationList` | [ConversationList.xml](../runtime/components/base/tools/screen/System/Llm/Conversations/ConversationList.xml) | Inspect the route and its visible controls through the activity. | AI-03 |

### tools: `System/Llm/Evals`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Llm/Evals/Compare` | [Compare.xml](../runtime/components/base/tools/screen/System/Llm/Evals/Compare.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `baseId`, `otherId`. | AI-09 |
| `/qapps/system/Llm/Evals/DatasetDetail` | [DatasetDetail.xml](../runtime/components/base/tools/screen/System/Llm/Evals/DatasetDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `datasetId`. | AI-09 |
| `/qapps/system/Llm/Evals/Datasets` | [Datasets.xml](../runtime/components/base/tools/screen/System/Llm/Evals/Datasets.xml) | Inspect the route and its visible controls through the activity. | AI-09 |
| `/qapps/system/Llm/Evals/ExperimentDetail` | [ExperimentDetail.xml](../runtime/components/base/tools/screen/System/Llm/Evals/ExperimentDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `experimentId`. | AI-09 |
| `/qapps/system/Llm/Evals/Experiments` | [Experiments.xml](../runtime/components/base/tools/screen/System/Llm/Evals/Experiments.xml) | Context: `datasetId`. | AI-09 |
| `/qapps/system/Llm/Evals/Runs` | [Runs.xml](../runtime/components/base/tools/screen/System/Llm/Evals/Runs.xml) | Context: `agentId`, `runId`, `scoredOnly`. | AI-09 |
| `/qapps/system/Llm/Evals/Scorers` | [Scorers.xml](../runtime/components/base/tools/screen/System/Llm/Evals/Scorers.xml) | Inspect the route and its visible controls through the activity. | AI-09 |

### tools: `System/Llm/Gateway`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Llm/Gateway/AliasList` | [AliasList.xml](../runtime/components/base/tools/screen/System/Llm/Gateway/AliasList.xml) | Inspect the route and its visible controls through the activity. | AI-10 |
| `/qapps/system/Llm/Gateway/ClientDetail` | [ClientDetail.xml](../runtime/components/base/tools/screen/System/Llm/Gateway/ClientDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `clientId`. | AI-10 |
| `/qapps/system/Llm/Gateway/ClientList` | [ClientList.xml](../runtime/components/base/tools/screen/System/Llm/Gateway/ClientList.xml) | Inspect the route and its visible controls through the activity. | AI-10 |

### tools: `System/Llm/Models`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Llm/Models/EndpointDetail` | [EndpointDetail.xml](../runtime/components/base/tools/screen/System/Llm/Models/EndpointDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `endpointId`. | AI-02 |
| `/qapps/system/Llm/Models/ModelDetail` | [ModelDetail.xml](../runtime/components/base/tools/screen/System/Llm/Models/ModelDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `modelId`. | AI-02 |
| `/qapps/system/Llm/Models/ModelList` | [ModelList.xml](../runtime/components/base/tools/screen/System/Llm/Models/ModelList.xml) | Inspect the route and its visible controls through the activity. | AI-02 |

### tools: `System/Llm/Remote`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Llm/Remote/AgentDetail` | [AgentDetail.xml](../runtime/components/base/tools/screen/System/Llm/Remote/AgentDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `remoteId`. | AI-06 |
| `/qapps/system/Llm/Remote/AgentList` | [AgentList.xml](../runtime/components/base/tools/screen/System/Llm/Remote/AgentList.xml) | Inspect the route and its visible controls through the activity. | AI-06 |
| `/qapps/system/Llm/Remote/McpServerDetail` | [McpServerDetail.xml](../runtime/components/base/tools/screen/System/Llm/Remote/McpServerDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `mcpServerId`, `listNow`. | AI-06 |
| `/qapps/system/Llm/Remote/McpServerList` | [McpServerList.xml](../runtime/components/base/tools/screen/System/Llm/Remote/McpServerList.xml) | Inspect the route and its visible controls through the activity. | AI-06 |

### tools: `System/Llm/Runs`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Llm/Runs/RunDetail` | [RunDetail.xml](../runtime/components/base/tools/screen/System/Llm/Runs/RunDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `runId`. | AI-03 |
| `/qapps/system/Llm/Runs/RunList` | [RunList.xml](../runtime/components/base/tools/screen/System/Llm/Runs/RunList.xml) | Inspect the route and its visible controls through the activity. | AI-03 |

### tools: `System/Llm/Skills`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Llm/Skills/RetrievalTest` | [RetrievalTest.xml](../runtime/components/base/tools/screen/System/Llm/Skills/RetrievalTest.xml) | Context: `query`, `page`, `userId`, `agentId`, `surface`. | AI-05 |
| `/qapps/system/Llm/Skills/SkillDetail` | [SkillDetail.xml](../runtime/components/base/tools/screen/System/Llm/Skills/SkillDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `skillId`. | AI-05 |
| `/qapps/system/Llm/Skills/SkillList` | [SkillList.xml](../runtime/components/base/tools/screen/System/Llm/Skills/SkillList.xml) | Inspect the route and its visible controls through the activity. | AI-05 |

### tools: `System/Llm/Tools`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Llm/Tools/ExposureDetail` | [ExposureDetail.xml](../runtime/components/base/tools/screen/System/Llm/Tools/ExposureDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `toolName`. | AI-07 |
| `/qapps/system/Llm/Tools/ExposureList` | [ExposureList.xml](../runtime/components/base/tools/screen/System/Llm/Tools/ExposureList.xml) | Inspect the route and its visible controls through the activity. | AI-07 |
| `/qapps/system/Llm/Tools/Policies` | [Policies.xml](../runtime/components/base/tools/screen/System/Llm/Tools/Policies.xml) | Context: `agentId`. | AI-07 |

### tools: `System/Llm/Triggers`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Llm/Triggers/TriggerDetail` | [TriggerDetail.xml](../runtime/components/base/tools/screen/System/Llm/Triggers/TriggerDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `triggerId`. | AI-08 |
| `/qapps/system/Llm/Triggers/TriggerList` | [TriggerList.xml](../runtime/components/base/tools/screen/System/Llm/Triggers/TriggerList.xml) | Inspect the route and its visible controls through the activity. | AI-08 |

### tools: `System/Protocols`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/system/Protocols/Home` | [Home.xml](../runtime/components/base/tools/screen/System/Protocols/Home.xml) | Authored text: `html`, `qvt`, `tui`. | AI-14, AI-15 |
| `/qapps/system/Protocols/MailQueue` | [MailQueue.xml](../runtime/components/base/tools/screen/System/Protocols/MailQueue.xml) | Authored text: `html`, `qvt`, `tui`. Context: `state`, `binding`, `emailServerId`, `fromDate`, `thruDate`, `pageIndex`. | AI-15 |
| `/qapps/system/Protocols/MailRelayDetail` | [MailRelayDetail.xml](../runtime/components/base/tools/screen/System/Protocols/MailRelayDetail.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `connectionKey`. | AI-14, AI-15 |
| `/qapps/system/Protocols/MailRelays` | [MailRelays.xml](../runtime/components/base/tools/screen/System/Protocols/MailRelays.xml) | Authored text: `html`, `qvt`, `tui`. Context: `text`. | AI-14, AI-15 |
| `/qapps/system/Protocols/ServerConfiguration` | [ServerConfiguration.xml](../runtime/components/base/tools/screen/System/Protocols/ServerConfiguration.xml) | Authored text: `html`, `qvt`, `tui`. Context: `protocolKey`. | AI-14, AI-15 |

### Embedded surfaces, dialogs, outputs and reconciliation

| Supporting surface | Activity / inspection |
| --- | --- |
| Model/endpoint credentials, bindings, capability probe and PromptPreview sections | AI-02/07/11; explicit versus detected capability and effective policy |
| Run steps/events/tool calls, conversation messages, call request/response, approval changes/decision metadata | AI-03/04; audited/redacted detail, no synthetic admin approval control |
| Skill sections/retrieval/use history; remote A2A/MCP discovery/allowlist | AI-05–07; listing a tool is distinct from exposing/allowing it |
| Trigger event/breaker/run controls, Inbox, Lessons review | AI-08; eligible actor and proposed/accepted state |
| Eval dataset/items/scorers/live attachment/experiment result review/compare | AI-09; dataset version at start, partial/canceled result and human score |
| Gateway clients/key/aliases and Usage quota/rollup | AI-10; gateway credentials distinct from upstream endpoint secrets |
| Constitution Rules/Edit/Json/Origins/Preview and [ConstitutionParts.xml](../runtime/components/base/tools/template/ConstitutionParts.xml) source/language sections; trusted reference viewer in Assist | AI-11; shipped/custom/revision/locale and exact trusted origins |
| [ConnectionParts.xml](../runtime/components/base/tools/template/ConnectionParts.xml) forms, checks and save/test/enable transitions; message type registration/raw forms; payload download/edit, per-item and bulk recovery | AI-12/13; owner semantics and staged incoming message lifecycle |
| Protocol readiness, MailRelayDetail included forms, MailQueue settle/retry | AI-14/15; hosted account SMTP ownership is not a general relay |
| Voice row dialogs and [VoiceTransitions.xml](../runtime/components/base/basic-comms/template/VoiceTransitions.xml) user/route lookups: trunks/numbers/caller IDs, fraud, emergency, devices, ring groups/schedules/IVR/queues and audio | AI-16/17; each child screen explicitly inventoried above |
| ChatBridges status/disable/retry and room bridge settings in BasicComms | AI-18 and COM-08; admin/room ownership boundary |
| Edge Pair wizard's five steps, Apply confirmation, route/domain/port/cluster/queue controls | AI-19/20; System certificates/protection linked, not duplicated |
| EntitySync selection/artifacts/history/reset | AI-21; explicit fixture records, not production resynchronization |

The archived review deferred deep LLM/edge/voice/bridge work. New Constitution/eval/protocol/routing controls are
included from source. Design-only fleet/sealed-secret features and absent provider integrations remain out of scope.
Successful local simulation and a successful external provider must never be conflated.

## 4. End-to-end activities

Use visible controls and inspect intermediate states, confirmation and the resulting data/output. Unless narrowed
below, each activity has Material light/dark, actual supported TUI and a brief Standard/output branch. Select narrow
and keyboard checks where layout/interaction differs. Record each unattempted branch in §6; a service call or render
alone does not complete the activity. Names beginning `review.` are fixture proposals, not shipped accounts.

### AI-01 — Orient to AI health and outstanding work

- **Actor/start:** System reader and AI administrator; local failed and active runs/calls.
- **Objective/steps:** Llm/Dashboard → inspect failure counts → Calls/Runs/Inbox → selected detail → Usage →
  return; compare with an empty/disabled-provider setup.
- **Expected:** Counts, status, actor and provider identity agree with destination records; unavailable
  configuration is actionable without implying a working model.
- **Recovery:** No model, unknown quota, missing run and independent backend failure leave other summaries
  usable.
- **Screens/modes:** Llm root/Dashboard, Inbox, Calls/Runs/Usage; browser/TUI/Standard and narrow summary
  layout.

### AI-02 — Configure and test a model binding

- **Actor/start:** Endpoint/model administrator and view-only reader; deterministic local model server and
  invalid-key fixture.
- **Objective/steps:** Models → create EndpointDetail/set secret/test → create ModelDetail → probe
  capabilities/set explicit capability → bind use case → PromptPreview → harmless Assist request → inspect
  CallDetail.
- **Expected:** Endpoint, model, binding and effective capability are clear; secrets do not reappear; probe
  failure is distinct from saved configuration; call uses selected binding.
- **Recovery:** Wrong key, invalid endpoint, unavailable model, unsupported capability, stale edit and canceled
  delete retain understandable state.
- **Screens/modes:** ModelList/ModelDetail/EndpointDetail, PromptPreview, Calls; actual local reply required,
  SH-13 owns Assist interaction.

### AI-03 — Trace a conversation through its run and provider call

- **Actor/start:** Run owner and authorized audit reader; local conversation with tool step and known trace.
- **Objective/steps:** Conversations/List → Detail → RunDetail → steps/events/tool calls → Calls/CallDetail →
  model → Llm/Traces → Monitor trace → return.
- **Expected:** IDs and timestamps join the same execution; another user content is withheld or explicitly
  audited; cancel is shown only for a valid current run.
- **Recovery:** Missing/expired trace, redacted body, canceled run, failed call and revoked audit authority do
  not expose content or lose the originating conversation.
- **Screens/modes:** Conversations, Runs, Calls, Traces wrapper; MON-12 owns AgentTraces behavior; TUI/Standard
  long payload disclosures.

### AI-04 — Inspect the lifecycle of a tool approval

- **Actor/start:** Run owner, actual approver through Assist and authorized audit reader; harmless proposed
  write with preview.
- **Objective/steps:** Approvals/List → Detail inspect target/change/expiry → Run → open owning approval channel
  → reject one proposal, approve a fresh one → reopen audit detail and run outcome.
- **Expected:** Proposed, decided and executed are distinct; decision actor/channel/time match the actual
  consent and exact operation; detail remains an audit surface.
- **Recovery:** Expired/stale approval, changed arguments, canceled consent, duplicate response and lost
  permission cannot approve a different action.
- **Screens/modes:** Approvals/ApprovalList/ApprovalDetail, RunDetail and SH-13 consent handoff; no invented
  Approve button in System.

### AI-05 — Maintain a skill and evaluate retrieval

- **Actor/start:** Skill administrator and reader; disposable local skill source with several sections and known
  query.
- **Objective/steps:** Skills/List → sync/create → SkillDetail edit/inspect sections → RetrievalTest → run query
  → inspect matched section → follow use to RunDetail → revise and retry.
- **Expected:** Source, version/sections and matched evidence are visible; retrieval results do not imply the
  skill ran; use history links to the right execution.
- **Recovery:** Malformed source, empty retrieval, unavailable embedding, failed sync item and denied edit leave
  the other skills usable.
- **Screens/modes:** Skills containers, SkillList/Detail/RetrievalTest and RunDetail; browser/TUI/Standard, long
  section content.
- **Personal-skill handoff:** SH-18 owns Assist's My skills and Save as skill interaction. Inspect its Share for
  review site copy here, compare ownership/status/source with the personal original, apply the eligible admin
  review/edit/lock controls, and verify retrieval/use. Retiring a personal skill and reviewing a shared copy must
  not be confused with one another.

### AI-06 — Connect a remote agent or MCP server locally

- **Actor/start:** Remote administrator with separate local A2A/MCP fixtures, reader and restricted tool user.
- **Objective/steps:** Remote AgentList → AgentDetail configure/test; McpServerList → Detail set credential/list
  tools → explicitly allow selected tool → inspect exposure/policy → run harmless request.
- **Expected:** Discovered, allowed and exposed tools are distinct; remote identity/capability and credentials
  are clear; only intended tool is usable.
- **Recovery:** Unreachable fixture, bad credential, malformed discovery, removed tool and refused invocation
  preserve safe configuration and useful error detail.
- **Screens/modes:** Remote AgentList/Detail, McpServerList/Detail and Tools exposure handoff;
  browser/TUI/Standard, no internet endpoints.

### AI-07 — Explain effective tools and prompt policy

- **Actor/start:** Tool/policy administrator, ordinary agent user and refusal counterpart; harmless local
  tool/skill/model.
- **Objective/steps:** Tools/ExposureList → Detail configure → Policies choose agent/add rule → PromptPreview
  inspect included sections/tools → Assist request → compare allowed/refused tool outcome.
- **Expected:** Prompt and policy show the actual effective limits; naming/exposure is not permission; a refused
  invocation identifies the relevant rule without leaking secret arguments.
- **Recovery:** Unknown service/tool, invalid policy, missing capability, stale prompt and canceled delete
  retain readable configuration.
- **Screens/modes:** ExposureList/Detail, Policies, PromptPreview, model/skill links and SH-13;
  browser/TUI/Standard.

### AI-08 — Follow a trigger, its breaker and a proposed lesson

- **Actor/start:** Trigger operator and reader; local event with deterministic agent reply and a separate
  failing event.
- **Objective/steps:** Triggers/List → Detail configure/fire → inspect events/run → Inbox item → Run → Lessons
  inspect/review proposed lesson → return; inspect/reset a prepared breaker.
- **Expected:** Trigger status, event, run and resulting proposal remain linked; reset is distinguishable from
  rerun; review state changes only by its eligible actor.
- **Recovery:** Repeated event, disabled trigger, open breaker, missing owner, failed run and rejected lesson
  preserve evidence and do not create unbounded retries.
- **Screens/modes:** TriggerList/Detail, Inbox, Lessons and Runs; actual fixture events, TUI/Standard review
  controls.

### AI-09 — Build and compare a bounded evaluation

- **Actor/start:** MONITOR_EVAL editor/scorer, separate MONITOR_EVAL_RUN holder and reader; small service-target
  local dataset.
- **Objective/steps:** Evals/Datasets → DatasetDetail add/edit items/attach scorer → Scorers configure →
  Experiments start → ExperimentDetail follow results → review a result → Compare two runs → Runs save/score a
  selected agent run.
- **Expected:** Experiment records dataset version at start; score meaning, failures and human review are
  visible; totals do not hide partial/canceled items.
- **Recovery:** Invalid scorer/item, denied start, changed dataset mid-run, failed result and canceled
  experiment preserve completed evidence and name unresolved items.
- **Screens/modes:** All Evals children, including Compare/Runs; no paid model needed, browser/TUI/Standard and
  wide comparison readability.

### AI-10 — Operate a gateway client and explain usage

- **Actor/start:** Gateway administrator, local gateway client and System reader; limited alias/quota and known
  requests.
- **Objective/steps:** Gateway/ClientList → ClientDetail set key/limits → AliasList configure → send local
  client request → inspect CallList/Detail → Usage quotas/rollup → disable/revoke fixture access.
- **Expected:** Client key is distinct from upstream credential; alias resolves as configured; usage and quota
  periods explain allowed/refused calls without a misleading zero.
- **Recovery:** Unknown alias, expired key, limit exceeded, failed upstream and delayed rollup retain request
  identity and clear accounting limits.
- **Screens/modes:** Gateway ClientList/Detail/AliasList, Usage, Calls; local request is fixture stimulus,
  visible management is the activity.

### AI-11 — Customize and verify the Constitution

- **Actor/start:** ADMIN_CONSTITUTION_GLOBAL holder and non-holder; disposable instance document and trusted
  local HTTPS reference fixture.
- **Objective/steps:** Constitution/Rules → Customize → Edit rule/reference/locale → Preview → Json
  inspect/download → Origins add exact origin → Assist reference viewer → reset to shipped document after
  comparison.
- **Expected:** Shipped versus custom and revision are clear; preview reflects the same rules; trusted origins
  and language fallback are explainable; reset is explicit.
- **Recovery:** Invalid JSON/rule, stale hash, denied edit, bad origin, missing locale and canceled reset keep
  the last good document and restrict unsafe references.
- **Screens/modes:** Rules/Edit/Json/Origins/Preview plus SH-13 reference viewer; browser/TUI/Standard,
  inspected JSON bytes.

### AI-12 — Configure a registered connection and inspect its contract

- **Actor/start:** Connection operator and reader; local gateway/remote/mail/printer fixture registered by its
  owner.
- **Objective/steps:** Integrations/Connections → create/select → ConnectionDetail configure/test where offered
  → inspect mappings → MessageTypes/detail → verify owner/allowed use → follow consumer.
- **Expected:** Connection kind, owner, enabled state and tested outcome are distinct; a mail-account-owned
  server is not presented as an instance relay.
- **Recovery:** Unsupported kind, invalid mapping, rejected secret, failed test and stale record keep editable
  non-secret values and a useful return path.
- **Screens/modes:** Connections/ConnectionDetail, MessageTypes/MessageTypeDetail, included ConnectionParts;
  System/Comms consumer handoffs, TUI/Standard.
- **Readiness branch:** On a native registered connection, save configuration → try enabling before a passing
  test → Test → inspect whether the test enabled it → enable/disable as offered → change a connection-relevant
  field → recheck required testing and its consumer. Exercise stale revisions and blank-secret preservation.
  Follow the owning editor for kinds that delegate configuration; a raw message-remote enum mapping is a separate
  branch. The shared forms/transitions are [ConnectionParts.xml](../runtime/components/base/tools/template/ConnectionParts.xml).

### AI-13 — Recover a message without repeating completed work

- **Actor/start:** Message operator and reader; local registered transport with staged, failed, acknowledged and
  partial-batch messages.
- **Objective/steps:** SystemMessages → inspect/filter → SystemMessageDetail → view/download payload → inspect
  attempts/history/ack/children → choose eligible recovery → preview bulk recovery → apply → inspect each
  result; edit only a disposable staged incoming message.
- **Expected:** Owner, direction, attempt and status are clear; acknowledgment is not confused with sending;
  independent successful messages remain complete and payload edits identify the selected message.
- **Recovery:** Malformed payload, unavailable owner, stale recovery, denied send, canceled bulk action and
  unknown outcome do not manufacture a successful retry.
- **Screens/modes:** SystemMessages, SystemMessageDetail/EditMessageText, MessageTypeDetail; actual local
  transport evidence and payload bytes, TUI/Standard.

### AI-14 — Understand protocol readiness and configure a relay

- **Actor/start:** Protocol/relay operator and reader; local mail relay/listener and missing/disabled/failed
  feature cases.
- **Objective/steps:** Protocols/Home → ServerConfiguration inspect effective source → run explicitly local
  check → MailRelays → Detail configure/test → consumer sends captured mail → readiness refresh.
- **Expected:** Not-built, disabled, failed-start and running states differ; checked public/network behavior is
  separate from local configuration; relay ownership and recipients are clear.
- **Recovery:** Bad relay settings, protocol off, refused outgoing check, missing trust prerequisites and failed
  startup remain visible without contacting unrelated endpoints.
- **Screens/modes:** Home, ServerConfiguration, MailRelays/Detail and System readiness/BasicComms handoff;
  browser/TUI/Standard.
- **Check boundary:** ServerConfiguration is read-only configuration plus explicit checks: Configuration sends
  nothing, Listener connects to the configured bind address, and Public address checks the advertised target after
  confirmation. Use a local advertised-target fixture, cancel once, then inspect the recorded result/time. There is
  no editable arbitrary check URL or restart action here; copied configuration changes/reload belong to SYS-06.

### AI-15 — Resolve failed and uncertain mail outcomes

- **Actor/start:** Mail outcome operator, owner and reader; one failed recipient and one unknown outcome with
  local capture.
- **Objective/steps:** MailQueue → filter owner/status → inspect privacy-preserving metadata → follow owner
  record → retry failed recipient → settle uncertain result only with evidence → inspect owner delivery and
  queue.
- **Expected:** Queue does not expose message body/address to unrelated admins; retries target failed recipients
  only; settled unknown outcomes have explicit evidence and actor context.
- **Recovery:** Owner unavailable, revoked authority, duplicate retry, stale outcome and canceled settlement
  keep successful recipients untouched and unresolved outcomes honest.
- **Screens/modes:** MailQueue, owner delivery detail and BasicComms/Reports handoff; no real outbound send,
  TUI/Standard forms.

### AI-16 — Prepare local voice endpoints and outbound policy

- **Actor/start:** Voice administrator and permitted/denied call users; local SIP/media simulator, disposable
  numbers/devices and no emergency dialing.
- **Objective/steps:** Voice/Status → Trunks configure → Numbers route → CallerIds grant → Fraud policy/user
  limit → Emergency inspect/configure location → Phones inspect/revoke test device → BasicComms dialer → verify
  result.
- **Expected:** Effective endpoint, caller identity, device and limits explain the offered call; configured
  emergency information is not claimed as a tested emergency service.
- **Recovery:** Unavailable media/SIP, invalid number, forbidden caller ID, fraud block, missing location and
  revoked device leave explicit limits.
- **Screens/modes:** Status, Trunks, Numbers, CallerIds, Fraud, Emergency, Phones; COM-12/14 user call/device
  work, TUI/Standard admin forms.

### AI-17 — Route an incoming call through a schedule, IVR and queue

- **Actor/start:** Voice administrator, two local agents and synthetic caller; separate open/closed schedule
  states.
- **Objective/steps:** Schedules define rule → RingGroups add members/voicemail/greeting → IvrMenus
  options/prompt → Queues agents/audio → Numbers route → local incoming call → inspect chosen
  destination/voicemail and queue status.
- **Expected:** Schedule, option and member/agent state explain the route; uploaded audio is the selected
  prompt; unanswered calls follow the configured fallback visibly.
- **Recovery:** No available agent, invalid route/option, missing audio, canceled delete, failed transfer and
  closed schedule are intelligible without silently dropping the caller.
- **Screens/modes:** Schedules, RingGroups, IvrMenus, Queues, Numbers and COM-14/15; local media only, browser
  admin/TUI/Standard and captured outcomes.
- **Choice-helper branch:** Exercise a selected user/route outside the first search page through the shared
  [VoiceTransitions.xml](../runtime/components/base/basic-comms/template/VoiceTransitions.xml), then reopen the
  saved number/IVR/queue. Check labels and target kind through user, group, queue, menu and voicemail choices;
  a late lookup must not change the selected routing target.

### AI-18 — Diagnose and retry a chat bridge

- **Actor/start:** Bridge administrator and two room users; local bridge counterpart with one failed delivery.
- **Objective/steps:** System/ChatBridges inspect connection/problem → open BasicComms room bridge settings →
  send local message → inspect bridge result → disable/enable as appropriate → retry failed item → inspect room
  timeline.
- **Expected:** Bridge identity, room mapping and delivery attempt are clear; repeated retry does not duplicate
  completed delivery; room membership still controls content.
- **Recovery:** Remote unavailable, invalid mapping, permission loss, ended bridge and stale retry keep
  independent bridges usable and retain failure evidence.
- **Screens/modes:** ChatBridges and COM-08 room settings, browser/TUI/Standard admin controls and local
  transport only.

### AI-19 — Pair an edge node and inspect every setup step

- **Actor/start:** Administrator of two disposable local runtimes; single-use pairing code and isolated
  configuration.
- **Objective/steps:** EdgeNode/Pair EdgeUrl → check → Connect → Discover select services → Apply
  inspect/confirm → Test → Status; follow resulting System certificate/protocol requirements and return.
- **Expected:** Each step states what is discovered versus changed; pairing identity, configuration changes and
  tests agree; unverified external DNS/certificate requirements remain explicit.
- **Recovery:** Invalid/reused code, wrong server, failed discovery, canceled apply, partial configuration and
  failed test preserve a recoverable state without double pairing.
- **Screens/modes:** All Pair wizard steps, Status and SYS-19/AI-14 handoffs; browser/TUI/Standard forms, no
  public edge service.

### AI-20 — Inspect edge routes, queues and member state

- **Actor/start:** Paired local runtimes from AI-19 with synthetic queued delivery/member conflict;
  administrator and reader.
- **Objective/steps:** EdgeClients/Clients → inspect/ping/revoke disposable client/member → Domains/Ports
  configure local route → Balancing/EdgeCluster inspect → Status pause/resume/resync or drain as appropriate →
  verify delivery/route.
- **Expected:** Domain, client, member and queue cursor are distinguishable; route/conflict and effective
  balance settings explain destination; draining is not offline.
- **Recovery:** Duplicate domain/port, unreachable member, queue cap, failed delivery, stale takeover and
  canceled revoke retain safe routes and name each unresolved item.
- **Screens/modes:** Clients, Domains, Ports, Balancing, EdgeCluster, EdgeNode/Status; local topology only,
  keyboard/TUI/Standard tables.

### AI-21 — Synchronize a bounded set of fixture records

- **Actor/start:** Sync operator and reader; two isolated runtimes with dedicated framework fixture entity
  rows/artifact selection.
- **Objective/steps:** EntitySyncList → create/detail → select artifacts → inspect included entities → execute
  via configured job/transport → History → correct a failed fixture → eligible reset/retry → compare records.
- **Expected:** Selection, run state and history identify what transferred; a reset is not presented as
  completed synchronization; independent failures are named.
- **Recovery:** Unavailable peer, invalid selection, duplicate attempt, denied reset and partial run retain
  history and protect already-completed items.
- **Screens/modes:** EntitySync/List/Detail/History, job/transport handoff and actual fixture readback; no full
  database resync.

### AI-22 — Reconcile integration access and remaining modes

- **Actor/start:** Administrator, narrow operators, run owner/audit reader and outsider with valid records from
  earlier waves.
- **Objective/steps:** Inspect every remaining route/helper; pair selected direct payload, credential-edit, run
  and configuration refusals with holder success; follow configuration → consumer → diagnosis → Back.
- **Expected:** All inventory entries have explicit coverage; aliases retain context and privileges;
  credentials/private payloads stay out of restricted responses and downloaded evidence.
- **Recovery:** Missing providers, binary features, external media/DNS or authorization remain blocked/partial
  branches with prerequisites, never inferred success.
- **Screens/modes:** All owned subtrees, BasicComms mounts and Llm/Traces handoff; no duplicate consumer or
  Monitor full walks.

## 5. Existing checks and future verification

This is an available-test map, not a results ledger. Read assertions and suite setup before selecting a run; static
shape checks, offline component fixtures, live browser walks and service rules provide different evidence.

| Component / suite | Existing specs | Coverage and limits |
| --- | --- | --- |
| tools / server | [Constitution](../runtime/components/base/tools/test/Constitution.spec.js) | Global permission/stale hash, customize/edit/reset and trusted-origin rules; only limited rendered Rules checks. |
| framework / server | [AgentEvals](../runtime/components/base/framework/test/AgentEvals.spec.js) | Versioned dataset, service-target scoring, human review and experiment permission rules; not full visual authoring. |
| framework / server | [AdminConnectionServices](../runtime/components/base/framework/test/AdminConnectionServices.spec.js), [AdminRecovery](../runtime/components/base/framework/test/AdminRecovery.spec.js), [AdminProtocols](../runtime/components/base/framework/test/AdminProtocols.spec.js), [AdminMailOutcomes](../runtime/components/base/framework/test/AdminMailOutcomes.spec.js) | Registered connection/recovery bounds, protocol readiness/confirmed checks, per-recipient retry/unknown settlement; service evidence. |
| framework / server | [Voice](../runtime/components/base/framework/test/Voice.spec.js), [SipDevice](../runtime/components/base/framework/test/SipDevice.spec.js), [Voicemail](../runtime/components/base/framework/test/Voicemail.spec.js), [EdgeVoice](../runtime/components/base/framework/test/EdgeVoice.spec.js), [EdgeScreens](../runtime/components/base/framework/test/EdgeScreens.spec.js) | Voice/device/voicemail/edge behavior and selected screens; not every routing/provider UI path. |
| basic-comms / server | [BasicCommsBridges](../runtime/components/base/basic-comms/test/BasicCommsBridges.spec.js), [BasicCommsCalls](../runtime/components/base/basic-comms/test/BasicCommsCalls.spec.js) | Consumer-side bridge/call behavior; System admin screens need their own interaction coverage. |
| tools / acceptance client | [Monitoring](../runtime/components/base/tools/test/acceptance/client/Monitoring.spec.js) | AI trace display and diagnostic handoff portions only; no full LLM configuration/approval/eval walk. |

Future acceptance candidates: **SystemAiConfigurationWalk** (AI-01–07/11), **SystemAiOperationsWalk** (AI-08–10),
**SystemIntegrationRecoveryWalk** (AI-12–15), **SystemVoiceBridgeWalk** (AI-16–18), **SystemEdgePairWalk**
(AI-19–20) and **SystemEntitySyncWalk** (AI-21), with selected access branches in each. Place voice/bridge screen
checks with their owning basic-comms component; System's tools inventory retains their mounted routes. Reuse the
single System acceptance sweep, never a new regression sweep for each subsection.

Follow the [verification policy](ui-review-shared-gallery.md#verification-policy-for-later-execution). Proposed
walk names above describe future acceptance work, not tests created or authorized to run by this document. Keep
focused bug/rule checks in regression and reuse existing sweeps/walks instead of adding duplicates.

## 6. Coverage, findings and running record

### Activity coverage ledger

Statuses: **pending**, **inspected**, **exercised** (pass/fail/partial named separately), **blocked**, **deferred**
(with reason). Existing tests and the archived review do not advance these statuses. Every branch is pending until
execution records it; “exercised” never implies that all modes and recovery paths passed.

| Activity | Status | Outcome / remaining branches | Evidence / findings |
| --- | --- | --- | --- |
| AI-01 — Orient to AI health and outstanding work | exercised — partial | Earlier walk. Seven-day dashboard showed no calls, no failures and no waiting runs on the fresh fixture. Failed-run drill and disabled-provider comparison pending. | AI-B05 (current). AI-B01; 01-dashboard |
| AI-02 — Configure and test a model binding | exercised — partial | Test on the default model returned the local fixture reply in 5 ms. The Calls list recorded its HTTP 200 result. Binding changes, probe failure and key rotation pending. | AI-B01; 02-model-test; 03-calls |
| AI-03 — Trace a conversation through its run and provider call | exercised — partial | Provider test call 100000 returned HTTP 200. After a drawer conversation, Runs showed completed run 100000 and ConversationDetail showed active conversation 100000 with two messages. The synthetic failed run had its own ID. The provider test call had a different cause; a single joined provider-call/trace and a restricted-content walk remain open. | AI-B01; 03-calls, 03-conversation-detail, 03-runs-final |
| AI-04 — Inspect the lifecycle of a tool approval | exercised — partial | In Assist, rejected the first round_review_ack proposal with feedback and approved a fresh proposal. The drawer finished the latter in two steps. System audit listed run 100051 Rejected, run 100052 Executed and synthetic RR_AI_EXPIRED Expired. The tool called a local no-op. Changed arguments, duplicate responses and permission loss remain open. | AI-B01; 04-expired-detail, 04-live-approval.jpg, 04-audit-final |
| AI-05 — Maintain a skill and evaluate retrieval | exercised — partial | Invalid description remained available for correction. Published skill 100051 and found round-review-stationery first for “blue ledger pencil”, score 64. Actual use/run history, retention after revision and peer access remain open. | AI-B01; 05-skills, 05-retrieval-result |
| AI-06 — Connect a remote agent or MCP server locally | exercised — partial | New Remote Agent refused both local card URLs. Continued from explicitly allowed-private fixture rows; Read Card and MCP discovery reached the local peers. MCP listed review_ack with Allowed N and an Allow action. Delegation, an allowed invocation and credential-change recovery remain open. Both creation forms accepted a local endpoint with private networking enabled. | AI-B01; 06-create-refusal, 06-card-result, 06-mcp-result; AI-UI-001; AI-B02 |
| AI-07 — Explain effective tools and prompt policy | exercised — partial | Earlier walk. Opened Assist tool policy and drawer prompt preview for the default model. Rule precedence under distinct actors and effective call behavior pending. | AI-B05 (current). AI-B01; 07-policies, 07-prompt |
| AI-08 — Follow a trigger, its breaker and a proposed lesson | exercised — partial | Created round-schedule and inspected Fire Now and run history. Opened the separate synthetic paused trigger RR_AI_TRIGGER with its open breaker, three failures and Reset Breaker action. Lessons had no proposed rows. End-to-end firing, breaker recovery and lesson approval remain open. | AI-B01; 08-triggers, 08-lessons, 08-trigger-created, 08-breaker-detail |
| AI-09 — Build and compare a bounded evaluation | exercised — partial | Created version 1 of round-review-eval with item 100000, attached Matches expected and ran the local service experiment. Result showed Completed, 1/1 done, mean 0.00, 0% passed, zero cost, the raw output and the reason it differed from the expected answer. Second-version comparison and cancellation remain open. | AI-B01; 09-eval-created, 09-eval-version, 09-experiment-result |
| AI-10 — Operate a gateway client and explain usage | exercised — partial | New Client retained the synthetic client ID, description and alias when submitted without a credential and explained which user-owned credential was required. The alias and Usage lists were inspected; Usage showed no rows in its date window. Fixture issuance was declined by automatic approval review because it would create a persistent AI-scoped credential. Authenticated usage, limits and revocation remain pending. | AI-B01; 10-gateway, 10-credential-refusal, 10-alias, 10-usage |
| AI-11 — Customize and verify the Constitution | exercised — partial | Viewed Rules and Agent Prompt. Customized the disposable instance Constitution to “November review constitution” (hash 2adc09184cdf), reopened the rule, then reset to the shipped Constitution and verified original hash 15b20fedbfb1. Organization overrides, malformed JSON and running-agent adoption remain open. | AI-B01; 11-preview, 11-custom-rule, 11-restored |
| AI-12 — Configure a registered connection and inspect its contract | exercised — partial | Compared seven registered connections and four message types. Created RR_UI_REMOTE, reopened its detail and verified “Disabled until a test passes”. Six textboxes and one combobox exposed the accessible name “Value”. Successful transport, credential-change recovery and refused edits remain open. Captured the native connection editor’s accessible field names in Chrome. | AI-B01; 12-connections, 12-types, 12-start-connection, 12-field-labels; AI-UI-003; AI-B02 |
| AI-13 — Recover a message without repeating completed work | exercised — partial | Found failed incoming RR_AI_MSG_FAILED. Following restart, owner recovery showed the result as Consumed; persisted readback confirmed both this row and the independent already-completed RR_AI_MSG_DONE were SmsgConsumed. Bulk preview, selective retry of a mixed batch and payload download remain open. | AI-B01; 13-failed, 13-message-detail, 13-owner-retry; fixture-outcomes.txt |
| AI-14 — Understand protocol readiness and configure a relay | exercised — partial | Protocols reported four ready and WOPI unavailable in this build. The loopback SMTP relay test passed; the sink received the companion retry message. Advertised-address checks and remote trust cases remain open. Captured distinct SMTP setting names in the Mail Relay accessibility tree. | AI-B01; 14-protocols, 14-tested-relay, 14-relay-test-outcome; AI-15; AI-B02 |
| AI-15 — Resolve failed and uncertain mail outcomes | exercised — partial | Compared RR_AI_MAIL_RETRY (“Not sent”, Send again) and RR_AI_MAIL_UNSURE (“May have been sent”, Settle). Retrying only the former produced one receipt at the loopback sink; persisted status ES_SENT. The uncertain row closed as ES_CANCELLED with its uncertain outcome and no delivery settlement. Human-reported delivery and refused ownership remain open. | AI-B01; 15-queue-failures, 15-after-retry, 15-after-settle; fixture-outcomes.txt |
| AI-16 — Prepare local voice endpoints and outbound policy | exercised — partial | Voice status explained the absent media server. Created Review local SIP at 127.0.0.1:18772 with registration off. The stored trunk showed TLS/SRTP and “Not checked yet”. Phone registration, media exchange, a real local call and outbound policy remain open. | AI-B01; 16-voice, 16-trunks, 16-trunk-save |
| AI-17 — Route an incoming call through a schedule, IVR and queue | exercised — partial | Created a local schedule and verified Monday 09:00–17:00 with note “Review office hours”. Queues showed their empty setup state. No call exercised time routing, IVR or queue behavior. | AI-B01; 17-queues, 17-schedule-created, 17-weekly-hours |
| AI-18 — Diagnose and retry a chat bridge | exercised — partial | Opened November offline bridge with one mapped room and one failed delivery. Send again changed the same item to waiting; stored state CdsPending, attempts 0. The screen named the missing ui-review-offline adapter. Actual remote delivery and remote deduplication require a connected local adapter. | AI-B01; 18-bridge-failed, 18-bridge-after-retry; fixture-outcomes.txt |
| AI-19 — Pair an edge node and inspect every setup step | exercised — blocked at certificate verification | Entered the local front_edge peer at localhost:18771 in Pair. Next returned UnknownIssuer and preserved the address. The production client requires a WebPKI-trusted certificate; the disposable peer has a private certificate. Connect, Discover, Apply and paired Test/Status remain blocked. | AI-B01; 19-pair, 19-tls-refusal |
| AI-20 — Inspect edge routes, queues and member state | exercised — partial | Earlier walk. On the local node Edge Clients explained its role and linked to Edge Node. Opening the actual front_edge peer via HTTP redirected the browser to its untrusted certificate (ERR_CERT_AUTHORITY_INVALID). Its authenticated client/status and pairing-specific routing remained blocked. | AI-B05 (current). AI-B01; 20-clients |
| AI-21 — Synchronize a bounded set of fixture records | exercised — failed | The original run created 772 History entries. On the recheck a controlled 302 produced one failed entry and zero copies. Correction delivered exactly one selected row. A separate 60-second window skipped a row and the Detail form rejected an interval update. | AI-B01; 21-sync-detail, 21-sync-history; AI-UI-002; AI-B02 |
| AI-22 — Reconcile integration access and remaining modes | exercised — partial | View-only Model List refused with the AgentAdmin permission message. Standard Datasets showed the created v1 fixture. In a real browser-hosted Apps TUI, g opened /apps/system/Llm/Evals/Datasets and showed Review local case. Also sampled the dark phone dashboard and 1280px prompt preview. Other identities, mode-specific actions and helper authorization remain open. | AI-B01; 01-dashboard-dark-phone, 07-prompt-dark-1280, 22-standard-eval, 22-view-only, 22-tui-eval |

### Findings register

Use `AI-UI-001` onward. P1 blocks work or essential understanding; P2 causes substantial friction or misleading
presentation; P3 is a localized refinement. Label evidence as observed behavior, static suspicion or design judgment.

| ID / severity | Actor, activity step, route, mode / viewport | Reproduction and impact | Evidence / owning layer | State / correction / retest |
| --- | --- | --- | --- | --- |
| AI-UI-001 / P2 | AI-06, New Remote Agent on Llm/Remote/AgentList, Material; administrator | Open New Remote Agent and enter a loopback card URL. Create refuses HTTP because the row must allow a private network. An HTTPS loopback URL also refuses because the address is internal. The modal exposes ID, Name, Card URL and Allowed Agents but omits the private-network choice available on the detail screen. An administrator cannot create this local card binding directly with its intended URL. | Observed, `06-create-refusal.jpg` and `06-create-refusal.txt`. System Remote Agent create dialog. | **Fixed and checked 2026-11-25.** New Remote Agent and New MCP Server offer Allow Private Network, as the detail screens do. ConnectionFieldNames checks the agent dialog. Live: a create with `http://127.0.0.1:18799/.well-known/a2a-discovery.json` and Y opened the new agent's detail. |
| AI-UI-002 / P1 | AI-21, `RR_SYNC_ONE` History, Material, administrator | One run of the named single-record sync received HTTP 302 on its local peer's put route. Without another submission, worker logs recorded repeated puts roughly 20–30 ms apart, and History displayed 772 entries. The runner had to be stopped. The receiving Example count was zero. An unsuccessful endpoint can consume workers and fill History continuously, obscuring individual attempts. | Observed in `21-sync-history.jpg` and `21-sync-history.txt`, worker logs and independent receiver readback. Owning service: [EntitySync run scheduling and history](../crates/moqui-core/src/service/system_sync/entity_sync.rs). | **Fixed and checked 2026-11-25.** A run queued its follow-up after any finish older than the delay buffer, so a failed run, which keeps the last success time, queued another at once. Now only a successful run queues one; a failed run waits for its next schedule. A redirect is reported with its target and the address to correct. The system_sync integration test serves 302 to every put. With the fix there is one request and one History row naming the redirect; without it there were 121 requests in 1.5 s. The original 302's cause was not reproduced; the likely one is the http-to-https redirect of a peer with https on. |
| AI-UI-003 / P3 | AI-12, RR_UI_REMOTE on Integrations/ConnectionDetail, Material, administrator | Open the connection detail and navigate its configuration form by accessible field names. Six distinct textboxes and a combobox all expose “Value”. The form does not identify address, username, signature header, use or reason to someone moving among those controls by name. | Observed in `12-field-labels.txt` and `12-field-labels.jpg`. The mail relay shows the same visible field pattern. Inspect the shared configuration form before applying a correction to both consumers. | **Fixed and checked 2026-11-25.** The shared `NativeConnection` form keeps Value as the column header and names each control by its setting, for example "Address (endpoint)", "Password or token" and "Reason". This covers both ConnectionDetail and MailRelayDetail. ConnectionFieldNames checks the controls' names in the qvt render. No screen-reader walk. |
| AI-UI-004 / P1 | AI-21, one selected Example, 60-second splits | History marked a run Complete with zero stored and advanced the success time past a selected row. Its source timestamp was inside the reported interval and the receiver stayed empty. The first reported end also lay after the eligible cutoff. Later runs skipped the row. | sync-history-first.json and sync-window-loss.json. In entity_sync::get_data, the returned upper bound is split_thru after the loop advances it, or the first upper bound when no interval was eligible. | **Fixed 2026-11-25.** `get#EntitySyncData` answered the end of the split after the last one it read (as the Java did), and the run stored that as the last success time, so a row in that next split was never sent. It now answers the end of the last split read, or the last success time when no whole split is past the delay buffer. A run waits for a whole split past the buffer, writing no History row but a message saying when the next split is ready, and queues a follow-up only when it moved the success time and another whole split is ready. Tests: `entity_sync_data_answers_the_end_of_the_last_split_it_read` (no split ready, a row at either end of a split sent once and one at the last success time not, the threshold skipping no split, a first run ending at the buffer), `an_entity_sync_run_follows_itself_only_when_it_moved_and_a_split_is_due` and `an_entity_sync_run_waits_for_a_whole_split_past_the_buffer` in moqui-core's system_sync tests. Upgrade notes: a sync that ran under the old code lost one split per run, and those rows are not resent; moving its lastSuccessfulSyncTime back resends them safely, since the store is an upsert. In a rolling upgrade, upgrade the pulling instances first: an old puller against a new source can queue follow-ups for up to one split. Not rechecked live. |
| AI-UI-005 / P2 | AI-21, John Doe, Material Entity Sync Detail | Update on the fixture reported “primary key entitySyncId is required on moqui.entity.sync.EntitySync.” Reopening showed the original 60,000 ms split and 5,000 ms delay. The operator could not repair the configuration here. | sync-set-window.json and sync-window.json. EntitySyncDetail.xml, EntitySyncForm and updateEntitySync. | **Fixed 2026-11-25.** The SEC-TLS-116 change wrapped the transition's service-call in actions, where a service-call passes no parameters without an in-map, so the update had no entitySyncId. It now has `in-map="context"`; an empty password field is removed from the context first, so the stored password stays. The same defect broke Monitor › Recovery's storePolicy (it made a blank policy) and storeAdapter (adapterId required), fixed the same way. Tests: the tools spec EntitySyncDetail posts the Update twice (the stored split and delay, an empty password keeping the stored one, an entered one replacing it), and AdminOverview's recovery feature (an edit of each stored, no other policy made). Not rechecked live. |
| AI-UI-006 / P2 | AI-21, John Doe, Standard Entity Sync Detail | A one-field edit resubmitted the unchanged split as `60,000`. The update failed and the old delay reopened. Entering plain digits into both fields saved successfully. | live.json, detail-numeric.json and numeric-error.txt. Shared Standard numeric form submission, observed on EntitySyncForm. | **Fixed 2026-11-26.** The display is as Java's (a numeric text-line shows its number grouped), but the update is the implicit `update#` service, which declares no parameters, and the entity write converted text with a plain parse, so `60,000` stayed text. Java's `FieldInfo.convertFromString` reads number text with `ec.l10n.parseNumber`; the entity layer's conversion (`hooks::basic_convert`) now reads number text that does not parse plainly in the user's locale, for every numeric field a form posts to an implicit create or update. Tests: moqui-entity `grouped_number_text_is_the_number_on_a_create_and_an_update` (fails on the old conversion) and the tools spec EntitySyncDetail's one-field edit (split and threshold posted grouped). Live pass in AI-B04. |

### Bounded batch record

For each later batch record:

- Intent, activities and branch limits; runtime/build/components, identities/grants, fixture IDs and dates.
- Observed steps and outcomes; evidence filenames, console/network/output observations and findings.
- Workarounds and blocked/deferred branches, with prerequisites and follow-up.
- In refinement only: decisions, owning files/components, correction, focused verification commands/results and retest.
- Temporary settings/grants/files/jobs restored, remaining cleanup and companion handoffs.

### AI-B01 — First browser review, 2026-11-25

The coordinator walked the System app using the built-in browser on the disposable November 25 runtime, release binary from checkout `6d8bb49f8a7c`. The ledger cites the matching file stems under `plans/ui-refinement-evidence/system-ai-integrations/batch-01/`. An inspected list, navigation tab or empty state covers only that entry point. Browser evidence, fixture setup and remaining conditions are distinguished below.

The primary Material walks used 1920×1080. The dark dashboard at 390px and prompt preview at 1280px covered narrower layouts. A view-only user on a separate hostname received the model-administrator refusal while the administrative actor could reach ModelList. Both Standard and the browser-hosted Apps TUI displayed the created evaluation fixture. The broader role matrix, remaining keyboard workflows and TUI actions remain pending.

The local model returned “Local review fixture response”; provider call 100000 recorded HTTP 200. Retrieval selected newly published skill 100051 for its distinguishing query. The remote discovery fixtures use local A2A and MCP endpoints, a single read-only acknowledgement tool and individually allowed-private rows `review-peer` and `review-mcp`. A dedicated front_edge peer uses a separate embedded database. Entity Sync fixture `RR_SYNC_ONE` selects only Example `RR_SYNC_SAMPLE` with dependents excluded. Its History showed 772 entries after the one failed invocation. A read-only check of the peer database found zero matching records.

**Cleanup.** The A2A/MCP loopback listener and the separate front_edge peer were stopped. All three ports had no listeners and the peer database PID file was gone; see `peer-cleanup.txt`. The coordinator owns the primary runtime and remaining fixtures.

**Ownership.** The System AI/integrations register owns the System-mounted Voice administration, Chat Bridges retry/status, protocol readiness and mail-queue recovery. [BasicComms](ui-review-basic-comms.md) owns the mail and chat composer, participant views, room membership and end-user calls. One loopback receipt supports the queue result; it does not establish a second compose or room-delivery result.

### AI-B02 — November 25 continuation

John Doe created closeout-agent and closeout-mcp in Standard with Allow Private Network set to Y. Each opened its detail, and Material listed the persisted local URL and Y. A separate submission without the flag received the private-network validation message. Chrome’s accessibility tree named Address (endpoint), User (endpoint), Password or token and Reason on Connection Detail, and SMTP host (endpoint), Port (endpoint), Password and Separate SMTP password on Mail Relay. See `agent-catalog.json`, `mcp-catalog.json`, `connection-names.json` and `relay-editor-names.json`.

The single-record fixture used independent source and receiver databases. With a completed one-second interval, one submission made one POST to a local redirect. One History entry named the 302 and its destination; independent Grid reads found zero receiver copies. Correcting the target via the granted setup API and retrying produced one EsComplete entry with recordsStored 1 and exactly one receiving RR_SYNC_SAMPLE. AI-UI-002 passed for bounded failure and recovery. The historical redirect source remains unknown. See `sync-redirect-recheck.json` and `sync-corrected-receipt.json`. The initial 60-second interval and the Detail update are recorded as AI-UI-004 and AI-UI-005. Both are fixed since, not yet rechecked live.

The newly created entries fetched the local agent card and discovered the MCP tool review_ack with Allowed N. See a2a-discovery.json and mcp-discovery.json. The existing service scenarios separately verified known-failed mail retry, uncertain delivery settlement, voice routing and owner-scoped recovery (supporting-checks.txt).

Local evidence is under `plans/ui-refinement-evidence/closeout-d10e22f/`.

#### Remaining branches

The ledger records the strongest observed outcome. The remaining steps, recovery paths and modes in each section 4 activity are deferred to the fixture and next check below. An explicitly blocked or failed result keeps that status.

| Activity | Required setup and next observation |
|---|---|
| AI-01 | Induce an attributable provider outage and trace the health card back to the failing operation. |
| AI-02 | Use a local responding model binding and repeat its test after invalidating its credential. |
| AI-03 | Correlate a local model call with the exact conversation and trace as a scoped reader. |
| AI-04 | Use distinct requester and approver accounts to refuse and approve a harmless proposed action. |
| AI-05 | Share a skill for review and compare retrieval for eligible and ineligible users. |
| AI-06 | Allow and invoke the discovered local tool, rotate its credential and compare the denied request. |
| AI-07 | Apply a scoped tool policy and compare the tool presented to the same agent before and after. |
| AI-08 | Trip the fixture trigger's breaker, inspect its proposed lesson and follow its reset. |
| AI-09 | Compare two bounded evaluation runs over the same deterministic model fixture. |
| AI-10 | Use a scoped gateway client and reconcile one call with its usage record after a limit or revocation. |
| AI-11 | Change and restore a disposable Constitution rule and inspect its use in a real run. |
| AI-12 | Validate a registered local connection and inspect its contract and audit trail as a second actor. |
| AI-13 | With one previously completed item, recover the failed message and verify recipient deduplication. |
| AI-14 | Send through the configured local relay and inspect reception after credential correction. |
| AI-15 | Inspect captured delivery for an eligible retry and prove an uncertain item stays settled without resending. |
| AI-16 | Register local SIP endpoints and verify caller identity under the configured outbound policy. |
| AI-17 | Route a synthetic incoming call through a schedule and IVR to the actual queue recipient. |
| AI-18 | Disconnect a local bridge, retry its queued message and compare both sides by message ID. |
| AI-19 | Pair a disposable edge node with a locally trusted certificate, then repeat after trust reversal. |
| AI-20 | On the paired edge, change a fixture route and inspect its queue and actual member state. |
| AI-21 | After repair, repeat the 60-second boundary with a row near the cutoff and save the interval from Detail. |
| AI-22 | Exercise the same granted integration action in its alternate mode and as a restricted account. |

### AI-B03 · November 26 change-led review

**AI-21, independent source and receiver.** FRESH_NEXT was stamped at 1795726477645. The first completed 60-second split ended at 1795726477644 with zero stored. The receiver was empty. The next completed split started at that exact boundary and stored one matching record in the independent database. This verifies AI-UI-004. The original Detail transition also passed its primary-key and password-retention checks. See sync-consecutive.json and rules.txt.

**AI-UI-006 · P2 · Fixed 2026-11-26 (see the finding row).** In Standard Entity Sync Detail, an administrator changed only Delay Buffer from 5,000 to 6000 and submitted Update. The untouched split field submitted its displayed value `60,000`. PostgreSQL rejected that numeric value during commit. Reopening showed the old delay. Repeating with split `60000` and delay `6000` persisted the new delay, shown as 6,000. This is a separate failure after the AI-UI-005 primary-key fix. The owning surface is the Standard numeric edit/form submission path used by EntitySyncForm. After repair, repeat the one-field edit with the other numeric field untouched and read both stored values. On FRESH_NEXT, the same check targeting 6100 reopened 5000 after the failed submission. The successful control reopened 6100, and an independent Grid read returned split 60000 and delay 6100. See live.json, detail-numeric.json and numeric-error.txt.

## 7. Decisions

- IDs are `AI` across this combined integration file; waves keep the large inventory reviewable without another
  document split. Monitor owns traces, System owns certificates/web protection and BasicComms owns user operation.
- List/detail/helper routes are reviewed through their actual default children. Approval audit views do not gain
  mutation controls merely because the review needs an approval outcome.
- External prerequisites remain explicit blocked branches; no paid provider, real call, public pairing or external
  message is authorized by this plan.

- Preparing or auditing these plans changes documentation only. Application/API changes, test runs, runtime walks
  and fixture creation belong to separate review/refinement execution.
- Current code, mounts and grants take precedence over older plans. Historical outcomes, permissions and exceptions
  are not inherited. Preserve routes, stored preferences and public contracts in any later refinement.
- Finish a review with explicit branch outcomes and a candid unresolved ledger; missing prerequisites do not become
  passes. Building absent features is outside this review.

### AI-B04 · Standard one-field update

On November 26, john.doe opened Standard Entity Sync Detail for UI_RECHECK on fa75eceb6. Split, threshold and delay displayed 60,000, 1,500 and 5,000. Changing only delay to 6100 and submitting Update reopened with 60,000, 1,500 and 6,100. Tools Grid independently returned the stored integers 60000, 1500 and 6100. AI-UI-006 passed. Evidence in `inspection-fa75/fixes.json`.

### AI-B05 · November 26 inspection follow-up

Build `fa75eceb6`. These entries update the activity ledger above. The earlier batches retain the original observations. Local receipts are under `plans/ui-refinement-evidence/inspection-fa75/`.

| Activity | Current evidence | Remaining observation | Receipt |
|---|---|---|---|
| AI-01 | A registered local model endpoint answered 503 on three attempts. AI Dashboard listed the failing ui-fa75-model calls. Its failed-call link opened Detail with the overloaded status and the controlled provider message. | Disable and re-enable the model, then compare the card and recover with a successful call. | `finalwalk.json` |
| AI-07 | Stored assist/write_ui with approval AgamNever. Grid verified the policy. Prompt Preview still listed write_ui before, during and after the comparison. The native policy fixtures separately checked available catalog results and rechecked policy on an approved write. | Run the same assist tool and actor with the saved rule and after removal. The preview alone does not establish its invocation outcome. | `additional.json`, `services.txt`, `policy.txt` |
| AI-20 | The Edge screens fixture changed a host route, tested refusal and exercised member controls. Separate live two-pairing and two-member fixtures demonstrated routed work and session movement when a member stopped. | Change the same route on the live pair and inspect its queue and member state through the screen with the request in flight. | `services.txt`, `consumers.txt` |

### AI-B06 · November 27 local model and service review

On `23204bf61`, John created c-pass-endpoint and c-pass-model from the Standard Models screens and ran Test against the loopback provider. Its correct key returned a fixed acknowledgement. Changing the endpoint key returned HTTP 401 with the provider's reason on Model Detail. Restoring the key returned HTTP 200. The Calls list showed both errors and the AI Dashboard reported six calls, two failed. The model answered from the isolated receiver on all six attempts. This exercises the credential branch of AI-02 and the provider-failure card for AI-01. [Model actions](ui-refinement-evidence/pass-2320/C/model.json) · [Provider requests](ui-refinement-evidence/pass-2320/C/model-peer.txt)

The service and screen fixtures exercised skill visibility and audited reads, scoped tool policies, evaluated outputs and scores, guarded connection changes, mail retry and settlement, edge route controls and entity-sync Detail. The first run had 190 passes and one SMTP-off expectation under the deliberately enabled review listener. With the SMTP-default configuration the isolated protocol spec passed all four features. [Feature outcomes](ui-refinement-evidence/pass-2320/C/services.txt) · [Protocol defaults](ui-refinement-evidence/pass-2320/C/protocol-default.txt)

John shared a personal skill for review and drove a local MCP acknowledgement through an actual Assist run. Calls Client B lacked AgentUse. John’s rejected and approved proposals recorded separate decisions. Repeating either returned 409. The same tool returned not_allowed with the Never policy and reached the peer after removal. The first two bounded experiment probes selected the service runner itself as their target. Submitting the Experiments form with the job-capable agent completed both one-item runs and enabled their comparison. The controlled model received the temporary Constitution rule and the marker disappeared after the shipped hash returned. [MCP peer](ui-refinement-evidence/pass-2320/C/mcp-peer.txt) · [Owner and restricted decisions](ui-refinement-evidence/pass-2320/C/decisions.json) · [Model prompt observation](ui-refinement-evidence/pass-2320/C/model-prompt.txt) · [Corrected trigger](ui-refinement-evidence/pass-2320/C/eval.json) · [Experiment comparison](ui-refinement-evidence/pass-2320/C/comparison.json)

The scoped gateway called the alias using its own disposable credential. Client Detail reported four input and four output tokens. Disabling the client returned 403 and revoking its app password returned 401. The real TUI model Test succeeded for John and Detail returned 403 to Calls Client B. A separate synthetic bridge receiver returned 403 on its first message and accepted the same source GUID on retry, producing one remote ID. [Gateway](ui-refinement-evidence/pass-2320/C/gateway.json) · [Terminal](ui-refinement-evidence/pass-2320/C/terminal-model.txt) · [Bridge receiver](ui-refinement-evidence/pass-2320/C/bridge-peer.txt) · [SIP receiver](ui-refinement-evidence/pass-2320/C/sip-peer.txt)

#### November 27 branch dispositions

The pinned review used synthetic actors and isolated receivers. The table records the observation that changed each activity’s disposition and its next independent check. The original activity protocols and dated batch rows retain the other pending branches, including presentation modes. Whole-activity status includes those branches. [Starting ledger and full activity protocols](ui-refinement-evidence/pass-2320/starting-queue.json)

| ID | Status | Current result | Remaining prerequisite and observation |
|---|---|---|---|
| AI-01 | partial | AI Dashboard counted two failed calls and Models showed the provider’s 401, then a successful call after restoring the key. [Receipt](ui-refinement-evidence/pass-2320/C/model.json) | Disable the model in its management screen, reread the health card, enable it and follow a successful recovery into the originating run. |
| AI-02 | partial | Endpoint Test followed the changed credential at the provider from 200 to 401 to 200. The registered model also answered through the gateway and terminal. [Receipt](ui-refinement-evidence/pass-2320/C/model.json) | With a scoped model editor, override a probed capability, provoke a stale form and cancel a delete, then inspect the effective capability in Prompt Preview. |
| AI-03 | partial | The model call, MCP tool event and conversation shared the owner’s run identity. B was refused the admin Conversation Detail. [Receipt](ui-refinement-evidence/pass-2320/C/remote.json) | Use an AgentAudit holder and a retained Monitor trace for that same run. Follow Calls and Trace through the return path, then remove the audit grant and reread. |
| AI-04 | partial | Two proposals waited for consent. B lacked AgentUse. John rejected one, approved the second and duplicate decisions returned conflict. Both runs finished. [Receipt](ui-refinement-evidence/pass-2320/C/decisions.json) | Use a second AgentUse holder for the ownership-specific refusal and an AgentAudit holder for the durable detail. Repeat after permission loss and compare edited approval arguments. |
| AI-05 | partial | John saved personal skill 100031, matched it with score 64 and shared it for review as draft 100032. The restricted peer had no skill text. [Receipt](ui-refinement-evidence/pass-2320/C/skill.json) | Use an AgentUse peer and designated reviewer. Publish or reject the shared draft, then compare retrieval and use-history across a revision. |
| AI-06 | partial | Read the local A2A card, discovered the MCP tool, allowed it and invoked it at the peer. After rotating the key the peer refused discovery. [Receipt](ui-refinement-evidence/pass-2320/C/remote.json) | Use a local delegated A2A task and changed discovery document. Remove a tool that a session previously saw, then inspect the stale invocation and recovery. |
| AI-07 | partial | The same assist agent and actor invoked the registered MCP acknowledgement, received not_allowed with the specific Never policy and invoked it after policy removal. [Receipt](ui-refinement-evidence/pass-2320/C/agent-ops.json) | Use a scoped agent with two overlapping rules and a distinct ordinary user. Compare rule order, inherited authority and what Prompt Preview offers. |
| AI-08 | partial | Failure limit one opened the breaker when assist could not serve job. The next fire was skipped. Reset closed it and a job-capable agent subsequently completed. [Receipt](ui-refinement-evidence/pass-2320/C/agent-ops.json) | Use a trigger failure that generates a proposed lesson. Follow its review, approval and subsequent trigger, including an intentionally canceled or duplicate event. |
| AI-09 | partial | Experiments 100002 and 100003 each completed one item on the same deterministic model and dataset version. Compare showed zero worse and zero better. No scorer was attached. [Receipt](ui-refinement-evidence/pass-2320/C/comparison.json) | Attach an eligible scorer and revise one item. Compare per-version scores and cancel a running experiment, then read completed and outstanding items. |
| AI-10 | partial | A scoped AI app password reached its allowed alias, used four prompt and four completion tokens, and appeared in Client Detail. Disabling the client returned 403, revoking its key returned 401. [Receipt](ui-refinement-evidence/pass-2320/C/gateway.json) | Set a low quota and check the Usage rollup and reset window. Compare an unknown alias and upstream error against that same client’s call history. |
| AI-11 | partial | Added C14 on the disposable instance. The model received the rule’s unique marker, then lost it after Reset. The original 13-rule hash returned. [Receipt](ui-refinement-evidence/pass-2320/C/agent-ops.json) | Use an organization-scoped actor and an invalid edit. Compare inherited and local text in a running agent and verify recovery after the rejected document. |
| AI-12 | partial | Registered adapter checks exercised connection contract, controlled change, retest and scoped settings. Account-owned SMTP was separately distinguished from the admin relay. [Receipt](ui-refinement-evidence/pass-2320/C/services.txt) | Use a registered live connection with an independent reader. Change its credential in Connection Detail, test and reread contract and audit history as that second identity. |
| AI-13 | partial | The recovery fixture preserved protected payloads, refused an ineligible item, reconciled an unknown outcome and continued after a failed item in a mixed batch. [Receipt](ui-refinement-evidence/pass-2320/C/extra.txt) | Use a receiving handler that keeps a receipt keyed by message ID. Include an already-consumed row in a selective retry and compare unique receiver copies and payload download rights. |
| AI-14 | partial | The local TLS SMTP peer refused bad credentials, accepted the corrected ones and received the test message. This is the account-owned outbound transport. [Receipt](ui-refinement-evidence/pass-2320/C/external-mail.json) | Use that peer as the registered System relay and submit its own Test. Compare advertised/public protocol checks and audit against a second reader. |
| AI-15 | partial | AdminMailOutcomes captured exactly-once eligible retry and kept uncertain settlement distinct from the transport’s sent state. A missing registered owner prevented retry. [Receipt](ui-refinement-evidence/pass-2320/C/services.txt) | Use Mail Queue as a scoped owner and outsider. Record human-reported settlement, the captured retry and subsequent refused repeat on the same record. |
| AI-16 | partial | Settings showed a registered SHA-256 soft phone. The dial control refused B and placed John’s permitted call. The UDP peer saw his assigned From number and replied Busy. [Receipt](ui-refinement-evidence/pass-2320/C/phone.json) | Use a local secure trunk with an actual encrypted media session, test its unavailable route, and compare its caller identity on recovery. |
| AI-17 | partial | An open UTC schedule and IVR timeout routed the inbound call into the active queue. John answered, the peer observed SIP 200, and History named the calling number. [Receipt](ui-refinement-evidence/pass-2320/C/ivr.json) | Change the schedule to closed and test IVR invalid digit and after-hours fallback. Verify the alternate recipient or voicemail for the same dialed number. |
| AI-18 | partial | The local bridge receiver refused the first message. System Send again delivered it under the same source GUID. A repeated retry was refused after delivery. [Receipt](ui-refinement-evidence/pass-2320/C/bridge.json) | Use a second room member and a restricted operator. End a mapping with a delivery in flight and read the member’s permitted or refused payload at both ends. |
| AI-19 | blocked | The earlier localhost pair stopped at UnknownIssuer. The review had no WebPKI-trusted edge identity for its DNS name. [Receipt](ui-refinement-evidence/pass-2320/C/prerequisites.txt) | Provide a client-trusted certificate and name for the pair and a one-use pairing code. Complete Connect, Discover and Apply, then reverse trust and retry. |
| AI-20 | partial | EdgeScreens exercised domain claim, queue cap, member/device controls and permissions with the link down. The previous live fixtures retain their separate routed-work evidence. [Receipt](ui-refinement-evidence/pass-2320/C/services.txt) | On the trusted pair from AI-19, change a live route while a request is in flight and compare member, queue and downstream completion on the same request. |
| AI-21 | partial | Entity Sync Detail fixtures exercised existing-ID update and retained credentials. B03/B04 retain independent receiver split and Standard interval readbacks for the closed findings. [Receipt](ui-refinement-evidence/pass-2320/C/services.txt) | For the remaining scheduling and trust branches, use independent peer databases, simultaneous edits and disabled destinations. Compare consecutive intervals with recipient rows. |
| AI-22 | partial | John used Test in the real terminal Model Detail and got the provider acknowledgement. B’s terminal was refused the same Detail with HTTP 403. The browser showed the same model in both themes and three widths. [Receipt](ui-refinement-evidence/pass-2320/C/terminal-model.txt) | Pair a narrowly granted holder with a denied account for alternate output, secret fields, helpers and stale navigation. Verify each output from its consumer. |

All four local test databases were discarded. John restored the Constitution and model bindings, paused the trigger, removed the mailbox delegate and caller-ID grant, removed the queue assignment, and disabled the bridge and trunk. [Cleanup and database checks](ui-refinement-evidence/pass-2320/C/cleanup.json) · [Phone cleanup](ui-refinement-evidence/pass-2320/C/voice-cleanup.json)
