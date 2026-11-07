# The Constitution

The Constitution is shared guidance for the people and the agents of a Moqui instance: one localized JSON document of
rules. Every Moqui agent run gets it in its system prompt; people read it in System › Constitution, in the Assist
drawer's Rules pane, and in an organization's own application. The design is
[LLM-A2A-Assist §7.10](../../plans/LLM-A2A-Assist.md#710-constitution-proposed-1315); this page describes what is
built.

## The document

`runtime/base-component/framework/docs/Constitution.json` is the shipped default: English, Spanish and Simplified
Chinese, rules C01 to C13, 37 references. A document is one object:

| Key | What it holds |
| --- | --- |
| `schemaVersion` | `1` |
| `defaultLocale` | The language every text must have (`en` in the default) |
| `title`, `provenance`, `instructions` | Locale maps (`{"en": "…", "es": "…"}`): the title, where the guidance comes from (for readers), the shared agent instructions (how a conflict and a person's acknowledgement are handled) |
| `rules` | In order; each `id` (stable: `C01`), `title`, `description` (for people), `instructions` (for agents), and `references` |
| `references` | In order; each `id` (stable, unique in the document: `C01-R01`), `title` (a locale map) and one `url` for every language |

Keys the code does not know are kept through every edit. A text resolves field by field: the exact locale, then its
base language (`es_MX` reads `es`), then the default locale. A reference URL is `https://` (no credentials) or a path on
this server starting with `/` (not `//`, no `.` or `..` segments). Validation names each problem by its JSON path and a
reason code (`crates/moqui-llm/src/constitution/document.rs`).

## Which document applies

One whole document, never merged:

1. **An internal organization's own document**: Mantle's, one `PartyContent` of type `PcntConstitution` per
   organization, the JSON in the resource its `contentLocation` names (`dbresource://mantle/content/party/<id>/constitution/Constitution.json`
   by default). It applies to the organization's members while it is their active organization (the
   `ACTIVE_ORGANIZATION` preference, under the same rule as `get#PartyOrganizationInfo`).
2. **The instance's own document**: `runtime/conf/Constitution.json`.
3. **The shipped default.**

A source that cannot be read or is invalid is logged by source and organization and passed over, with a diagnostic
readers and editors see; a provider failure is never taken for "no organization document". When not even the shipped
default can be used, no Constitution applies and agent runs go on without the section.

The framework reads organizations through `constitution-provider` elements of the `llm-facade` conf; Mantle's
`MoquiConf.xml` names `mantle.party.ConstitutionServices.get#ScopedConstitution`, an implementation of
`org.moqui.admin.ConstitutionServices.get#ScopedConstitution` (in `userId`, `organizationPartyId`; out `scopeStatus`
none, inherit or override, the organization and `documentText`). It runs under the read fence.

## Permissions

| Change | Needs |
| --- | --- |
| The instance's document (store, customize, reset) and the trusted reference origins | The sealed `ADMIN_CONSTITUTION_GLOBAL` (framework, granted to `ADMIN`) |
| An organization's own document | `ORG_CONSTITUTION_EDIT` (Mantle, granted to `ADMIN`) for an organization the user works in, or any internal organization for a holder of `ADMIN_CONSTITUTION_GLOBAL` |

Reading needs only a signed-in user: the global document for anyone, an organization's for its members. `AgentAdmin`
grants nothing here. An application that edits (IronStone Admin) adds its own screen checks over these.

## Services

`org.moqui.admin.ConstitutionServices` (`runtime/base-component/framework/service/org/moqui/admin/`, bodies in
`crates/moqui-llm/src/constitution/services.rs`):

| Service | What it does |
| --- | --- |
| `get#Constitution` | The document that applies (or `scope` global or shipped, or `organizationPartyId`), localized for reading, with its source, hash, diagnostics and `globalHash` |
| `validate#Constitution`, `edit#ConstitutionDocument`, `preview#ConstitutionPrompt` | Pure helpers both editors use: the problems of a document; one edit (add, change, move, remove a rule or reference; a text in a locale; the default locale; remove a locale) with new ids never reused; the prompt section and its size |
| `store#Constitution`, `create#ConstitutionCustomization`, `reset#Constitution` | The instance's document, written whole (temporary file and rename), each change with the hash it was read at |
| `record#ConstitutionChange` | Keeps the before and after documents in `AgentConstitutionSnapshot` and logs actor, scope and hashes to `moqui.monitor.audit` (internal; Mantle calls it) |
| `get#ConstitutionReferences` | READ, an agent tool (`get_constitution_references`, found through `find_tools`): a rule's references, from the run's own document |
| `get#ConstitutionSnapshot`, `resolve#ConstitutionReference` | A run's document for reading; a reference for the dialog, checked at each open |
| `get#ReferenceOrigins`, `store#ReferenceOrigins` | The trusted reference origins |

`mantle.party.ConstitutionServices`: `get#OrganizationConstitution`, `store#OrganizationConstitution` and
`reset#OrganizationConstitution` for an organization's own document, with the same hash checks.

## Agent runs

A run resolves its Constitution when it starts (`crates/moqui-llm/src/constitution/snapshot.rs`): the requester's
active organization's document, or the global one for a job or a run as the agent's own user; the locale is the
requester's (a job's run-as user's), else the document's default. The document is stored once by its hash in
`AgentConstitutionSnapshot`; the run keeps `constitutionSource`, `constitutionOrgId`, `constitutionHash` and
`localeCode` through its steps, pauses and resume.

The prompt section (`render.rs`) comes after the release texts and before the agent's and the organization's
instructions: `## {title}`, the shared instructions, each rule's `{id} — {title}` and agent instructions, and a hint to
find `get#ConstitutionReferences`. It is never cut. A conversation's frozen prefix records the run key it was built for
(`AgentConversation.constitutionKey`); a run with another key (an edit, another organization, another language)
rebuilds the prefix before its first call, and the same key keeps the same bytes. System › LLM › Prompt Preview shows
the section a new run would get.

## Reading and references

- **System › Constitution** (`runtime/base-component/tools/screen/System/Constitution/`): Rules (read, Customize,
  Reset), Edit (per language: the document's texts, rules, a rule's references, the languages), Agent Prompt, JSON
  (download, check, save) and Origins. Its parts are `tools/template/ConstitutionParts.xml`.
- **The Assist drawer** has Constitution / Rules in its header menu: the rules of the active run, labeled as such,
  else the rules that apply now, with a reading language and Back to conversation (the draft and the thread's scroll
  stay). A link in an answer that is exactly one of its run's trusted references opens in the reference dialog.
- **The reference dialog** (`js/WebrootConstitution.js`: `m-constitution-view`, `m-reference-dialog`,
  `m-assist-constitution`) opens only on a click; `/assist/constitutionReference` resolves the reference again each
  time (still in the document, still readable, its origin still trusted). The page is framed with
  `sandbox="allow-scripts allow-same-origin"` (no top navigation, popups, forms or downloads), no referrer, and unloaded
  on close; focus returns to the link. A framed page that does not load cannot be told apart from one that does, so the
  dialog always offers Open in a new tab. `/assist/constitution` and `/assist/constitutionReference` need a signed-in
  user, not `AgentUse`.
- **Trusted origins** are instance policy in `runtime/conf/ConstitutionOrigins.json` (`https://grok-the-rock.org` when
  there is none): exact `https://` origins on the default port. A file that cannot be used trusts nothing. A
  Constitution reset leaves them; an organization's document cannot add one; they are not CORS. A new or changed
  reference URL of an untrusted origin is refused on save.

The app's screen pages send no `frame-src`, so the allowlist is enforced by `resolve#ConstitutionReference` and the
client rather than by the browser; adding `frame-src` would mean listing every frame the app uses. A destination's own
`frame-ancestors` or `X-Frame-Options` still decides whether it can be framed.

## Tests

- `cargo test -p moqui-llm constitution` and `prompt`: validation, locale fallback, edits, rendering, the shipped
  default's completeness and size, the hash, the origins, section order and the prefix rebuild.
- `./moq test tools --spec Constitution`: stores, the sealed permission, customize, edit and reset, origins, and
  System › Constitution › Rules in qvt and tui.
- `./moq test mr-mantle --spec Constitution`: an organization's document, its provider answer and its refusals.
- `./moq test webroot --spec WebrootAssist`: the reference dialog's logic in the page.

Not covered by regression tests: the dialog's drawing, focus and framing in a browser, and whether a model follows the
rules (an opt-in live-model evaluation).
