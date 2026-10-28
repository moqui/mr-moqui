# Dev AIDE: the assisted IDE

Oct 24, 2026 · Design. Built the same day: P1a, P1b and P2 (§18); P3 to P7 on Oct 28, 2026; what is not built yet is listed
in "Built so far" at the end of §18. The first questions were answered the same day (§20: 1, 4 and 5, and the
Projects point of 13), and D1, D7, D15 and D16 record what they decided. A review the same day, for users from
experienced coders to technical business users (who mostly extend entities and screens and let the agent do the
typing), added D17 to D19, amended D10 and D11, and re-phased the work around them (§18); its findings are recorded
where they apply. Dev AIDE (Assisted IDE) is an app of the
`/qapps` shell for changing and creating Moqui components while the server runs them: a workspace of the components'
repositories and directories with an editor, git and live reload; an artifact index with search and a browser; an
entity modeler; a service writer with a logic composer and ECA rule tools; a screen builder; a small task panel; and
the Assist agent with tools for all of it, for people and AI agents alike. It is a new component,
`runtime/dev-component/dev-aide`, that loads only on a `dev` instance, and the default build becomes a Dev package
that carries it (D16).

The tools have been talked about since early OFBiz, and small prototypes made over the years; this plan expands the
user's notes of Oct 24, 2026 into a design grounded in what mr-moqui has built. Reviewed for it: the framework's
definition caches and what follows a file change, the component discovery, the Assist drawer, canvases, approvals and
tool registry (docs/Assist.md, [LLM-A2A-Assist](LLM-A2A-Assist.md)), git hosting and revision management
(docs/Git-Hosting.md, docs/Revision-Management.md), IronStone Projects' Code (`boss/plan/design-projects.md` §7 and its
screens), the vendored client libraries, the screen and XML Actions macros, the XSDs, the IronStone style guide
(`boss/plan/style-guide.md`), and Gitea's repository view and editor (`~/work/moquiwork/3p_other/gitea`, at fc49368).

Paths: `core/` is `crates/moqui-core/src/`, `lib/` is `crates/moqui-lib/src/`, `fw/` is
`runtime/base-component/framework/`, `webroot/` is `runtime/base-component/webroot/screen/webroot/`, `boss/` is
`runtime/component/mr-boss/`, `DA/` is `runtime/dev-component/dev-aide/`, and Gitea paths are relative to its
clone. Every other path is relative to this repository. `my-site` stands for a site's own component (D17).

The plan has the decisions (0), what it looks like (1), where it starts (2), the component (3), the workspace and its
repositories (4), the editor (5), live reload (6), the artifact index and browser (7), the entity modeler (8), the
service writer (9), ECA rules (10), the screen builder (11), data files and new components (12), Assist and agents
(13), tasks (14), security (15), data and configuration (16), the framework changes (17), the phases (18), tests (19),
and the risks and open questions (20).

---

## 0. Decisions

| # | Decision | Why |
| --- | --- | --- |
| D1 | A component of its own, `runtime/dev-component/dev-aide`, kept in this repository. `runtime/dev-component` is a component directory as `runtime/component` is: it holds the framework's dev components, tracked here as base-component's are (more will join the dev-aide), and any a user adds of their own. The conf's `component-list` names it as a `component-dir` with `purpose="dev"`, the dev-aide's `component.xml` says `purpose="dev"` too, and on any other purpose each component there is left out with a WARN line naming it. | Asked for (question 4). The tools write code the server runs (D15), so they must not reach production until the sandboxing is trusted; the second mark keeps a copy in `runtime/component` out too. |
| D2 | Live artifacts first. The tools edit the files the running server loaded; every save reloads exactly what that file defines (its services, its screen, the ECA rule set of its kind, its entities relinked) and reports what it reloaded. Changes from outside (another editor, an agent in a terminal) reload the same way through a file watcher in the framework, on dev instances. | Asked for ("see updates instantly", "explicit artifact cache clears on save"). Today screens, scripts and templates follow their files, while services, entities and ECA rules need a restart (§2). |
| D3 | One source of truth per open file: its text buffer. The visual tools are views of a file tab that turn gestures into structural operations; each becomes a minimal text edit of the buffer through a lossless XML layer. Text and visual views switch at once, share one undo history and one Save, and a saved file keeps its comments, order and formatting, so its git diff is the change. | The notes ask for an easy switch between visual and text. Writing XML out of a model drops comments and reformats files, which makes every diff noise. |
| D4 | CodeMirror 6 is the editor: one vendored bundle (`webroot/libs/codemirror`, built by `refresh.mjs` as Tiptap is) behind `m-code-editor` and `text-area editor-type="code"` in the framework. Not Monaco. | XML with schema completion, JavaScript and Markdown are first class; it is modular and a fraction of Monaco's size, needs no workers or `unsafe-eval`, works on touch, takes Quasar's dark mode, and has a merge view. Gitea's web editor is CodeMirror 6 now (`web_src/js/modules/codeeditor/`). |
| D5 | Working trees through the developer's own `git` command: status, diffs, staging by hunk, commits with their hooks and signing, branches, stashes, and fetch, pull and push with their credential helpers and SSH agent. The framework's gitoxide hosting stays for hosted repositories. | It does what the developer's terminal does, with the same config, hooks, signing and credentials; gitoxide has no push client; and it works in every package, Lite too. The IDEs do the same. |
| D6 | The repository screens are Gitea's shapes merged into an IDE: a repository bar, tabs (Code, Changes, Commits, Branches, Tags, Stashes, Settings), the file tree on the left, a directory shown as Gitea shows it, a file as an editor tab. A plain directory has the same layout with the version control disabled and a "Not a git repository" chip. | Asked for: a familiar layout, and one screen for both kinds of directory. |
| D7 | IronStone Projects' Code stays separate and different: it keeps to the safe repository features, which read, or write only refs, merges, reviews and settings through the hosting services and their access checks, and never edit or run code. The two share what is underneath (the framework's services for hosted repositories; `m-code-editor`, `m-file-tree`, `m-diff-view` and the CodeMirror bundle, none of which write files) and stay independent: neither depends on the other. What writes code stays in the dev-aide. | Confirmed with the user. Projects runs in production on hosted repositories through `org.moqui.impl.GitServices`; the dev-aide edits working trees on dev instances (§4.10). |
| D8 | An artifact index in the framework (`core/devtools/`): every definition with its file and line, the references between artifacts read from the XML and from script string literals, and on dev instances the calls observed on the artifact stack; kept in database tables shaped for the questions the app asks (question 11). It serves search, the browser, completion, go to definition, reference checks, the agents' tools, `moqui refs`, MCP and a later language server. | One index, so the browser, the editor and the agents agree; built from the definitions the runtime loaded. In tables, so the database does the filtering, sorting and searching, the index survives a restart, and how far it scales is a measurement, not a guess. |
| D9 | The Artifact Browser draws the current artifact in the center, what uses it in a column to its left, what it uses to its right (or above and below), each side's artifacts grouped by type in one card with one connector, and each expansion a new card in the next column. Absolutely positioned Quasar cards on a pan-and-zoom layer with one SVG layer for the connectors: no HTML canvas, no Mermaid, no graph layout library. | Asked for. Generations stay in their columns, so the layout is one sweep down each column. |
| D10 | The screen builder's canvas is the real render of the screen in a same-origin frame, each widget stamped with its source position and the file that wrote it, under an overlay that selects, edits, removes and inserts. Structure is edited as blocks: a Vue tree of block components over the vendored SortableJS (nested groups) with a `q-menu` palette, driven by the XSD digest, for the logic composer and the screen outline; Tiptap is a time-boxed spike only if the tree fails a named measure (§9.2). No GrapesJS, no Blockly. | What the builder shows is what users get, with data. Blocks suit structure, and a contenteditable document cannot host live forms and lists. The review flipped the engine: ProseMirror is a rich-text document model, an attribute-heavy tree with multi-slot containers fights its schema, its history goes unused (D3), and what it adds is modest in Vue over what is vendored. |
| D11 | Agents: a `dev` agent in the Assist drawer with dev tools (artifact and source reads, checks, proposed edits, sim runs, git) behind dev skills. Every agent write is one change set through `propose_edit`: its card shows the change as artifacts with Preview (the screen rendered from the proposed text, F14) and Accept all, and Review opens it hunk by hunk in the editor, an approval handed off to the editor as Edit in screen hands one to a screen's Save. `write_ui` gains the dev-only `screen-xml` dialect as the preview of a proposed screen file; `qvue-screen` is later. Agents outside the app edit files on disk, the watcher makes that live, and they read the index and the checks over MCP and `moqui refs` and `moqui check`, later LSP. | "Help both people and AI agents". It keeps the approval model of LLM-A2A-Assist D3 and builds the `screen-xml` dialect its D5 left to these tools. One write path, reviewed two ways: business users accept what they can see, coders accept hunks. |
| D12 | Try in sim. Running a service, or a snippet as a scratch inline service, defaults to the sim overlay of embedded Postgres: the result, its messages, the rows it would write and its log. A run can be saved as a `moqui test` spec feature with a `checkData` of those rows. | Code runs against real data while it is written, with nothing kept; specs come from real runs. |
| D13 | Tasks: a small panel over a task source (this instance's Mantle work efforts when Mantle is loaded, or another Moqui instance over REST, such as the team's Projects server), with configurable app paths that open a project or a task (IronStone Projects, Marble ERP, Simple Screens). | Asked for. A developer's tasks usually live on the team's server, not in the dev database. |
| D14 | Browser first. The IDE views are `.qvue` screens with no terminal form; the lists (repositories, artifacts, ECA rules, problems, tasks) are XML screens with a `tui` form, so `moqui tui --agent` reaches them too. | An editor and its canvases need a browser; the lists cost nothing to keep in the terminal. |
| D15 | The dev-aide gives its users code execution as the server's OS user, by design: the main exception to the rule that no code comes in from outside and runs on the server, and so to phase 11's "no web eval endpoint even in development", for dev instances with the dev-aide loaded. Dev purpose only (D1), the `DEV_AIDE` permission, writes confined to the workspace roots, every write audited, agent writes only through approvals, the session token on every write. It is a sandboxing question: as the sandboxing improves and earns confidence, particular things may be allowed in particular contexts, each decided on its own (§15). | Answered question 1. Saying it plainly decides the controls. |
| D16 | A Dev package becomes `./moq build`'s default: Lite's features with `git` and `ssh` added, and `devtools` (what only development needs: the dev-component crates and the framework's file watcher), in a Cargo profile of its own with Lite's settings, `develop` (Cargo's `dev` is its debug profile, which the Debug package uses). `--lite` builds Lite as it is today. Debug has `devtools` too; Lite, Maximal and Enterprise do not (§3.3). | Answered question 5. Lite has neither git nor ssh, which development wants (the dev-aide, the hosting, the SSH apps), and the packages a deployment uses should not carry dev tools. |
| D17 | Extension first. The conf and Settings name a *site component* (`site-component`); roots whose components the site does not own are *upstream*: shown, searchable, read-only with a lock chip, and every Save on them reads "Extend in my-site". Every gesture that adds something writes an extension in the site component: `extend-entity` for a field, relationship, index or seed data; a `screen-extend` file for a form field, a list column, a widget at a named anchor, an action, a subscreen or a transition override; a new SECA or EECA file for a rule on an upstream service or entity. A Settings toggle, "Allow editing upstream components", turns the lock off for framework developers, with a warning. The agent follows the same rule (§13.1), and `check` reports an in-place edit of an upstream component as a problem while the lock is on. | Added by the review. For a site, editing mr-mantle or mr-marble in place is a fork that every upgrade fights; the framework already has `extend-entity` and `screen-extend`, and the IronStone plans defer "custom fields on any entity" to these tools. |
| D18 | Two arrangements of one model. Guided: "Customize this screen" from any app page, Entities, Screens and Services as tools of their own (the artifact in its visual view, the file status strip, Save, Text one click away, Problems and the Assist drawer, nothing else), a site Home with "Your customizations" and one-click Publish (stage the site component's changes, commit with a message Assist drafts and the task trailer, push). Workspace: the full IDE, with Branches, Tags, Stashes and hunk staging under an Advanced toggle. The same buffers, operations, save pipeline and approvals serve both. | Added by the review. Business users start on an app screen and want "what did I customize, and ship it"; coders want the IDE. A layout choice, not a second implementation. |
| D19 | A local dev instance per developer is the primary case; a shared team dev server is a mode from the first version (`dev-aide shared="true"`): commits carry the logged-in user as author, network git commands on a non-loopback listener are off unless an admin enables them with a token, presence shows who edits what, and one working tree with last save wins by hash and Compare on conflict until co-editing (F15). | Decided with the user at the review. Business users rarely run a server themselves, and the identity and conflict questions are cheap to answer up front and costly to retrofit. |

---

## 1. What it looks like

**Principles.**

- The running server knows the application: every entity, service, screen and rule as it loaded them, with their files
  and lines. The tools edit those artifacts, not only text, and the editor's completion, checks and navigation come
  from the same model.
- Save means live. One Save in every tool writes the file, reloads what it defines, checks it and shows its git status;
  a change made outside the app does the same through the watcher.
- Extend, do not fork (D17). An upstream component's artifacts are read and extended from the site component; the
  views show every definition as the server merged it, with its origin (the defining component, each extension,
  DbForm), and open the right file on edit. In-place edits of upstream files are the framework developer's exception,
  behind a toggle.
- The text is the truth (D3). Every visual tool is a view of a file's buffer; Text is one click away with the same undo,
  and nothing is reformatted.
- Errors teach: a check names the element, the XSD's documentation and the skill section that has the rule, so a person
  and a model fix it the same way (the pattern of `openui-lang`, docs/Assist.md "Parsed once").
- Agents work as people do: they read, run in sim and propose; the user reviews and saves. Nothing an agent writes
  reaches disk without the user's click.
- Failure tolerant, as [AGENTS.md](../AGENTS.md) asks: a file that does not parse keeps its previous definitions loaded
  and shows as a problem, and the index, the tree and the other reloads go on.
- Framework standard tools: Vue, Quasar and the `m-*` widgets. The one new library family is CodeMirror 6; Tiptap,
  SortableJS and the Vue compiler are vendored already.

**The app** is mounted under `apps` as `dev` ("Dev AIDE", menu index 97, beside System at 98 and Tools at 99).

| Menu | Path | Kind | What |
| --- | --- | --- | --- |
| Home | `dev/Home` | XML, tui | The site's customizations ("Site extensions: 14, 0 broken"), unpublished changes and Publish; repositories with changes, problems, the reload log, recent files; "No site component yet. Create one" when every root is upstream |
| Workspace | `dev/Workspace` | qvue | The IDE (§4, §5): a repository or directory, its tree, editor tabs with text and visual views, changes and history |
| Repositories | `dev/Repositories` | XML, tui | Every root with branch, ahead and behind and changes; fetch and pull all; get or create a component (§4.7) |
| Artifacts | `dev/Artifacts` | XML, tui; qvue | Search, an artifact's references as lists, the Customizations list (what the site extends or overrides, §7.2), and the browser canvas (§7, P9) |
| Entities | `dev/Entities` | XML, tui | Entities and view entities by package and component; each opens in the entity view (§8) |
| Services | `dev/Services` | XML, tui | Services by path with type, coverage and SECA count; each opens in the service view (§9) |
| Screens | `dev/Screens` | qvue | The screen tree with its mounts; each opens in the screen view (§11) |
| ECA Rules | `dev/Eca` | XML, tui | The active SECA, EECA, EmECA and gitECA rules (§10) |
| Data | `dev/Data` | XML, tui | Data files by component and type; load, check and export (§12) |
| Tasks | `dev/Tasks` | XML, tui | P8: my tasks from the task source, opened in the project app (§14); the first version keeps only the current task's trailer |
| Settings | `dev/Settings` | XML | Roots, editor, git, task sources and apps |

Every visual tool is a view of a file tab, and a file tab lives in either arrangement (D18). In the guided one,
Entities, Services and Screens are tools of their own: each opens the artifact in its visual view with the file status
strip (§4.8), Save, Text one click away, Problems inline and the Assist drawer, and nothing else, for example
`dev/Entities/Edit?artifact=moqui.example.Example`, which resolves the artifact to its file through the index. In the
Workspace the same tab opens among the others
(`dev/Workspace?component=example&path=entity/ExampleEntities.xml&view=entity&artifact=moqui.example.Example`; a
component names its root and its directory in it), so every tool has the same buffers, Save, git status, problems and
Assist context, and "Open in Workspace" moves between them. On any app page of a dev instance, "Customize this
screen" in the shell's page menu and the Assist drawer (§3.5) opens the screen builder on that screen with the page's
parameters, so the canvas shows the record the user was looking at.

**The workspace.**

```
┌ ≡ Dev AIDE › Workspace › mr-mantle                                  [Search ⌘K] [+] … [Account] [Assist] ┐
├────┬───────────────────────────────────────────────────────────────────────────────────────────────┬──────────┤
│ ▤  │ [▾ mr-mantle] [⎇ master ▾] ↑2 ↓0 · 3 changed            [Fetch] [Pull] [Push 2] [Commit…] │ Assist   │
│ ⌕  │ Code · Changes 3 · Commits · Branches · Tags · Stashes · Settings                             │ drawer   │
│ ±  ├────────────────┬──────────────────────────────────────────────────────────────┬───────────────┤          │
│ ◇  │ Files     ⌕ ⋯  │ service › mantle › order › OrderServices.xml  M   [Go to file] │ Outline       │          │
│ ☐  │ ▸ data         │ OrderServices.xml ● │ OrderEntities.xml │ Artifact: place#Order │ ▾ services    │          │
│    │ ▸ entity       │ [Text │ Service]  [Diff] [Blame] [History]       ✓ reloaded 9 ms │   place#Order │          │
│    │ ▾ service    M │ ┌──────────────────────────────────────────────────────────┐ │ Inspector     │          │
│    │   ▾ mantle   M │ │ CodeMirror: XML with schema and artifact completion,     │ │ <service>     │          │
│    │     ▸ order  M │ │ embedded JavaScript; gutter: changes, problems, folds    │ │ used by 7     │          │
│    │ ▸ screen       │ └──────────────────────────────────────────────────────────┘ │ [Browse]      │          │
│    │ ▸ test         │ Problems 2 · Output · Search · Try · Tasks                   │               │          │
│    │                │ Ln 120, Col 8 · XML · LF · UTF-8 · ⎇ master · 2 problems     │               │          │
└────┴────────────────┴──────────────────────────────────────────────────────────────┴───────────────┴──────────┘
```

The rail on the left switches the side panel: Files, Search, Changes, Artifacts, Tasks.

**A change, start to end.** On a site whose site component is `my-site` (D17), a user looking at an example in the
gallery's `ExampleDetail` (`runtime/base-component/example/screen/Gallery/ExampleDetail.xml`, an upstream screen) opens
Assist from "Customize this screen" (the `dev` agent, picked by default on dev-aide pages) and asks: "Add a due date to
examples and show it after Ends."

1. The agent calls `artifact` to describe `entity:moqui.example.Example` (its fields, file, line and origin) and its
   references (the screens and services that use it), reads the entity's element and the form with `source`, and, as
   the `site-customizing` skill says, calls `propose_edit` with two extensions in `my-site`: an `extend-entity` with a
   `field` `dueDate` in `my-site/entity/MySiteEntities.xml`, and in
   `my-site/screen-extend/example/screen/Gallery/ExampleDetail.xml` a `form-single name="ExampleFacts"` merge with the
   field placed after `exampleEndDate` in the form's `field-layout` (F16). Each edit is checked before it is shown; an
   in-place edit of the example component would have come back as a problem.
2. The drawer's card reads "Assist proposes 2 changes in my-site: Entity `Example`: + field `dueDate` (Date). Screen
   `ExampleDetail`: + field `Due Date` after `Ends`" with Preview, Accept all, Review and Reject. Preview renders
   `ExampleDetail` with the proposed extension (F14) and the record on the page; a note says the data appears once the
   entity half is applied. The user changes the title to "Due" on the card (its rows have their titles and places
   editable, as a service call's arguments are), previews again and accepts. A coder picks Review instead: both files
   open with the proposal as an inline merge, Accept and Reject on each hunk, and saves.
3. Accept writes both files. The report: entity `moqui.example.Example` relinked (or "restart to apply", with the
   button, until F5 lands), table `EXAMPLE` gained column `DUE_DATE`, screen `ExampleDetail` reloaded, no problems. The
   page shows the new field; both files show M in my-site.
4. The approval resolves as executed and the run reads "the user accepted 2 of 2 changes and edited one (the title)",
   then answers.
5. Home shows "Unpublished changes: 2". Publish commits them with the message Assist drafts and the current task's
   trailer, and pushes. In the Workspace the same changes sit in Changes with their diffs.

With "Allow editing upstream components" on, a framework developer gets the same flow against the example component's
own files. The same change from an agent in a terminal: it edits the two files on disk; the watcher reloads the entity
and the screen within a second; `moqui check` (or the MCP `check` tool) reports the files clean, or warns that they are
upstream; the Workspace's Changes shows them for review and commit.

---

## 2. Where it starts

As built, checked Oct 24, 2026.

**What follows a file change today.**

| Artifact | Follows its file | Where |
| --- | --- | --- |
| Screens (`screen/`, `screen-extend/`) | Yes: a cached definition is used while every file it was read from keeps its modified time, checked at most every 2 s outside production. Only whole caches clear (`ScreenFacade::clear_named`) | `core/screen/cache.rs` |
| `.qvue`, `render-mode text` files, templates | Yes: kept by location and modified time | `core/script/template.rs` `location_text` |
| Script files at a `location` | Yes: read again when the file changes, at a check interval, never in production | `core/script/runtime/facades.rs` `read_script` |
| REST API files | New paths: the roots are read again on a miss | `core/service/rest.rs` |
| Services | No: a file is merged into the factory's registry once and not read again; `lint` replaces the whole registry | `core/service/define.rs` |
| Entities | No: the model is immutable after `EntityModel::load` | `core/entity/share.rs` |
| SECA, EmECA and gitECA rules | No: loaded once per factory | `core/service/eca_rules.rs` |
| EECA rules | No: read on first use | `core/entity/load_feed/eca.rs` |
| A changed entity's table | Once per entity, on first use, when its group has `runtime-add-missing`: a missing table created, missing columns added | `core/entity/meta.rs` `check_table_runtime` |

`runtime/conf/MoquiDevConf.xml` still carries Java's expiries for the `service.location` and `entity.definition`
caches; the service registry and the entity model do not read them. So on a dev instance today a change to a service,
entity or ECA file the server has read takes a restart. Inline actions are no part of the problem: their scripts are
kept by their XML text (`core/script/actions.rs`), so new text compiles anew.

**What the dev-aide builds on.**

| Piece | What it gives | Where |
| --- | --- | --- |
| Assist | The agent runtime, approvals with hand-offs (Edit in screen, the transition observer), screen sessions on the tui model, skills with sections, the drawer's page state and suggested prompts | docs/Assist.md, plans/LLM-A2A-Assist.md |
| Canvas dialects | `openui` and `vue-sfc` with its sandbox frame; a registry that takes the planned `screen-xml` dialect as it is (`devOnly`) | `core/llm/ui/`, `webroot/js/WebrootAssistCanvas.js` |
| Registration from a crate | `submit_tool!`, `submit_dialect!`, `submit_assist_prompts!` | `core/llm/tools/mod.rs`, `core/llm/ui/mod.rs`, `core/llm/assist/prompts.rs` |
| MCP server | Read tools and admin-exposed services for agents outside, as the user | docs/MCP.md |
| Sim overlay | A call run against an overlay of the database, the rows it wrote, a fence against outside effects; embedded Postgres builds only | `core/entity/cache.rs` (`enter_sim`, `sim_changes`), the approval preview |
| Command line | `moqui types` (TypeScript of entities, services and `ec`), `moqui lint` (expression parses with oxc, Groovy idioms and hazards), `moqui test --coverage` (services no spec called), `moqui tui --snapshot` and `--agent` | `core/typegen/`, `core/script/screen_lint.rs`, `core/script/lint_hazards.rs`, `core/service/coverage.rs`, docs/TUI.md |
| XML Actions to JavaScript | One generator function per action element: the exact translation a composer shows | `fw/template/XmlActions.js` |
| Screen macros | The qvt and tui renderers the builder stamps | `fw/template/screen/ScreenQvt.js`, `ScreenTui.js` |
| Element positions | Each `MNode` has its file, line and column; the reader is hand-written, no XML crate | `lib/node/` |
| Widget fragments | Cached as re-serialized text (`node.write_string()`), so their positions are lost | `core/screen/definition.rs` `XmlFragment` |
| Vendored libraries | Vue 3.5.44 and its compiler for QuickJS, Quasar 2.35.1, Tiptap 3.31.4 with ProseMirror and Yjs in one bundle, highlight.js, Marked, DOMPurify, Mermaid, Chart.js, SortableJS, xterm.js; no code editor | `webroot/libs/refresh.mjs` |
| Read-only code | `code-view` (highlight.js) and the `moqui-diff` table | `fw/xsd/xml-screen-3.xsd`, docs/Revision-Management.md |
| Git hosting | Hosted `local`, `db` and `remote` repositories over gitoxide, merge requests, branch rules, gitECA rules; the `git` feature, which today's default package (Lite) leaves out | docs/Git-Hosting.md, docs/Packaging.md |
| Co-editing | Yjs rooms on the notification socket (`yrs`), `m-coedit` | `core/collab/`, `webroot/js/README.md` |
| XSDs | 12 schemas, 7,319 lines, with documentation; nothing reads them | `fw/xsd/` |
| Script console | None: phase 11 dropped the Groovy shell and decided on no web eval endpoint "even in development" | `runtime/base-component/tools/screen/Tools/GroovyShell.xml`, archive/Implementation-Phases.md |

**The corpus** the index and the reload work on (base-component and component, measured Oct 24, 2026): 1,251 XML
files; about 1,270 entity and view-entity definitions in 41 entity files; about 2,300 services in 163 service files;
920 screen files and 32 `.qvue`; 117 ECA rules (73 SECA, 36 EECA, 1 EmECA, 7 gitECA). The framework's 217-entity model
loads in about 5 ms (Performance-Implementation), so relinking the whole model on a save is cheap.

---

## 3. The component

### 3.1 Loading only in dev

- **Conf.** `MoquiDefaultConf.xml`'s `component-list` gains `<component-dir location="dev-component" purpose="dev"/>`
  after `component`. `purpose` (new, a comma list of instance purposes, empty for all) is also taken on `component`
  elements and on `component.xml`'s root; the dev-aide's `component.xml` says `purpose="dev"`.
- **The purpose at discovery.** Discovery runs before the default properties are applied (`core/context/factory.rs`:
  `component::discover`, then `apply_default_properties`), so it reads `instance_purpose` as the boot will set it: the
  process (`-D` or the environment), else `runtime/.env`, else the merged runtime conf's `default-property`. A
  component's own conf merges after discovery and cannot turn dev components on.
- **Left out.** On another purpose, each component in a purpose-restricted directory, and each whose `component.xml`
  names other purposes, is left out with a WARN: `Excluding component dev-aide (runtime/dev-component/dev-aide): it
  loads only when instance_purpose is dev; this instance is production`. A `component` element naming such a component
  is left out too: strict, until the sandboxing is trusted for a particular use (D15).
- **Dependencies.** A component that loads on every purpose may not depend on a dev-only one: discovery logs an error
  naming both and leaves the dependent out (the others go on). A dev component may depend on any.
- **The directory.** `runtime/dev-component` is a component directory as `runtime/component` is. The framework's own
  dev components are tracked in this repository, as base-component's are: the dev-aide first, more later, each named in
  `.gitignore` as an exception to `/runtime/dev-component/*` (`!/runtime/dev-component/dev-aide/`). A user's own dev
  components sit beside them in repositories of their own, which this repository ignores as it ignores
  `runtime/component`. `./moq`'s component tasks take `--dev` to work there (`component-get --dev`,
  `component-create --dev`), and `git`, `git-status` and `git-pull` include its repositories.
- The startup log names the dev components loaded, and `moqui version` the dev crates linked (§3.3).

### 3.2 Its parts

```
runtime/dev-component/dev-aide/
  component.xml            name="dev-aide" purpose="dev"; depends on the framework only
  MoquiConf.xml            the apps mount and the dev-aide conf element (§16)
  Cargo.toml, src/         the crate (§3.3): workspace files, the git layer, agent tools and dialects
  entity/                  moqui.dev.* (§16)
  service/org/moqui/devaide/  WorkspaceServices, VcsServices, ArtifactServices, ModelServices, TryServices, TaskServices
  screen/DevAide.xml       the app root, its menu and shell providers; screen/DevAide/ the screens of §1
  screen/devstatic/        DevAide.js (the client of the views and their pure parts) and its CSS
  data/                    DevAideSecurityData.xml and DevAideAgentData.xml (seed): access, the dev agent and its policy
  skill/                   the dev skills (§13.1)
  prompt/dev.md            the dev agent's addendum
  template/                component and screen templates (§11.2, §12)
  test/                    specs (moqui test dev-aide) and node tests of DevAide.js
```

### 3.3 Build and the Dev package

- The crate holds what must be Rust: the workspace file operations with their confinement (§4.1), the git layer
  (§4.2), the agent tools and dialects (§13), and the service implementations they share. The general pieces (reload,
  index, lossless XML, schema digest, checks) are the framework's (§17), because `moqui check`, `moqui refs` and MCP
  need them without the dev-aide.
- **The packages** (D16; docs/Packaging.md changes to this):

  | Package | `./moq` flag | Cargo profile | Binary | Features |
  | --- | --- | --- | --- | --- |
  | Dev | (none) | `develop` | `target/develop/moqui` | `dev`: Lite's, `git`, `ssh` and `devtools` |
  | Lite | `--lite` | `lite` | `target/lite/moqui` | `lite`, today's Default package as it is |
  | Debug | `--debug` | `dev` | `target/debug/moqui` | `maximal` and `devtools` |
  | Maximal | `--release` | `release` | `target/release/moqui` | `maximal` |
  | Enterprise | `--enterprise` | `enterprise` | `target/enterprise/moqui` | `enterprise` |

  The Browser build is as it was. Every task that runs moqui (`run`, `load`, `test`, `save-db`, `archive` and the rest)
  takes the same flags and uses the Dev binary without one. The package is named Dev wherever a name can be (`./moq`,
  the docs, the `dev` feature); only its Cargo profile is `develop`, as `dev` is Cargo's own debug profile.
- **`devtools`** (a new feature of `moqui-host`, forwarded by `crates/moqui` and the generated crate) is what only
  development needs: the dev-component crates, the framework's file watcher (§6.3) and later `moqui lsp`. The
  dev-aide's crate is a workspace member, as base-component's crates are, which `crates/moqui` links when `devtools` is
  on; a user's own dev crates are linked by the generated crate (`target/moqui-app`) as optional dependencies that
  `devtools` turns on. `./moq build --release -- --features devtools` adds it to another package, as `inbound-smtp` is
  added to a Lite build. Plain `cargo` keeps every feature, `devtools` included.
- **What changes with it.** The default build has git hosting, `git://` locations and the SSH server, so specs that say
  `requires: ["git"]` run in a plain `./moq test`, and the dev conf's `ssh_port` (2222) opens the SSH server on
  127.0.0.1 with `./moq run`, as 8080 and 8443 open the web server (a second instance on the machine sets other
  ports). The first build takes longer (gitoxide and russh). The default binary moves from `target/lite/moqui` to
  `target/develop/moqui`, so the README's Quick Start, docs/Packaging.md, `moq`'s help and completion, and any script
  that names the path change with it.
- A linked native component whose component is not loaded contributes nothing (docs/Native-Components.md). The agent
  tools registered with `submit_tool!` are not gated by component (`core/llm/tools/mod.rs` `registered` takes every
  linked one), so each needs the `dev.aide` capability (§13.2) and checks the component and purpose when called. A
  binary without `devtools` (Lite, Maximal, Enterprise) still loads the component's screens on a dev instance; its
  services answer that they are not built into this host, as a missing feature's do, and Home says how to build them
  in.

### 3.4 Access

- Artifact group `DEV_AIDE_APP` (the root screen, `inheritAuthz`), ALWAYS ALL for `ADMIN`, as `TOOLS_APP` and
  `SYSTEM_APP` are seeded (`runtime/base-component/tools/data/ToolsSecurityData.xml`); user groups `DEV_AIDE_USERS`
  and `DEV_AIDE_EXTENDERS` with the same screen access, for developers and business users who are not admins.
- Three permissions, in tiers, which every dev-aide service and dev tool checks: `DEV_AIDE_VIEW` (browse, search, read
  files, Try in sim); `DEV_AIDE_EXTEND` (everything of VIEW, and writes only inside site components, additive DDL
  only, Publish to the site's remote); `DEV_AIDE` (everything: in-place edits of upstream components when the toggle
  is on, destructive DDL, every git command). `ADMIN` and `DEV_AIDE_USERS` get `DEV_AIDE`; `DEV_AIDE_EXTENDERS` get
  `DEV_AIDE_EXTEND`. An agent has the tier of its user (§13.2).
- All of it is seed data of the dev-aide, so it exists only where the component loads.

### 3.5 The app root and the shell

- `MoquiConf.xml` mounts `component://dev-aide/screen/DevAide.xml` under `apps` as `dev`.
- `shell-providers` of the root: a `palette-source` `artifacts` (Entities, Services, Screens, Rules; the shell runs it
  only for users who may open the app), palette actions (Go to file, Go to artifact, New component, Repositories),
  `keymap` keys (Go to file, Go to artifact, Search in files, Save; as `moqui.keymap` actions, so a user rebinds them),
  and `assist-prompt` rows (§13.4).
- In dev, framework error pages and Monitor Logs lines that name a file and line link to the Workspace at that line when
  the dev-aide is loaded (`ec.factory.getComponentInfoList()` tells a screen).
- **Customize this screen** (D18): on a dev instance with the dev-aide loaded, for users with `DEV_AIDE_EXTEND` or
  above, the shell's page menu and the Assist drawer's header offer it on every app page. It opens the screen builder
  (§11.2) on the page's screen, in extension mode when the screen is upstream, with the page's parameters, so the
  canvas shows what the user was looking at; the drawer's suggested prompts on app pages come from the recipes of
  §13.1 ("Add a field to this form", "Add a column to this list", "Add a button here").

---

## 4. The workspace and its repositories

### 4.1 Roots

- A root is a directory the Workspace shows and may write. By default: the git repositories that hold the loaded
  components (found by looking up from each component directory for `.git`: this repository holds the framework, tools,
  webroot, example and dev-aide, and each component cloned with git into `runtime/component` or
  `runtime/dev-component` is its own), each loaded component in no repository (a plain root), and `runtime/conf` when
  no repository holds it. Settings and the conf add roots and exclusions (§16).
- A root lists the components inside it, so the tree can be scoped to one component or show the whole repository.
- **The site component and upstream roots** (D17). The conf's `site-component` (or Settings) names the component the
  site owns; more than one may be named, the first is the default target. A root whose components the site does not
  own is *upstream*: shown, searchable and openable, read-only with a lock chip on its files and its tabs, and every
  Save on it reads "Extend in my-site" and writes the extension there (§8.1, §11.2). The repository that holds the
  framework is upstream like any other. "Allow editing upstream components" in Settings (users with `DEV_AIDE` only)
  turns the lock off with a warning that names the fork. With no site component, Home says "No site component yet.
  Create one" and New component (§12) offers the Site kind first.
- **Confinement.** Every path resolves against its root's canonical directory: no `..` after normalization, no absolute
  path, symlinks resolved and required to stay inside the root, writes under `.git/` refused (and `.git/` hidden), a
  deny list (`runtime/db`, `runtime/log`, `runtime/txlog`, `runtime/tmp`, `runtime/.env`, `runtime/cert`, `target`),
  text over 2 MB opened read only, binary files shown (images, PDFs) or downloaded, never edited.
- Files a `.gitignore` ignores are dimmed (a toggle hides them); `target/` and `node_modules/` stay collapsed.

### 4.2 The git layer

The crate runs the `git` on `PATH` (or the conf's `git-command`) for working trees, never through a shell:

| Kind | Commands |
| --- | --- |
| Read | `status --porcelain=v2 -z --branch`, `diff` (worktree, index, commits; `--no-ext-diff`), `log` (NUL-separated formats, by page, by path), `show`, `blame --porcelain` (incremental), `branch`, `tag`, `stash list`, `remote -v`, `ls-files`, `check-ignore`, `grep` |
| Write | `add`, `reset`, `restore`, `apply --cached` (a hunk's patch), `commit -F -` (amend, sign-off), `switch`, `branch`, `tag`, `merge` (fast-forward only by default), `cherry-pick`, `revert`, `stash`, `init` |
| Network | `fetch`, `pull --ff-only` (rebase as an option), `push` (the upstream, set on the first push), `clone` |

- Arguments are argv, paths after `--`; `-c core.quotepath=off -c color.ui=false`; `LC_ALL=C` for parsed output;
  `GIT_TERMINAL_PROMPT=0`; `GIT_OPTIONAL_LOCKS=0` for reads, so status polling never takes `index.lock` from the
  developer's terminal.
- One write at a time per repository (queued); reads in parallel. Reads time out after 15 s; network commands after
  300 s, with Cancel, their `--progress` lines streamed to the page as events.
- Credentials are git's own: its helpers and the SSH agent of the server's environment. Nothing prompts; an
  authentication failure shows git's message and the hint (a credential helper, an SSH key, or the command in a
  terminal).
- Hooks run: a pre-commit hook's output shows in Output, and "Skip hooks" is a checkbox, off.
- Commits carry the logged-in user as author (`GIT_AUTHOR_NAME` and `GIT_AUTHOR_EMAIL` from the user account) and the
  server's OS identity as committer (`user.name`, `user.email`), so a local instance commits as its developer and a
  shared one (D19) still names who made each commit; the audit row names the user too. The Workspace shows both
  identities and warns when the committer's is not set. In a container, the developer's git config and credentials are
  mounted or an HTTPS token is used.
- **Shared mode** (`dev-aide shared="true"`, §16; D19): fetch, pull, push and clone are off unless an admin enables them
  in Settings, with a deploy token for the site's remote or a per-user HTTPS token, kept encrypted
  (`DevAideCredential`); the tree and the tabs show who else has a file open ("being edited by Jane", presence over the
  notification socket, ahead of F15); a save by one user is announced to the others with the file and who saved it,
  and their clean tabs take the new text (§4.9); the startup WARN on a non-loopback listener (§15) becomes INFO.
- No `git` on `PATH`: every root shows as plain, with a notice saying why (failure tolerant).

### 4.3 The layout: Gitea merged with an IDE

Gitea's repository page is a header and tabs, then a row with the file tree toggle, the branch drop-down, the
breadcrumbs, Go to file (T) and Add file, the file tree on the left and the directory or file on the right
(`templates/repo/view.tmpl`, `view_content.tmpl`, `view_file_tree.tmpl`); its editor is that page with the file in
CodeMirror and a commit form under it (`templates/repo/editor/edit.tmpl`, `commit_form.tmpl`). The Workspace keeps those
places and puts an IDE in them:

- **The repository bar**: the root picker (repositories and plain directories, with their components, a lock on the
  upstream ones), the branch drop-down as Gitea's (branches and tags tabs, a filter, Create branch from the current),
  ahead and behind the upstream, the change count, Fetch, Pull, Push (with its count), Commit (Changes with the message
  focused) and Publish (D18: stage the site component's changes, commit with a message Assist drafts and the current
  task's trailer, push; the guided arrangement's one git action). Publish pushes to the current branch unless a
  **publish branch** is set (question 15): per repository in its Settings, or per user by an admin (the
  `DEV_AIDE_PUBLISH_BRANCH` preference, set on the User screen in System and not shown to the user), as a name or a
  pattern such as `publish/{user}`; then Publish pushes there and opens a merge request on the hosting side when the
  origin is a hosted repository (Projects, or a known host), else names the branch to merge. Off by default; an admin
  may require it for a user or a repository, which hides the plain push.
- **Tabs**: Code, Changes, Commits, Settings (remotes, `.gitignore`, the root's components, the publish branch and
  whether it is required, Open on the hosting site when the origin is a known host); Branches, Tags and Stashes under
  the Advanced toggle, in the git-depth phase (P8).
- **Code**: the rail (Files, Search, Changes, Artifacts, Tasks); the tree (`m-file-tree`: lazy, filtered, git status
  on files with roll-ups on folders; New file, New folder, Rename, Move, Delete, Copy path, History, Blame, Open in its
  view, Show in the Artifact Browser); a directory shown as Gitea shows it (breadcrumbs with Copy path, Go to file, Add
  file with New file, Upload and From template, the entries with their last commit's summary and date, the README
  below); a file as an editor tab (§5); the bottom panel (Problems, Output, Search, Try, Tasks); the right panel
  (Outline and Inspector: the element at the cursor, its documentation, its artifact's summary and use count with
  Browse); and the status bar (line and column, language, line ending, encoding, branch, problems, the last reload).
- **Tabs are typed**: file tabs with a view (Text, or the file kind's visual view of §8 to §11) and tool tabs (an
  Artifact Browser, a commit, a compare, a change set to review, search results). The open tabs of each root are kept
  in the browser.
- **Search in files**: `git grep` in a repository (its `.gitignore` respected), a walk of the directory in a plain root;
  text or a regular expression, case, a path filter; the results in the bottom panel, each opening its line. Replace
  across files is later.
- The shell's app drawer is closed while the Workspace is the page (its menu button opens it), and the Workspace takes
  the window's height with no page scroll.
- Below 1024 px the Workspace reads only: the tree, a file, Changes as a list, the browser in vertical orientation.

### 4.4 Plain directories

The same layout: the repository bar has a folder chip, "my-app · Not a git repository", where the branch
drop-down was, and Fetch, Pull, Push and Commit disabled with that tooltip; Changes, Commits, Branches, Tags and Stashes
are shown disabled; the tree has no status marks; a directory's entries show their modified time; a file's chip says
"Not versioned". Settings offers Initialize a git repository here (with a confirm). With no history, a delete or a move
asks once more.

### 4.5 Changes and commit

- The changed files in groups: Conflicts (when there are), Staged, Changes, Untracked; each with stage, unstage,
  discard (confirm), open and diff.
- A file's diff in CodeMirror's merge view (side by side or inline); the working tree side is editable. Stage, Unstage
  and Discard on each hunk (a hunk is staged by `git apply --cached` of its patch) come with the git-depth phase (P8);
  the first version stages by file.
- The message: a summary and a body, the current task's trailer as a chip (§14), Amend, Sign off; Commit, and Commit and
  Push.

### 4.6 History, branches, tags, stashes, remotes and conflicts

The first version has the Commits tab, Switch and New branch; the rest of this section is the git-depth phase (P8),
since a terminal does it already and the users of D17 and D18 rarely need it.

- **Commits**: the branch's log or a path's (a file's History), by page; a commit's files and diffs (`m-diff-view`,
  the `moqui-diff` look, read only), Browse files at this commit, Revert, Cherry-pick onto the current branch, Copy id.
- **Branches**: local with upstream and ahead and behind, and remote; Switch (refused with git's message when local
  changes conflict, with Stash and switch), New, Rename, Delete (a second confirm to force), Merge into the current
  (fast-forward only unless a merge commit is chosen), Compare two refs (as Gitea's compare).
- **Tags**: New (lightweight or annotated), Delete, Push. **Stashes**: Stash (a message, untracked files), Apply, Pop,
  Drop. **Remotes** (Settings): add, set URL, remove, prune.
- **Conflicts** after a pull, merge, cherry-pick or stash pop: a file opens three ways (ours, the result, theirs) with
  Accept ours or theirs on each conflict and the result editable; Mark resolved stages it; Abort runs the operation's
  `--abort`.
- No rebase, `reset --hard` or force push in the first version, but `push --force-with-lease` after a confirm that names
  the remote branch (question 8).

### 4.7 Repositories

`dev/Repositories` lists every root as `./moq git-status` does: kind, path, components, branch, upstream, ahead and
behind, changes, last fetch. Fetch all and Pull all (fast-forward only) go root by root, and one that fails is reported
while the others go on; a pull ends with the reload's report and, when the index finds an anchor of a site extension
that the upstream no longer has (§5.6 "extension anchors"), "Extensions affected: 2", which opens Problems at them.
From here: Get a component (through `./moq component-get` from `addons.xml` and `myaddons.xml`,
`--dev` for `runtime/dev-component`, when the runtime has `moq`), New component (§12), and Clone a URL into
`runtime/component/<name>` or `runtime/dev-component/<name>`. A change to the set of components needs a restart, which
a banner says.

### 4.8 The file status in every tool

`m-dev-file-status` heads the editor and every visual view: the root and path (Copy), the status (M, A, D, U,
conflicted, ignored, Not versioned) and the branch with ahead and behind; its menu has Diff, History, Blame, Stage,
Discard and Commit.

### 4.9 Changes from outside

The framework's watcher (§6.3) passes changed paths to the dev-aide, which refreshes the tree and the status (300 ms
after the last) and tells the open tabs. A clean tab takes the new text and keeps its cursor and scroll. A tab with
edits shows "Changed on disk" with Compare, Reload (drops the edits) and Keep mine. Save checks the disk itself (§5.7),
so nothing is overwritten without being seen.

### 4.10 Projects' Code, reviewed

IronStone Projects' Code (`boss/plan/design-projects.md` §7, `boss/screen/Projects/Code/`, `ironstone.projects.CodeServices`)
hosts repositories for teams in production: Repositories, RepositoryDetail with Files, Commits, Branches, Tags,
Activity and Settings, Compare, CommitDetail, merge requests with reviews and line comments, Git access, all over
`org.moqui.impl.GitServices` and `GitReviewServices`, and tied to work items, projects and client access. Its §7 says
searching code and editing files in the browser belong to "the coming developer tools".

| Part | Projects (hosted repositories) | Dev AIDE (working trees) | Where it lives |
| --- | --- | --- | --- |
| File tree | `RepositoryFiles`: one directory at a time | `m-file-tree`, with git status | framework widget; Projects may adopt it |
| File view | `code-view` | `m-code-editor` (read only or editing) | framework |
| Diff | the `moqui-diff` table | CodeMirror merge view, `m-diff-view` | framework |
| Log, branches, tags, compare | gitoxide, through `GitServices` | git command, through `VcsServices` | each its own; the same shapes |
| Merge requests, reviews, CI statuses, webhooks, branch rules | yes | no: links to Projects when the origin is a hosted repository | Projects and the framework's hosting |
| Status, staging, commits, push and pull to remotes, stashes, conflicts | no | yes | dev-aide |
| Work items, clients | yes | the task panel (§14) | Projects; dev-aide reads a task source |

Confirmed with the user (D7): Projects' Code stays separate and different, with only the safe repository features. It
reads repositories, and writes only refs, merges, reviews and settings through the hosting services and their access
checks (`check#GitRepositoryAuthz`, the branch rules); it never edits or runs code. Moving it into the dev-aide would
also remove it from production, where Projects needs it, and its back end is not a working tree. So the two share what
is underneath (the framework's hosting services and the widgets) and stay independent, neither depending on the other,
and they link: a working tree whose origin is a hosted repository of a Projects server gets Open merge request there.
Editing a hosted repository's files in the browser (each save a commit through `git://`), the editing §7 left to the
developer tools, is a later root kind of the dev-aide (§18, P10), on dev instances only. Code search, read only, is a
safe feature by the same rule; whether Projects takes it back from "the coming developer tools" is its plan's call.

---

## 5. The editor

### 5.1 CodeMirror 6

- **Not Monaco.** Monaco is several MB, needs web workers (and `worker-src blob:` in a policy), does poorly on touch and
  small screens and themes apart from Quasar. CodeMirror 6 is MIT and modular, under a megabyte minified with the
  languages here, with XML schema completion, nested languages, lint, a merge view with per-hunk controls and an
  accessible DOM, and no `unsafe-eval`. Gitea's editor uses it now (`web_src/js/modules/codeeditor/main.ts`, with the
  VS Code keymap and indentation markers), where it had Monaco before.
- **The bundle**: `webroot/libs/codemirror/codemirror.min.js`, an esbuild IIFE exposing `CodeMirror`, built by
  `refresh.mjs` from pinned packages, each published at least two weeks before it is pinned as the others are:
  `@codemirror/state`, `view`, `language`, `commands`, `search`, `autocomplete`, `lint`, `merge`, `lang-xml`,
  `lang-javascript`, `lang-markdown`, `lang-json`, `lang-css`, `lang-html`, `lang-vue`, `lang-sql`, `lang-yaml`,
  `lang-rust`, their `@lezer/*`, `@replit/codemirror-indentation-markers` and `@replit/codemirror-vscode-keymap`; with
  MANIFEST.json and NOTICES.md entries and the [License-Review](archive/License-Review.md) gate (all MIT). It loads when
  the first editor mounts (`moqui.loadScript`), as Tiptap's does; other languages come later as a second bundle of
  legacy modes.
- **The widget**: `m-code-editor` in a new `webroot/js/WebrootCode.js` (loaded on first use and registered with
  `qvt.registerLate`, with `m-file-tree` and `m-diff-view`): `modelValue`, `language` or `file-name`, `read-only`,
  `line-numbers`, `wrap`, `highlight-lines`, and the URLs of a schema digest, completion and checks; it emits
  `update:modelValue`, `save` and `cursor`, and gives the dev-aide its `EditorView`. In an XML screen,
  `text-area editor-type="code"` with a new `language` attribute puts it in a form (the SQL runner, Service Run's JSON,
  a script field). It follows the shell's dark mode and the user's font size and keymap (default or VS Code; Vim later).
- highlight.js stays for `code-view` and for code blocks in Markdown.

### 5.2 One buffer, many views

- A file tab holds one CodeMirror `EditorState`: the text and its history. A visual view reads the buffer's outline
  (Lezer's XML tree as elements with their names, attributes and spans, kept current as the text changes) and the
  server's data on the artifact. A gesture becomes operations of the lossless XML layer, which answer text edits; the
  view dispatches them as one transaction, one undo step, and reads the outline again.
- **The lossless XML layer** has one implementation, Rust in `lib/xml_edit`, with its vectors in
  `crates/moqui-lib/testdata/xml-edit-vectors.json`; the agents' edits and the server's own use it directly, and a view
  applies an operation through it either in the browser through the WASM build of `moqui-lib` (a small module exposing
  `xml_edit`, loaded with the first visual view) or, until that module is built, by `POST xml-op` with the buffer's
  text and the operations, answered as replacement ranges the view dispatches as one transaction (a millisecond on a
  dev server). Lezer stays in the client for the outline, highlighting and cursor-to-element; it never writes.
  Operations: set, remove and rename an attribute; insert an element before, after, first in or last in another;
  remove; move; replace; set the text (a CDATA section kept); wrap and unwrap. New text follows the file: its
  indentation unit and line endings, and attributes on one line unless the siblings wrap theirs. Nothing else in the
  file changes.
- Undo is the buffer's: Mod+Z in a visual view undoes its last operation, and switching views keeps the history. A
  tab's unsaved text survives a reload of the page (IndexedDB, by root and path with its base hash, offered back when
  the file is opened).

### 5.3 Moqui XML

- **The schema digest.** The server reads the framework's XSDs into a digest per root element (the elements, the
  children each takes, and each attribute's type, enumeration, default, whether it is required, and documentation),
  kept by the XSDs' hash. It feeds `lang-xml`'s `elements` (completion of elements, attributes and enumerated values),
  hovers, the checks, the composer's palette and the properties panels. An XSD construct the reader does not take is
  logged, and that element is left open (any child, any attribute).
- **The reference rules.** One table in the framework (`core/devtools/refs.rs`) says which attribute names which kind
  of artifact and how it resolves in its context: `entity-name` an entity; `field-name` under an `entity-find`, or in a
  form with a `map` or `auto-fields-entity`, a field of that entity; `service-call`'s `name` a service, and its
  `field-map` names its in-parameters; `location` a resource; a `link` or `default-response` `url` a screen path
  relative to the screen; a form's `transition` a transition of the screen; a relationship's `related`; a SECA's
  `service`, an EECA's `entity`; `extends`, `implements`; in a data file the element names (entities), the attributes
  (fields) and status and enumeration ids (rows). The expression sites (`condition`, `from`, `in-map`, `${}` in text)
  are the list `moqui lint --screens` parses (`core/script/screen_lint.rs`). The table drives value completion (from
  the index, and for status items and enumerations from the database), go to definition (F12, Mod+click), references
  (Shift+F12, the browser's lists), hover cards (an entity's fields, a service's parameters) and the reference checks.
  `lang-xml`'s own completion knows only the digest's elements, attributes and enumerated values; everything the table
  adds (a field under its `entity-find`, a service's parameters in its `field-map`, a screen path from the screen) is a
  custom completion source over the index, a work item of its own.
- **JavaScript inside**: `<script>` bodies and the expression sites are JavaScript within the XML (CodeMirror's mixed
  parsing): highlighted, completed (§5.4) and checked.

### 5.4 JavaScript

- Completion of `ec` from a JSON digest of the host API that the type generator writes beside `moqui-host.d.ts`
  (`core/typegen/`: the facades and methods with their documentation); of the names known at the cursor (in-parameters,
  names set before); of entity and service names inside the string arguments of `ec.entity.find`, `makeValue` and
  `ec.service.sync().name`; and of in-parameter names in a `.parameters({...})` object of a named service.
- Checks: the oxc parse `moqui lint` runs, and the hazards of `core/script/lint_hazards.rs`. A script file at a
  `location` is checked as the function body it runs as (`core/script/parse.rs` `file_function`). A TypeScript checker
  is later and optional (question 10): if the framework takes TypeScript for scripts generally, its tools are in the
  binary and the checker is the server's, run as the other server checks are; a browser worker over `moqui types`'
  declarations only otherwise.

### 5.5 Markdown and the rest

Markdown with fenced code in its languages and a preview beside it (`m-markdown`); a skill file's front matter checked
as the skill index reads it. JSON, CSS, HTML, SQL, YAML and Rust are highlighted and edited (Rust says rebuild needed).
A `.qvue` file has its template, script and style each in its language, the template checked with the vendored Vue
compiler as `core/screen/vue_check.rs` does. Any other text is plain.

### 5.6 Checks

- **Sources**: the schema digest (an unknown element or attribute, a missing required attribute, a value outside its
  enumeration); the reference rules (an unknown entity, field, service, parameter, screen path, transition, form or
  location); JavaScript parse errors at expression sites and in scripts, placed inside the attribute value; the
  framework's lints; Vue template compile errors; and the reload's own failures (a service that did not register, with
  `core/service/define.rs` `registration_failures`' text).
- Two rule classes of D17: **upstream edit** (a change to a file of an upstream component while the lock is on, with
  the extension that would do it: "add the field through extend-entity in my-site") and **extension anchors** (a
  `screen-extend` whose named form, container, section or transition the base screen no longer has, an
  `extend-entity` whose entity is gone, a SECA whose service is), run after every reload and after a pull (§4.7), and
  by `moqui check --extensions` in CI.
- The schema checks run in the browser as the user types; the server's run 500 ms after typing stops and on save, on
  the buffer's text (`POST check` with the text and its hash).
- Each problem names its rule, and where there is one, the skill section and the XSD's documentation, so the Problems
  panel and the agents read the same words (§13.1).
- The Problems panel lists the open files' problems and the last reload's, across roots; a click opens the place.

### 5.7 Save

- One Save for every tool (Mod+S, Save all; the visual views' Save is the same): `POST save` with the root, the path,
  the text and its base hash. The server checks the user, the root and the path, and the base hash against the file (a
  409 with the disk's text when they differ, which opens Compare); keeps the file's line endings, encoding (UTF-8, a
  BOM kept) and mode; writes a temporary file beside it and renames it over; writes an audit row; reloads (§6) and
  checks; and answers the report: the new hash, the file's git status, what was reloaded, any DDL run, the problems,
  and whether a restart or a rebuild is needed.
- The status bar and a toast show it ("Saved · service file reloaded: 14 services · 9 ms"). A reload that fails keeps
  the previous definitions loaded and shows as a problem.
- Create, rename, move and delete take the same checks and audit; moving an artifact's file offers to update the
  references the index finds (§7).
- Save is explicit in every view, Design included: the canvas previews the unsaved buffer through F14's overlay
  (§11.2), so a save is a decision, and the buffer's undo is the way back before it. Save all writes files by artifact
  kind (entities, then services, then ECA rules, then screens), as the reload API orders them (§6.2), so a screen never
  loads before its field.

---

## 6. Live reload

### 6.1 What a change reloads

| Change | Reload | Change in the framework |
| --- | --- | --- |
| Screen XML (`screen/`, `screen-extend/`) | Its cached definition dropped, with the resolved paths holding it, at once instead of at the next 2 s check; a `screen-extend` file drops the screen it extends | F6 (`ScreenFacade::forget`) |
| `.qvue`, `.qjs`, `.tui.json`, `render-mode text` files, templates | Their text cache entry dropped (kept by modified time already) | F6 (an entry drop) |
| Script files | Their script text dropped (read again on change already, at an interval) | F6 |
| Service files | The registry rebuilt from every service file, as `lint` replaces it today, so replaced and removed services, `implements` and `service-include` are all resolved by the same code as a boot (inline actions compile anew from their new text); a per-file merge only if the rebuild measures slow on the corpus of §2 | F3 |
| REST API files | The roots read again | F6 |
| SECA, EmECA and gitECA files | That kind's rule set built again from its files (81 rules today) | F4 |
| EECA files | The EECA rules built again | F4 |
| Entity files | First version: "restart to apply", with the restart button (§6.5); the restart's startup add-missing check makes the new columns. Later (F5, staged to the modeler's phase): the changed files read again, the model relinked and swapped in (requests in flight keep the one they have); derived definitions, data document and SQL caches and the entity caches of the changed entities dropped; the add-only table check of those entities run again | F5 (staged) |
| Data files | Nothing by itself; Load and Check (§12) | |
| Skill files | That file synced into the skill index | F6 |
| `MoquiConf.xml`, `component.xml`, the runtime conf | Restart needed | |
| `src/**/*.rs`, `Cargo.toml` | Rebuild needed (`./moq build`) | |
| webroot JavaScript and CSS | The open pages are told to reload | |

### 6.2 The reload API

`core/devtools/reload.rs` `reload_paths(paths) -> ReloadReport`: classifies each path by its place and root element,
groups the paths by kind and runs the kinds in dependency order (entities, services, ECA rules, screens and the rest),
each kind once (one relink for every changed entity file) and each independently (a failure is reported for its file
and the rest go on), and answers per file what it reloaded, added and removed, any DDL, the
problems, and restart or rebuild. The dev-aide's save and the watcher call it; the last 200 reports are kept for Home's
reload log and the agents (`check` with no file, §13.2).

### 6.3 The watcher

On a dev instance of a package with `devtools` (Dev and Debug, §3.3), with `dev-reload` on (it is when
`instance_purpose` is dev, unless turned off), a file watcher (the `notify` crate: inotify, FSEvents,
ReadDirectoryChangesW; through the license gate and `deny.toml`) watches every component directory except
`runtime/db`, `log`, `tmp` and `target`. It waits 300 ms after the last event (an editor's
write-and-rename is one change) and calls the reload for what changed. It is the framework's, so an agent or an editor
in a terminal gets live reload without the dev-aide; the dev-aide adds its tree and tab events.

### 6.4 In the browser

A reload sends `DevReload` notifications (`/notws`) to the users with the dev-aide open: the Workspace refreshes, an
open screen preview reloads in place, `moqui.componentCache` drops the screens whose files changed (a cached `max-age`
screen would otherwise stay), and the problems update.

### 6.5 Restart and rebuild

A conf or component change needs a restart, an entity change too until F5 lands, and a Rust change a rebuild; a banner
says which. **Restart** is a button on the banner from the first version: the server re-executes itself with its
arguments (a `DEV_AIDE` transition, audited; under `./moq run` the same process comes back), `DevReload` tells the open
pages to reconnect, and the Workspace returns to its tabs from the browser's state. Sessions and the sim overlay are
lost, which the confirm says. It is the universal fallback that makes every phase shippable without the risky reloads.
**Rebuild** (`./moq build` run from the app, its output streamed, then the restart) is later (P10); the banner shows
the command until then.

---

## 7. The artifact index and the Artifact Browser

### 7.1 The index

- **Artifacts**, each with an id, kind, name, component, file, line, a one-line summary and its **origin** (the
  defining component, each component whose `extend-entity`, `screen-extend` or form `extends` adds to it, with file
  and line, and the DbForm rows that change a form): entities, view entities and their extensions, and fields;
  services, with what they implement and include; screens with their transitions and forms; SECA, EECA, EmECA and
  gitECA rules; REST resources and methods; data documents, data feeds, service jobs, artifact groups, email templates
  and DbForm forms (rows of the database); scripts and templates (files); skills. Whether a component is upstream or
  the site's (D17) is a property of the component, so every view and the agent can say which side of the line an
  artifact and each of its extensions is on.
- **Ids**: `entity:mantle.order.OrderHeader`, `field:mantle.order.OrderHeader.statusId`,
  `service:mantle.order.OrderServices.place#Order`,
  `screen:component://mr-marble/screen/SimpleScreens/Order/OrderDetail.xml`, `transition:<screen>#<name>`,
  `form:<screen>#<name>`, `seca:<file>#<id or position>` (and `eeca`, `emeca`, `giteca`), `rest:/mantle/orders`,
  `job:<jobName>`, `document:<dataDocumentId>`, `file:<root>/<path>`.
- **References**, each with its source position and how it refers (`entity-find`, `entity-auto create`, `service-call`,
  transition service, SECA `post-service`, `subscreens-item`, `link`, `member-entity`, relationship, script string,
  and the **extension anchors**: a `screen-extend`'s named form, container, section or transition, an
  `extend-entity`'s entity, a SECA's service, so a pull can report what an upgrade broke, §5.6):
  from the XML by the reference rules (§5.3); from scripts and inline `<script>`s by oxc, the string literals of the
  calls the rules name; from rows (a job's service, a data document's entities, an artifact group's members).
- **Observed calls.** On a dev instance the artifact stack (`core/context/artifact.rs`) counts each pair of a frame
  and the frame pushed on it (screen, transition, service, entity): what static reading cannot see (a name made at run
  time) and what really runs. The counts are kept per thread and merged each second, so a push costs one map update,
  and the merged counts are written to `DevArtifactCall` rows each minute, so they survive a restart and join the
  reference lists; between flushes the memory holds at most 200,000 pairs, the oldest dropped first.
- **Stored in the database** (answered question 11). The index is rows: `moqui.devtools.DevArtifact` (one per
  artifact, with its origin and its file's hash), `DevArtifactRef` (one per reference, with its position and how it
  refers) and `DevArtifactCall`, framework entities in `fw/entity/DevToolsEntities.xml`, shaped for the questions the
  app asks, so the database does the filtering by kind and component, the sorting, paging and counting, and the text
  search (the dialect's own operators where it has them, `LIKE` on the name's segments elsewhere), and a restart keeps
  the index: a file whose hash is unchanged is not read again. Built in the background at startup on a dev instance,
  or at first use; kept current per file by the reload (§6), which replaces that file's rows in one transaction. A
  file that does not parse is logged, and its old rows stay, marked stale. How far it scales is measured, not assumed:
  the corpus of §2 and a synthetic one ten times its size, in P3; the target for the corpus is a first build in under
  2 s on the Dev package and a per-file update in milliseconds. A model-only boot (`moqui check` and `moqui refs`
  without `--server`) builds a small in-memory index of the loaded definitions instead, enough for the reference
  checks.
- **API**: `find(text, kinds, components, limit)` (camel case and segment matching: `OrdHdr`, `place#Order`),
  `describe(id)`, `refs(id, direction, kinds)`, `at(file, offset)` (the artifact or reference at a position, for go to
  definition; a range query on the file's rows), `unresolved(file)` (for the checks); each an entity find, so a screen
  can also list the rows directly. `org.moqui.devaide.ArtifactServices`, the agents' `artifact` tool and `moqui refs`
  (§13.6) wrap it.

### 7.2 Search

`dev/Artifacts/Search` (a find with kind and component chips, results with summary and file), the shell palette's
Artifacts group, and Go to artifact in the Workspace. A result opens in its view, in Text at its line, or in the browser.

**Customizations** (`dev/Artifacts/Customizations`, XML with a `tui` form, and Home's summary): every artifact of the
site components that extends or overrides an upstream one, by kind and file, with its anchors' state ("Site
extensions: 14, 0 broken"), Open and Browse; the upgrade checklist after a pull (§4.7). Later, a DbForm row here offers
"Convert to site extension", which writes the equivalent `screen-extend` and removes the row through an approval.

### 7.3 The browser

```
                         ┌ used by ─────────────┐                      ┌ uses ────────────────┐
┌ used by ──────────┐    │ Screens          4   │                      │ Entities          6  │    ┌ uses ─────────┐
│ Screens       1   │────│ ◂ OrderList          │    ┌─────────────┐   │   OrderHeader      ▸ │────│ Entities    3 │
│ ◂ FindOrder       │    │ ◂ OrderDetail        │────│ service     │───│   OrderItem        ▸ │    │ Services    1 │
│ Transitions   2   │    │ Services         3   │    │ place#Order │   │ Services          4  │    └───────────────┘
└───────────────────┘    │ ◂ …                  │    │ mantle.order│   │   …                ▸ │
                         │ SECA rules       1   │    │ :412        │   │ SECA rules on it  2  │
                         └──────────────────────┘    └─────────────┘   └──────────────────────┘
```

The center is `place#Order`; its users are in the card to its left and its uses in the card to its right, one connector
each. `OrderList` was expanded, so its own users are a card further left, connected to its row; `OrderHeader` was
expanded to the right the same way.

- **The center**: the current artifact's card (kind, name, component, file and line, its summary) with Open, Open in
  Text, Try or Preview or Data, Copy name.
- **The sides**: one card left with everything that uses it, one card right with everything it uses, each a section
  per kind (Screens, Transitions, Services, Entities, SECA rules, EECA rules, Jobs, Data documents, REST, Scripts …).
  A row has the kind's icon, the short name (the full one in its tooltip), how it refers, its observed count, and on its
  outer side the expand button. Eight rows a section, then "+ n more" opens the section.
- **Generations**: expanding a row adds a card in the next column out, holding that row's own users (on the left) or
  uses (on the right), connected to the row. Expanding several rows of a card puts several cards in the next column,
  in the rows' order. Collapsing removes the card and what grew from it. An artifact already on the path from the
  center is marked as a cycle and not expanded again; the same artifact elsewhere lights up on hover.
- **Connectors**: one per card, from the row it came from to the card's near edge, a curve in one SVG layer beneath the
  cards; dashed when the link was only observed.
- **Layout**: a column's x by its generation; in a column the cards in their source rows' order, each at its source
  row's height and pushed down past the card above it, in one sweep; measured after render (ResizeObserver) and placed
  again. Vertical orientation swaps the axes (users above, uses below). A pure function with node tests.
- **Pan and zoom**: one CSS transform on the layer: drag the background, wheel or trackpad to pan, Ctrl+wheel or a pinch
  to zoom at the pointer, keys for both, Fit and Center.
- **Filters**: kinds, components, framework artifacts (hidden by default when the center is not the framework's),
  observed and static links.
- Any row becomes the center on a click (Back and Forward keep the trail). The same expansions are a List view for
  keyboards and screen readers, and `dev/Artifacts/Refs` gives them as an XML screen with a terminal form. The List
  view and `dev/Artifacts/Refs` ship with the index (P3); the canvas follows in P9.

---

## 8. The entity modeler

### 8.1 The entity view

The view of an entity file's tab, one entity at a time, chosen from the file's outline or by the cursor:

- **Header**: name, package, group, origin (the defining component with a lock when it is upstream, and a chip per
  component whose `extend-entity` adds to it, the site's first), and its other attributes (cache, audit, use and the
  rest) from the digest in a panel. For an upstream entity the view is the merged definition and every edit lands in
  the site component's `extend-entity` (D17), which the strip names ("extending in my-site"); with the upstream toggle
  on, the edit goes to the file that defines the thing edited.
- **Fields**: a flat list of about 3 to 50 rows: name, type (the field types of the database conf, each database's SQL
  type in its tooltip), primary key, not null, default, encrypt, audit log, localization, create only, description,
  origin; edited in place; add after the selected row, drag to reorder, remove (with its uses from the index, "used in
  3 screens and 2 services"). A field another component adds through `extend-entity` names that component and its
  edits go to that file. **Add** asks for a kind before a type: Text, Number, Date or Date-time, Yes/No, Picklist (a
  new or an existing `EnumerationType`), Reference to entity (a `one` relationship, the related entity from the
  index), Status (a `StatusType` with its items and flow), or Other (the raw field types). Picklist and Status are
  composite operations in one undo step: the `...EnumId` or `statusId` field, its relationship, and the
  `EnumerationType` and `Enumeration` (or `StatusType`, `StatusItem` and `StatusFlowTransition`) rows in the site
  component's seed data file (§12), loaded on save. A new field ends with "Show it on a form", which opens in the
  builder (§11.2) the screens the index finds using the entity, with the hint that a form listing its fields needs the
  field added while an `auto-fields-entity` form shows it already. A label asked for in another language goes to the
  component's L10n messages.
- **Relationships**, outgoing and editable: type (`one`, `many`, `one-nofk`), the related entity (a search drop-down over
  the index), title, key maps (from the related entity's primary key unless given), fk name.
- **Incoming**, one read-only list: the entities whose relationships point here (from the index), with their
  relationship, type and key fields, each linked to that entity's view, and Browse for the rest.
- Indexes; master definitions; seed data; the EECA rules on the entity (edited in §10's view); Where used (a count with
  Browse); Data (the row count and first rows, a link to Tools' Data Edit).
- **The table**: its columns against the fields (missing columns, columns no field names, type differences), from the
  database's metadata.

### 8.2 Saving and the database

- Save writes the file (D3), relinks the model (F5; "restart to apply" with the button until it lands, §6.5) and runs
  the table check of the entity: a missing table is created and missing columns are added. On a dev instance the
  dev-aide runs it for the changed entities whatever the group's `runtime-add-missing`, and the report lists the DDL.
  This additive DDL is all the `DEV_AIDE_EXTEND` tier ever runs.
- What the check does not change is said: a removed field leaves its column, a type change leaves the column's type,
  a renamed field adds a column. Moqui keeps no migrations (`change-set` is tabled in `entity-definition-3.xsd`), so
  for `DEV_AIDE` users the view offers what a developer would do by hand on a dev database, each with its SQL shown
  first and a confirm that has the table's name typed back: drop the column, change its type, rename the column keeping
  its data, drop and create the table losing its rows. A rename defaults to "add the new column and keep the old"; the
  destructive form is the second choice. They run through the SQL runner's path (`core/entity/info.rs` `run_sql`) and
  are audited. The extend tier sees none of them.

### 8.3 The view-entity editor

- **Members**: a member entity is picked with a search drop-down. Once there are members, the drop-down first offers
  the relationships of the member it joins from, as `member-relationship` by relationship name with its key maps
  implied, and then any entity. Join type (optional or not) and sub-select per member.
- **Aliases**: `alias-all` per member with its excludes; aliases with functions, `case` and complex aliases, the fields
  picked from the members.
- `entity-condition` with its `econdition`s and `date-filter`, and `order-by`, with field pickers.
- A diagram of the members as boxes and their joins as lines, on the browser's pan-and-zoom layer.
- Run: the SQL the view makes and its first 20 rows, read only.

---

## 9. The service writer

### 9.1 The service view

The view of a service file's tab, its services in the outline, one at a time:

- **Header**: verb and noun, type (inline, entity-auto, script, rust, the remote types, interface), location and method,
  authenticate, allow remote, transaction and timeout, semaphore, validate, description, implements and includes.
- **Parameters**: in and out, nested for Map and List types, each with name, type, required, default, allow-html, its
  validations and description; `auto-parameters` from an entity shown expanded, each implied parameter marked.
- **Implementation**: inline actions in the logic composer or as text; a script service's file opened as a second
  buffer in the view and saved with it; entity-auto shows its entity and verb; rust shows its location and opens the
  `.rs` file when it is under a root.
- The SECA rules on the service (edited in §10's view), and Add.
- Where used (the index) and coverage (the last `moqui test --coverage` report's call count, when there is one).

### 9.2 The logic composer

- **Blocks.** Each XML Actions element is one block, so the outline is the XML: containers (`if` with its `else-if` and
  `else`, `iterate`, `while`, an `entity-find` with its conditions) hold blocks, and the order is top to bottom as the
  actions run. An outline of synchronous steps, not a flow chart.
- **Sentences.** A block reads as a sentence with its main attributes as chips (a table per element in the client over
  the digest's attributes): "Find one Example where exampleId = exampleId → example", "Call
  mantle.order.OrderServices.place#Order with context → result", "If !example … else …", "For each item in
  orderItemList", "Set total = subtotal + tax", "Return error: Order not found". A chip opens a small editor (an
  expression in a one-line CodeMirror with completion, an entity or service in the search drop-down); Properties has
  every attribute with its documentation.
- **The palette** (`/`, or + between blocks) is grouped as the XSD groups the operations: Control (`iterate`, `message`,
  `return`, `break`, `continue`, `check-errors`), Environment (`set`, `filter-map-list`, `order-map-list`), Calls
  (`service-call`, `script`, `service-group`), Entity find (`entity-find`, `entity-find-one`, `entity-find-count`,
  `entity-find-related`, `entity-find-related-one`), Entity values (`entity-make-value`, `entity-create`,
  `entity-update`, `entity-delete`, `entity-set`, `entity-delete-by-condition`, `entity-delete-related`), Entity other
  (`entity-data`, `entity-sequenced-id-primary`, `entity-sequenced-id-secondary`), Conditions (`if`, `while`, `assert`)
  and `log`; with snippets ("find one by its key", "create from the parameters", "loop over a find").
- **Names**: those known before each block (in-parameters, and what blocks set: `set`'s field, `value-field`, `list`,
  `out-map`, the `iterate` entry) complete expressions and mark an unknown name.
- A `service-call` block lists the called service's in-parameters, the required ones marked, and adds them on a click.
  A `script` block shows its JavaScript, edited in a popup editor.
- **Generated JavaScript**: a read-only view of the script the framework runs for the actions
  (`fw/template/XmlActions.js`), with the element comments that `MOQUI_ACTIONS_DEBUG` writes.
- The composer edits XML Actions wherever they are: screen `actions`, `always-actions` and `pre-actions`, transition
  actions, ECA rule actions.
- **The engine** (D10): a Vue tree. Each block is a component drawn from the outline (§5.2) and the xml-actions
  digest; containers nest; SortableJS nested groups move blocks within and across containers with drop lines; the
  palette is a `q-menu` on `/` or the + between blocks; arrow keys move the selection and Alt+arrows the block; undo is
  the buffer's. Every change is an operation of §5.2 and the tree is read again from the outline after it, so the tree
  never holds state the text does not. The measures the engine must meet: 300 blocks drawn in under 100 ms, nested drag
  across three levels, a chip edit that round-trips through the vectors; if the Vue tree fails one, Tiptap (vendored)
  is a time-boxed spike against the same measures, and only then the engine.

### 9.3 Finding services

`dev/Services` (by path and component, with type, parameter count, SECA count and coverage), search over names,
descriptions and parameters (the index), the browser, and Go to artifact.

### 9.4 Try in sim

- The Try panel: a form of the in-parameters (values kept per service in the browser), Run in sim (the default) and Run
  for real (with a confirm), as the current user.
- In sim it runs on a thread of its own logged in as the user, as the approval preview does (`enter_sim`, the sim fence
  that refuses outside effects, `sim_changes`), for at most 30 s, and answers the out-parameters, messages and errors,
  the rows written (entity, create or update or delete, key, fields before and after), the call's log lines and its
  time. Sim needs embedded Postgres (`EntityFacade::sim_available`); without it Try offers Run for real only.
- **Save as spec** writes a feature into `test/<Service>.spec.js` of the service's component (made when missing):
  `call(name, parameters)`, a `then` comparing the out-parameters, and a `checkData` of the rows the sim wrote, for the
  developer to trim. Run spec runs `moqui test <component> --spec <Name>` and shows the result in the bottom panel.
- **Snippet**: a scratch inline service (XML Actions or JavaScript) run the same way, kept in the browser and saved only
  when asked (question 1).

### 9.5 Assist in the writer

The agent sees the service and the selection (§13.4); an answer's code blocks get Insert at cursor, Replace selection
and Propose as change (§13.3); the prompts include "Explain this service", "Write a spec from my last Try" and "Add
parameter validation".

---

## 10. ECA rules

- `dev/Eca` lists the active rules of each kind as the framework holds them, after an `id` replaced an earlier rule and
  the priorities sorted them (`core/service/eca_rules.rs`, `core/entity/load_feed/eca.rs`): SECA by service and
  `when`, EECA by entity and event, EmECA, and gitECA by event and repository and ref pattern; each with its file and
  line, priority, id, and its condition and actions in short. A rule replaced by a later one of its id shows under it,
  marked replaced. Filters: kind, component, service or entity pattern, event.
- **The ECA view** of an ECA file's tab: its rules as a list; a rule's form (its kind's attributes from the digest, the
  condition in an expression editor, the actions in the logic composer); Add (in this file when it is the site's, else
  in a new `service/<Name>.secas.xml` (or `.eecas.xml` under `entity/`) of the site component, D17, or another the
  user picks), Remove, Move.
- Save rebuilds that kind's rule set (§6). The service and entity views list their rules and link here.
- Later: a SECA's condition evaluated against the context of its service's last Try, an EECA's against a record, a
  gitECA rule against a push from the push log (`GitUpdate`).

---

## 11. The screen builder

### 11.1 The screen tree

- `dev/Screens` shows the screens as the screen facade mounts them: each root under `apps` (and webroot's others), and
  their subscreens by the three ways Moqui mounts one: a file in the directory of the parent's name, a
  `subscreens-item` in the parent, and a `screen-facade` mount in a component's `MoquiConf.xml` (as IronStone mounts its
  apps). A node shows its menu title, path, file, how and where it is mounted (with file and line), whether it is the
  default subscreen, render modes, `.qvue`, its terminal form's state (the TUI worklist of `moqui lint --screens`) and
  its authentication.
- **Actions**: Open in the screen view; New subscreen (from an archetype template, §11.2, as a file in the directory, or
  an item for a screen kept elsewhere; under an upstream parent the item goes in the site component's `screen-extend`
  of that parent, its `subscreens` merge, D17); Move (a directory mount moves the file and its directory; an item or a conf
  mount is edited; the references to the old path the index finds are offered for update); Remove (with its references
  first); Set default; Reorder the menu (`menu-index`).

### 11.2 The design canvas

- **The frame.** The screen renders in a same-origin frame of the Workspace, with its parameters set in a bar (required
  ones asked for, recent values offered, the page's own when opened by Customize this screen), at a width (390, 768,
  1280, full) and light or dark. Preview mode is the live screen; Design mode lays the overlay on it. The frame renders
  the tab's buffer, not the file: F14's overlay puts the unsaved text (or a proposed change set's) at the screen's
  location for that request, so Design shows every change before Save (§5.7).
- **Stamps.** A render that carries `X-Moqui-Aide: source`, from a `DEV_AIDE_EXTEND` or `DEV_AIDE` user on a dev
  instance, answers `frame-ancestors 'self'` in place of `'none'`, and `ScreenQvt.js` writes
  `data-m-src="<file>:<line>:<col>"` on each widget's root element, the file being the one that wrote the element: the
  base screen, a `screen-extend` (merged elements keep their own file), or `dbform:<formId>` for a field a DbForm adds.
  The positions come from the parse: on a dev instance the definition keeps each widget element's file, line and column
  in an attribute before its fragment is serialized, since `XmlFragment` keeps only re-serialized text (F10).
- **Extension mode** (D17). An upstream screen opens in it: the canvas is the merged render; elements the site's
  extensions wrote carry a badge, DbForm fields "from DbForm · group X", upstream elements a lock; the tab's buffer is
  the site's `screen-extend` file for the screen (`my-site/screen-extend/<path after ://>`, made on the first edit),
  so D3 holds. The selection's actions are the extension operations: Add field to this form and Add column (a form
  merge with the field's place in the layout, F16), Insert before or after (a `widgets-extend` at the nearest named
  sibling, or `prepend` and `append` inside a named container), Replace section, Override transition, Actions before or
  after. An **Extension points** list in the right panel names the base screen's anchors (named containers, forms,
  sections, transitions); an unnamed element says "cannot be anchored; nearest anchor is X" and offers the insert
  there, and, to a user who may edit upstream, "Add an anchor here", which names the element in the base screen's own
  file: anchors and extension points are added to the base screen by an edit of it, never generated (question 14), and
  the extension then follows. With the upstream toggle on, or on a site's own screen, Design edits the screen's file as
  below.
- **The overlay**: a hover outlines the element under the pointer and names it (`form-single ExampleFacts`, `field
  statusId`) with its origin; a click selects it (Properties); a double click edits its text or title in place. The
  selection has Edit, Remove, Insert before, Insert after and Insert inside (a container), a drag handle to move it
  (drop lines between siblings and in containers), Select parent (Esc) and Open in Text. A change is an operation on
  the buffer, the frame re-renders from the buffer in place keeping its scroll and the selection by source position,
  and Save writes it.
- **Insert by intent.** The menu is shaped by what is selected, and each intent writes the XSD elements (and in
  extension mode the extension that carries them): on a form-list, *Add column* (a field of the entity, a field over a
  relationship, a computed one; a `form-list-column` placed after the selected column) and *Add filter* (a field merged
  into the find form when the list's find uses `search-form-inputs`, else it says why not); on a form-single, *Add
  field* (an entity field, its title asked for, placed after the selection in the `field-layout`); on a detail
  screen, *Add section* (a related list over an incoming relationship from the index, a box, a tab) and *Add button*
  (a transition that calls a service picked from the index, or one the agent composes in the site component, and its
  link); on a list row, *Add row action*. Under them, **Insert, XML screens** offers the widgets the XSD takes at that
  place (`AllWidgets` in containers, the field widgets in a field, the standalone fields), with snippets: a form-single
  of an entity's or a service's fields, a form-list of an entity with its find, a container-box with header and body, a
  container-row of two columns, a link to a screen or a transition (pickers), a section with a condition, a
  dynamic-dialog, a button-menu, a status-bar, a number-card, a chart. A new screen starts from one of the style
  guide's archetypes (`boss/plan/style-guide.md` §3: Landing, List, Detail, 360, Task, Board, Calendar, Split, Settings,
  Wizard, Dialog, Report, Editor, Panel, Tree list), as the gallery shows them
  (`runtime/base-component/example/screen/Gallery/`).
- **Forms**: a field's place on screen comes from the form's `field-layout` when it has one, so moving a field moves
  its `field-ref` (or a list's `form-list-column`), and adding a field to such a form adds its `field-ref` too; without
  a layout it is the order of the `field` elements. A form merge from a `screen-extend` does the same through F16's
  `after`.

### 11.3 Outline and properties

The Outline is the screen's widget tree as blocks, on the composer's engine (§9.2): drag to move, `/` to insert, Delete,
duplicate, keyboard moves; selecting a block selects the element on the canvas and in Text. Properties shows the
selected element's attributes from the digest with typed inputs (an enumeration a drop-down, a boolean a toggle, an
entity, service, field, transition or screen path a picker from the index) and their documentation, each checked as it
changes.

### 11.4 `.qvue` screens

The first version opens a `.qvue` screen in Text with the preview frame; the design mode below is P9. A `.qvue` screen
is the coder's form, and D17's site extensions are XML.

- A `.qvue` screen is two files: its XML (parameters, transitions, actions, `render-modes`, and the `render-mode text`
  naming the `.qvue`) and the `.qvue` (template, script, style). The screen view opens both, saved together.
- The template's elements and their offsets come from the vendored Vue compiler's parser
  (`webroot/libs/vue/compiler-dom.cjs.js`, which loads in a page too); for a stamped render the server parses the
  template with it and sends a copy with a `data-m-src` offset on each element (F10). Operations are text edits in the
  template block.
- **Insert, Vue**: Quasar components (`q-card`, `q-btn`, `q-input`, `q-select`, `q-table`, `q-tabs` …), the `m-*` widgets
  (`m-form`, `m-text-line`, `m-drop-down`, `m-date-time`, `m-display`, `m-chart`, `m-markdown`, `m-code-editor` …) and
  layout (`row`, `col`, gutters), each with snippets.
- The terminal form (`.tui.json` or a `tui` text) shows beside it, with the worklist's note when it is missing.

### 11.5 Text

Text (CodeMirror on the same buffer) sits beside Design or in its place; a selection follows between them.

### 11.6 Assist in the builder

One write path (D11): the agent's screens arrive as `propose_edit` change sets, previewed on this canvas from the
proposed text (F14) and opened here by "Open in the builder"; `write_ui` with `screen-xml` (§13.5) is the preview of
such a proposal on the drawer's canvas, and the agent's `screen` session sees a screen it is building as text (§13.2).

---

## 12. Data files and new components

- **Data files** open as XML whose element names complete to entities, attributes to the entity's fields, and status
  and enumeration values to rows. Actions: Load this file (the data loader, in process, with the file's own type),
  Load the changed records (only the records whose text changed since the last commit), Check against the database
  (which records differ, as `checkData` reports), Export records to this file (a find over an entity, as Tools' Data
  Export writes). The three-way compare of [Data](proposals/Data.md) is the later version of Load changed.
- **Seed data as rows**: a data file's records also show as a table per entity (the entity's fields as columns, from
  the model), where a row is added or edited in place and written back as its element; the Picklist and Status kinds of
  §8.1 write their rows this way, into `data/<Site>SeedData.xml` of the site component (`type="seed"`).
- **New components**: from `./moq component-create` when the runtime has `moq`, else from the dev-aide's templates:
  **Site** first (D17: a `component.xml` depending on the apps it extends, an `entity/<Site>Entities.xml` ready for
  `extend-entity`, `screen-extend/`, a seed data file, a `service/` directory with a `.secas.xml` ready, and a `test/`
  spec; it becomes the `site-component` when none is set), App (a root under `apps`, its menu, its artifact group and
  seed access, an entity file, a list and a detail screen from the archetypes), REST API, data only, native
  (`Cargo.toml` and `src/` with `submit_component!`, built by `./moq build`), theme, skill pack (skills and prompts for
  agents, no code), and dev tool (in `runtime/dev-component`, loaded only on dev instances: mock services, test data
  screens). Each starts a git repository with an optional remote. The new component loads after a restart, which the
  banner's button does (§6.5).

---

## 13. Assist and agents

### 13.1 The dev agent

- An `AgentDefinition` `dev` ("Dev Assist") seeded by the dev-aide: its addendum `component://dev-aide/prompt/dev.md`
  (`instructionsLocation`), a role group that permits the dev-aide's screens and services, a tool policy with the dev
  tools and writes by approval only, `canvasDialects` `openui,screen-xml,qvue-screen`, `startPath` `/apps/dev`. The
  drawer starts with it on dev-aide pages; elsewhere the agent picker offers it to `DEV_AIDE` users.
- **Skills** in `DA/skill/`, in sections, with the front matter the skill index reads (`unlocks`, `surfaces`,
  `sections`, `screens`). Reference skills: `moqui-dev` (components, artifact kinds, conventions, how a save reloads; it
  unlocks `propose_edit`, as `openui-lang` unlocks `write_ui`), `site-customizing` (D17: "an edit to an upstream
  component is a fork; use extend-entity, screen-extend, seed data or DbForm", with each mechanism's limits word for
  word: what a form merge can place, what `widgets-extend` can anchor to, that a missing anchor is added to the base
  screen and never generated, what DbForm can and cannot change, when a restart is needed), `entity-xml`, `service-xml`, `xml-actions`, `screen-xml` (with the archetypes), `qvue-screens`,
  `eca-rules`, `data-files`, `moqui-specs` and `component-types`. Every rule a check quotes (§5.6, the upstream-edit
  and extension-anchor rules included) is in its section word for word, which a test checks, as for `openui-lang`.
- **Recipes**: procedure skills whose `screens` match app pages and dev-aide views, so the drawer offers them as
  prompts (§13.4) and the agent follows them step by step, extension first: "add a field and show it on its screens",
  "add a list column", "add a filter", "add a related section", "add a button that runs a service", "make a list and
  a detail for an entity", "add a picklist", "add a status flow", "write a spec from my last Try". Each names its
  steps (the extension, the seed data, the label, the spec), what to preview and what to ask the user. They are the
  business user's main interface and the evaluation tasks of §19 ("Add a due date to orders and show it after the ship
  date", against mr-marble's order screens and into my-site, is one).

### 13.2 Tools

| Tool | Class | What |
| --- | --- | --- |
| `artifact` | READ | `find`, `describe`, `refs` and `at` over the index (§7.1); `describe` gives the definition with its file and line |
| `source` | READ | A file's lines (a range, or an artifact's element), its outline and git status; the user's unsaved buffer when the page sent it |
| `check` | READ | §5.6's checks on a file or on proposed text, problems naming rule and skill section; with no file, the last reloads' problems |
| `propose_edit` | WRITE | A change set: edits by artifact (operations of §5.2 on an element an id or path names), by text (an exact old text and its new text, as coding agents edit), or new files; a `target` of `extend` (the default: the site component's extension) or `in-place`; each checked first, an in-place edit of an upstream component a problem while the lock is on (§5.6); an approval (§13.3). The one write path for every agent edit (D11) |
| `try_service` | READ in sim, WRITE for real | §9.4's run; a real run is an approval, as any service call |
| `vcs` | READ, WRITE, EXTERNAL | status, diff and log; commit with its message and files (the card shows them); push (the card names the remote and the commits) |
| `screen` | READ | The existing screen session: the agent sees a screen it is building as text, on the tui model, as it sees any screen |
| `write_ui` | READ | The existing tool, with the dev dialects (§13.5) |

- Classified as LLM-A2A-Assist 4.3 classifies tools; registered by the dev-aide crate with `submit_tool!` and the
  capability `dev.aide`, which a run has only on a dev instance with the dev-aide loaded for a user with
  `DEV_AIDE_VIEW` or above; each tool checks both again when called, and the writes check the user's tier (§3.4): an
  agent of a `DEV_AIDE_EXTEND` user proposes extensions and additive DDL only, and its `vcs` push is Publish to the
  site's remote. `artifact`, `source` and `check` are pinned in the dev agent's policy;
  `try_service` and `vcs` are add-on tools of the lazy catalog, so the conversation's tool list stays the same.

### 13.3 Change sets

- `propose_edit` makes an `AgentApproval` of a new target kind, `file`: the files, each with its base hash and patch,
  bound by the approval's hash (LLM-A2A-Assist 4.8). The card reads the change as artifacts, from the index and the
  patch: "Assist proposes 2 changes in my-site: Entity `Example`: + field `dueDate` (Date). Screen `ExampleDetail`: +
  field `Due` after `Ends`", each row with the few values a business user changes (a title, a place) editable as a
  service call's arguments are, then the files with their counts (+3 −0), and **Preview**, **Accept all**, **Review**
  and **Reject** with feedback. Preview renders the affected screen from the proposed text through F14's overlay on
  the drawer's canvas (the `screen-xml` dialect, §13.5), with the page's parameters; when a change set spans an entity
  and a screen the preview shows the layout and says the data appears after Accept.
- **Accept all** writes the files as proposed when each file's disk hash is still its base and no editor has unsaved
  edits in it; otherwise only Review is offered. The drawer asks the Workspace in its page before it posts the decision, and
  the server checks the hashes on the disk as it writes.
- **Review** is the hand-off, as Edit in screen hands a write to the user's own Save (`approval::handoff`): the
  Workspace opens the change set with each file in an inline merge view, Accept and Reject on each hunk and the text
  editable; Save accepted writes through the normal Save, and that save resolves the approval as executed with what the
  user did ("accepted 3 of 4 hunks; edited OrderServices.xml lines 120 to 131"), which the run reads. Leaving the review
  or rejecting ends it as a hand-off or a rejection ends.
- A file changed since the proposal gets the proposal merged three ways onto its buffer (the base, the user's, the
  agent's); a hunk that does not merge shows as a conflict in the review.

### 13.4 Page state and prompts

- The dev-aide pages add an `editor` part to the drawer's page state: root, path, language, view, unsaved, the element
  at the cursor (`<service verb="place" noun="Order">` at line 120), the selection (at most 2,000 characters), the
  problem count and the artifact id. The tail gives it one line, within its 1,200 characters (LLM-A2A-Assist 7.1).
- Suggested prompts: the recipes of §13.1 by the screens they match (on an app page "Add a field to this form", "Add a
  column to this list", "Add a button here"; in the entity view "Add a picklist"; in the service view "Write a spec
  from my last Try"), and `assist-prompt` rows for the rest: "Explain this artifact", "Where is this used?", "What
  does my site customize here?", "Fix the problems in this file".
- On dev-aide pages the code blocks of an answer get Copy, Insert at cursor, Replace selection and Propose as change,
  through a client registry the drawer gains (`moqui.assist.codeActions.register`).

### 13.5 The dev dialects

Both are `devOnly` in the dialect registry and offered on dev instances only (LLM-A2A-Assist D5 and 4.10). A program
is previewed in place through F14's request-scoped overlay: the proposed text stands at its real location (a new
screen's path under its mount, an existing screen's file, or a `screen-extend` over its base) for that render only, so
relative paths, subscreens and extensions resolve as they will after the save. Nothing is written for a preview.

- **`screen-xml`** (D11): the program is an XML screen file, or a `screen-extend`, as a `propose_edit` change set
  proposes it; the canvas is that proposal's Preview (§13.3). The server checks it (digest and references), as
  `write_ui` checks OpenUI, and renders it through the real macros as the user ∩ the agent into a merged qvt fragment
  for the canvas: the whole render under the read fence, with a time limit and the AT_LLM tarpit's counts against a
  preview that loops (question 9) (its actions and expressions; a write held and shown as "would post updateOrder"), `render-mode text` of client code and any script as placeholders, links shown
  and not followed. The fragment so holds only what the macros write, as any screen render does. Canvas actions are
  the card's: Accept all (a new screen file at its path and mount, or the change set of the existing file), Review, and
  Open in the builder (the proposal becomes the builder's buffer, unsaved).
- **`qvue-screen`** is later (P9): the program is an XML screen block and the SFC blocks of `vue-sfc` (`<template>`,
  `<script>`, `<style>`). The SFC previews in the existing sandbox frame (docs/Assist.md "Vue SFC canvases"), its
  `$bridge` reads answered by the program's own read-only transitions under the read fence and its submits held as
  approvals. Accept writes the `.xml` (with `render-modes="qvue"` and the `render-mode text` naming the `.qvue`) and
  the `.qvue`.
  In a saved `.qvue` screen `$bridge` is a small framework mixin over `moqui.request` and the screen's own transitions
  (`mScreen.path`), so the component's code is the same in the canvas and in the screen (F12).

### 13.6 Agents outside the app

- **Files**: the watcher (§6.3) makes their edits live; `git` and the Workspace's Changes show what they changed. The
  framework ships a skill for coding agents (`fw/skill/moqui-coding-agent.md`, and the same text as a section an
  AGENTS.md can point at): how a save reloads and when a restart is needed, how to run `moqui check` and `moqui refs`,
  extension first (D17) and where a site's extensions go, how to run a spec. For these developers the differentiating
  pieces are, in order, the watcher, `moqui check`, `moqui refs`, the reload reports and MCP; LSP is later.
- **MCP**: on a dev instance with the dev-aide loaded, the `mcp` agent's policy (seeded, dev only) takes `artifact`,
  `source` and `check` (READ, as the user, under the read fence) and `try_service` in sim; `propose_edit` only when an
  admin adds it, since an MCP client usually has the files.
- **Command line**: `moqui refs <id or name> [--in|--out] [--kind k]` and `moqui check [paths] [--extensions]
  [--json]` (§5.6's checks, as `path:line:col: error: message [rule, skill#section]`, the skill section also given as
  its file path and heading so an agent reads the rule with `cat`; `--json` one object per problem; exit 1 on an error
  and 0 otherwise, stable, so it runs as a git pre-commit or coding-agent hook; with no paths the files git shows as
  changed; `--extensions` the anchors of every site extension), on a model-only boot as `moqui types` boots;
  `--server URL` asks a running dev instance, with a login key kept as `moqui tui --server` keeps one.
- **LSP** (later, in packages with `devtools`): `moqui lsp` over stdio for editors and coding agents: diagnostics,
  completion, hover, definition, references, document and workspace symbols, from the same index and checks; a local
  boot or `--server`.
- **The terminal**: the dev-aide's XML screens have terminal forms, so `moqui tui --agent` and `agent-apps` reach search,
  references, ECA rules, problems and repositories.

---

## 14. Tasks

The first version keeps only the current task (the `DEV_AIDE_CURRENT_TASK.<root>` preference, set by hand or from a
task key typed in the commit form) and its trailer in Publish and Commit; the sources, the panel and the apps below are
the git-depth phase (P8), since a developer's tasks live in the team's tools already.

- **A task source** answers the user's tasks: `mantle` (this instance's `WorkEffort` tasks assigned to the user, when
  Mantle is loaded) or `moqui-rest` (another Moqui instance, such as the team's Projects server, over its REST API with
  a user name and an access token kept encrypted). The user picks one in Settings.
- **The panel** (the rail's Tasks, and `dev/Tasks`): my open tasks, those of the projects linked to the current
  repository first (`WorkEffortGitRepository`, when the source knows the repository), each with key, title, status and
  project. Actions: Open (in the configured app), Copy key, Use in commit (a `Task: PROJ-12` trailer, or `Fixes
  PROJ-12`, which Mantle's gitECA rules read when the commit reaches a repository a Projects server hosts or mirrors:
  docs/Git-Hosting.md "Projects and tasks"), Branch (a local branch `PROJ-12-<words>`), Set current (sticky per
  repository: the commit message gets its trailer), and Quick create (title and project).
- **App paths** are conf (§16): a task app names its project, task and new task paths, with `{projectId}` and `{taskId}`,
  and a base URL when it is another instance. mr-marble's Project and Task screens are named the same way.

  | App | Project | Task |
  | --- | --- | --- |
  | IronStone Projects | `/qapps/Projects/Projects/ProjectDetail?workEffortId={projectId}` | `/qapps/Projects/WorkItems/WorkItemDetail?workEffortId={taskId}` |
  | MarbleERP | `/qapps/marble/Project/ProjectSummary?workEffortId={projectId}` | `/qapps/marble/Task/TaskSummary?workEffortId={taskId}` |

- Nothing more: planning, boards and time are the project app's.

---

## 15. Security

- **What it is.** A dev-aide user writes code that the server runs: services, scripts, screen actions, entities. That
  is code execution as the server's OS user, with the user's git credentials. The controls follow from it.
- **Where**: dev purpose only, twice marked (D1). A WARN at startup when a dev instance's HTTP listener is bound to a
  non-loopback address, naming the risk; INFO when the conf says `shared="true"` (D19), which also turns the network
  git commands off until an admin enables them with a token (§4.2).
- **Who**: the tiers of §3.4: `DEV_AIDE_VIEW` reads, `DEV_AIDE_EXTEND` writes inside site components with additive
  DDL, `DEV_AIDE` everything; `DEV_AIDE_APP` for the screens; granted in seed only to `ADMIN`, `DEV_AIDE_USERS` and
  `DEV_AIDE_EXTENDERS`; a header login (an API key) reads but never writes files or runs git; every write is a
  transition with the session token.
- **Configuration and code.** DbForm (per user group, rows in the database, in production, by admins in System) is
  configuration; the dev-aide writes code (files in components, versioned, on dev instances), and the site component
  (D17) is where a site's code goes. The views show both with their origin, so nobody wonders where a field came from.
- **What**: the confinement of §4.1; the git layer's argv rules (§4.2); text size limits; no symlink escapes.
- **Record**: a `DevAideAudit` row for every write (save, create, move, delete, DDL and SQL, git commands that write,
  real Try runs), with the agent's run and approval when an agent proposed it.
- **Agents**: writes only through approvals, decided only in an interactive session (LLM-A2A-Assist D3); push is
  EXTERNAL; MCP and A2A callers never write files unless an admin opts a tool in; the dev dialects' previews run under
  the read fence and draw no client code (§13.5); the `vue-sfc` sandbox rules hold for `qvue-screen`.
- **The rule and its exception** (answered question 1): no code comes in from outside and runs on the server; what runs
  ships in components. Phase 11 kept that even in development (no web eval endpoint). The dev-aide is the main
  exception, on dev instances only; Try's snippet is the nearest thing to a console, in sim by default.
- **A sandboxing question.** What keeps the exception to dev is how far the sandboxing can be trusted, not the tools.
  This plan leans on the layers there are: the read fence, the sim overlay and its fence, the `vue-sfc` frame, QuickJS
  runtimes with memory and time limits, and the locked-down template renderer. As they improve and earn confidence,
  particular things may be allowed in particular contexts (a purpose, a user, an agent); each such step is its own
  decision, naming the thing, the context and the layer that contains it.

---

## 16. Data and configuration

**Entities** (`moqui.dev`, in `DA/entity/`):

| Entity | Holds |
| --- | --- |
| `DevAideAudit` | A write: user, date, action (save, create, move, delete, ddl, sql, git command, try), root, path, hashes before and after, detail (JSON), agent run and approval |
| `DevAideRoot` | A root added in Settings, or the settings of a discovered one: location, label, read only, publish branch and whether it is required (question 15) |
| `DevAideTaskSource` | A task source: kind (`mantle`, `moqui-rest`), base URL, user name, access token (encrypted), project filter |
| `DevAideTaskApp` | A task app added in Settings, as the conf's `task-app` |
| `DevAideCredential` | Shared mode (D19): a deploy token for a root's remote, or a user's HTTPS token, encrypted, set by an admin |

Preferences: `DEV_AIDE_EDITOR` (font size, keymap, wrap, tab size, whitespace), `DEV_AIDE_UPSTREAM_EDITS` (D17's
toggle, `DEV_AIDE` users only; the conf's `allow-upstream-edits` is its default), `DEV_AIDE_ARRANGEMENT` (guided or
workspace, D18), `DEV_AIDE_PUBLISH_BRANCH` (question 15: an admin sets it on the User screen in System; it is not
shown in the user's own preferences), `DEV_AIDE_TASK_SOURCE`, `DEV_AIDE_CURRENT_TASK.<root>`. The browser keeps open
tabs, layout and unsaved text (IndexedDB) per user. File edits proposed by agents are `AgentApproval` rows (target
kind `file`); the index and the observed calls are the framework's `moqui.devtools` rows (§7.1), since the index is
the framework's (D8).

**Conf**: a `dev-aide` element (added to `moqui-conf-3.xsd`) in the component's `MoquiConf.xml`, and `dev-reload` in the
framework's:

```xml
<dev-aide site-component="my-site" shared="false" allow-upstream-edits="false"
        git-command="git" git-read-timeout="15" git-network-timeout="300" max-edit-bytes="2097152">
    <root location="/home/me/work/notes" label="Notes"/>
    <exclude pattern="runtime/db/**"/>
    <task-app name="projects" title="IronStone Projects"
            project-path="/qapps/Projects/Projects/ProjectDetail?workEffortId={projectId}"
            task-path="/qapps/Projects/WorkItems/WorkItemDetail?workEffortId={taskId}"/>
    <task-app name="marble" title="MarbleERP" project-path="/qapps/marble/Project/ProjectSummary?workEffortId={projectId}"
            task-path="/qapps/marble/Task/TaskSummary?workEffortId={taskId}"/>
</dev-aide>
<dev-reload debounce-ms="300"/>   <!-- on when instance_purpose is dev; enabled="false" turns it off -->
```

---

## 17. Framework changes

| # | Change | Where |
| --- | --- | --- |
| F1 | `purpose` on `component-dir`, `component` and `component.xml`; the effective purpose before discovery; the WARN per component left out; the dependency check; `dev-component` in the default `component-list` | `core/context/component.rs`, `factory.rs`, `fw/conf/MoquiDefaultConf.xml`, `moqui-conf-3.xsd` |
| F2 | The Dev package as `./moq`'s default (feature `dev`, Cargo profile `develop`, `target/develop/moqui`) and `--lite` for Lite; the `devtools` feature (the dev crates, the watcher) in Dev and Debug; the dev-aide's crate a workspace member linked under `devtools`, a user's dev crates through the generated crate; `--dev` for the component tasks, `runtime/dev-component` in the git tasks; the docs and help that name the default package | `moq`, `.cargo/config.toml`, `Cargo.toml`, `crates/moqui-host/Cargo.toml`, `crates/moqui/Cargo.toml`, docs/Packaging.md, README.md |
| F3 | Reload the service registry: rebuilt from every service file as `lint` replaces it, the changed file's new text included, and swapped in; a per-file merge (its services replaced or removed, includes and implements resolved again) only if the rebuild measures slow | `core/service/define.rs` |
| F4 | Rebuild the SECA, EmECA and gitECA rule sets, and the EECA rules | `core/service/eca_rules.rs`, `core/entity/load_feed/eca.rs` |
| F5 | Staged behind the restart (§6.5): the entity model swappable (an `Arc` behind a lock in place of `SharedModel`'s owned model), a relink from changed files, the caches of changed entities dropped, their table check run again; taken up in the modeler's phase (P5) when the restart proves tiresome | `core/entity/share.rs`, `facade.rs`, `derived.rs`, `meta.rs`, `moqui-db` model |
| F6 | `core/devtools/reload.rs`, and the dev watcher under `devtools`; `ScreenFacade::forget(location)`; single-entry drops of text, template and script caches; REST roots and skill sync on demand; `DevReload` notifications; restart from the app (a re-exec, §6.5) | `core/devtools/`, `core/screen/cache.rs`, `core/script/`, `crates/moqui-host/src/` |
| F7 | The lossless XML layer and its vectors, Rust only; `xml_edit` exposed in the WASM build of `moqui-lib` for the views, and `POST xml-op` until then | `lib/xml_edit.rs`, `crates/moqui-lib/testdata/xml-edit-vectors.json`, `crates/moqui-browser` |
| F8 | The XSD digest and the reference rules, served to clients | `core/devtools/schema.rs`, `refs.rs` |
| F9 | The CodeMirror bundle; `WebrootCode.js` with `m-code-editor`, `m-file-tree` and `m-diff-view`; `text-area editor-type="code"` with `language` | `webroot/libs/refresh.mjs`, `webroot/js/`, `fw/xsd/xml-form-3.xsd`, the qvt macros |
| F10 | Source stamps: widget positions kept through `XmlFragment` on dev instances, with the file that wrote each element (the base screen, a `screen-extend`, a DbForm), `data-m-src` in `ScreenQvt.js` for a stamped render, a stamped `.qvue` template, `frame-ancestors 'self'` for those renders | `core/screen/definition.rs`, `fw/template/screen/ScreenQvt.js`, `core/screen/` |
| F11 | The artifact index and the observed calls, in the `moqui.devtools` tables (question 11) | `core/devtools/index.rs`, `core/context/artifact.rs`, `fw/entity/DevToolsEntities.xml` |
| F12 | Assist: approval target kind `file` with its hand-off to the editor and its card as artifacts with Preview (§13.3); the page state's `editor` part in the tail; `moqui.assist.codeActions`; the `$bridge` mixin for `.qvue` screens (P9). The shell: a page may keep the app drawer closed while it is the page; "Customize this screen" in the page menu and the drawer's header on dev instances (D18) | `core/llm/approval/`, `core/llm/assist/state.rs`, `webroot/js/WebrootAssist.js`, `WVQ` |
| F13 | `moqui refs` and `moqui check` (`--json`, `--extensions`, a stable exit code, the changed files by default) and the coding-agent skill; later `moqui lsp`, under `devtools` | `crates/moqui-host/src/`, `fw/skill/` |
| F14 | A request-scoped location overlay: a screen renders from unsaved or proposed text standing at its location for one request (the Preview of §13.3, the builder's buffer of §11.2, the dialects of §13.5), in P4; services from proposed text later | `core/context/resource.rs`, `core/screen/cache.rs` |
| F15 | Later: Yjs out of the Tiptap bundle into its own, for co-editing code with `y-codemirror.next` on the existing rooms | `webroot/libs/refresh.mjs`, `core/collab/` |
| F16 | `screen-extend` strengthened (D17): `widgets-extend where="prepend|append"` inside a named container (the XSD's FUTURE); a form merge that places a merged field in the form's `field-layout`, and a merged `form-list-column`, by `after="fieldName"`; the order of several components' extensions of one screen stated (component load order, as entities); stamps carry the writing file (F10); a `moqui lint --screens` rule for unnamed containers in base screens, so upstream authors add anchors | `core/screen/definition.rs`, `core/screen/form.rs`, `fw/xsd/xml-screen-3.xsd`, `core/script/screen_lint.rs` |

---

## 18. Phases

Each phase ends with its check; nothing in a phase needs a later one. The order is the review's (D17 to D19): the
framework first, then the thinnest path from an app page to a published extension, with the agent, then the visual
tools, then the service writer, then git depth. The milestone after P4 is "add a field to an upstream entity through
my-site, show it on a list and a detail, see it live, publish", with the agent; after P6 the same by hand. P5 and P6
are independent of P4 and may run beside it.

- **P1a. Framework groundwork** (F1 to F4, F6, F8, F13, F16; F5 staged). The Dev package as the default build and
  `--lite`, landed first as its own commit with the README, docs/Packaging.md and `moq`'s help, since D16 changes every
  contributor's workflow; dev-only loading; the reload API and watcher over what reloads today, the service registry
  rebuild and the ECA rule sets, restart from the app; the schema digest and reference rules; `screen-extend`
  strengthened; `moqui check` and `moqui refs`. **Check**: `./moq build` makes the Dev package (`moqui version` lists
  `git`, `ssh` and `devtools`) and `--lite` the Lite package as before; a boot as production leaves the dev-aide out
  with its WARN, and as dev loads it; on a dev instance a service and a SECA changed on disk take effect without a
  restart, an entity change shows "restart to apply" and the restart button brings the server back with the column
  made, and a file that does not parse keeps the old definitions with a problem; a `screen-extend` adds a column after
  a named one and a field into a layout; `moqui check --json` reports a reference error with its skill section and
  exit 1; the digest reads all 12 XSDs; a coding-agent session (Claude Code or the like) edits the example component
  with the watcher and `moqui check`, and what it needed goes into the coding-agent skill; the path confinement and
  the git command run on Windows.
- **P1b. Client groundwork** (F7, F9). The CodeMirror bundle and `m-code-editor`, `m-file-tree`, `m-diff-view`; the
  lossless XML layer in Rust with `xml-op` (or the WASM module) and its vectors. **Check**: the vectors pass; an
  `m-code-editor` in a form saves; a visual-view operation round-trips through `xml-op` without touching other text.
- **P2. The Workspace and the site component.** The component, access and tiers, app root and menu; roots with
  upstream read-only and the toggle; the site component and the Site template; the tree with status, editor tabs in
  Text, the save pipeline with its report, checks (digest, references, upstream edit) and the Problems panel; outside
  changes; plain directories; search in files; Go to file; the git layer's reads, stage by file, commit with the
  trailer, fetch, pull, push, Switch and New branch, Publish with the publish branch and merge request settings (per
  repository and per user); shared mode's author, token and presence. **Check**: a
  Chromium smoke edits a service file in a site component, saves, reads the report and calls the service's new
  behavior; a save on an upstream file is refused with "Extend in my-site" and allowed with the toggle; a plain
  directory shows the disabled version control; a save over a file changed on disk opens Compare; Publish commits and
  pushes to a local bare remote, and to the publish branch when an admin set one for the user; specs on scratch
  repositories with git isolated from the desktop (no askpass, no
  credential helper, no global config) show the author and committer identities.
- **P3. The index** (F11). The index in its tables with origin, extension anchors and the observed calls; Search, the
  palette group,
  Go to artifact, go to definition and references in the editor; the reference lists and `dev/Artifacts/Refs`; the
  Customizations list and Home's summary; the pull-time extension check and `moqui check --extensions`; the MCP tools
  `artifact`, `source` and `check`. **Check**: the corpus indexed in its target time and a synthetic corpus ten times
  its size measured, with the per-file update and the find times recorded; a restart keeps the index; references match
  a fixture set of known artifacts; a renamed form in a fixture base screen shows its site extension as broken after a reload; the MCP
  tools answer from the SDK client.
- **P4. Agents** (F12 but `$bridge`, F14 for screens). The dev agent, its skills and recipes, tools and policy; change
  sets with the card as artifacts, Preview through the overlay, Accept all and Review in the editor; the `screen-xml`
  dialect as a proposal's preview; the page state and prompts; code actions; "Customize this screen" reaching the
  drawer's recipes on app pages. **Check**: evaluation tasks on the scripted fake provider with recordings (add a field
  through my-site and show it, add a column, find where a service is used, write a spec from a Try), a smoke of a
  proposal previewed, partly accepted and saved, and `moqui test dev-aide --agents --live` (a key from the environment,
  nightly and before the phase closes): the recipes' tasks scored by `moqui check` clean, the spec passed and the diff
  confined to the expected files, against a pass rate, with passing runs recorded into the replay set.
- **P5. The entity modeler** (F5 if taken up). The entity view with origin and extension first, field kinds with
  Picklist and Status writing seed rows, saving with additive DDL and the restart, the destructive changes for the full
  tier on a typed confirm, the view-entity editor; the seed data row table. **Check**: specs for field and relationship
  edits that keep comments and order; adding a field to an upstream entity writes an `extend-entity` in my-site and
  adds its column; a Picklist writes its rows and the drop-down shows them; the extend tier never sees a destructive
  change.
- **P6. The screen builder** (F10). The screen tree and its moves; the stamped canvas with the writing file, Design
  from the buffer through the overlay, extension mode with the Extension points list, inserts by intent, properties
  and the outline on the Vue tree; "Customize this screen" opening the builder from an app page. **Check**: a smoke
  from an app page adds a column to an upstream list through a `screen-extend` and sees it on the canvas before and
  after Save; a field inserted into a form with a `field-layout` lands in the layout; an unnamed anchor is reported
  with its nearest named one.
- **P7. The service writer and ECA rules.** The service view; Try in sim and Save as spec; the ECA lists and view; the
  logic composer on the Vue tree, measured as §9.2 says, Tiptap a spike only on a miss. **Check**: the composer's
  operations round-trip through the vectors and meet the measures; a Try in sim lists its rows and its spec passes; a
  new SECA in my-site runs after its save.
- **P8. Git depth and tasks.** Hunk staging, Branches, Tags, Stashes, Compare, conflicts, blame, cherry-pick and
  revert, Remotes, Repositories; `push --force-with-lease` behind its confirm; the task sources and panel (§14).
  **Check**: specs on scratch repositories for each write; a smoke resolves a conflict after a pull.
- **P9. The rest of the canvases and LSP** (F13's LSP, F12's `$bridge`). The Artifact Browser canvas; `.qvue` design
  mode and the `qvue-screen` dialect; `moqui lsp`. **Check**: a smoke expands both sides two generations and flips
  orientation; an editor's LSP client gets diagnostics, completion and definition on the example component.
- **P10. Later.** F5 if not taken up in P5; F14 for services (a Try on proposed text); a trace of a Try in sim on the
  composer's blocks; co-editing code (F15); a TypeScript checker; hosted roots (`git://`, a commit per save); the
  three-way data load; the rebuild from the app; tests of ECA rules against recorded contexts; "Convert to site
  extension" for DbForm rows.

**Built so far** (Oct 24, 2026): P1a, P1b and P2, with these differences from the text above.

- **Watcher and restart.** The watcher starts with a `moqui` run's server, not a test's. The dev-aide hears reload
  reports from its first service call, as it has no tool factory: a binary without `devtools` still boots with its
  screens. The non-loopback WARN is the host's, and binding is the new `webapp_http_bind` (empty is every address).
- **Checks.** Schema problems are warnings, long term: the XSDs trail what the framework reads. References, expressions
  and parse failures are errors.
- **Editor.** `@replit/codemirror-vscode-keymap` is left out of the CodeMirror bundle: it carries no license text. The
  default keymap has the common VS Code bindings.
- **XML edits.** The lossless layer is reached through `apply#XmlOps` only; the WASM module is not built.
- **Settings.** The site components and network git are stored in `DevAideSetting` rows.
- **Not built yet:**
  - `moqui check --server`;
  - shared mode's presence, save announcements and `DevAideCredential` tokens;
  - the System User screen field for `DEV_AIDE_PUBLISH_BRANCH`, which is a preference an admin can set;
  - opening a merge request on a hosted origin: Publish names the branch to merge;
  - keeping the app drawer closed on the Workspace, which takes the window's height by CSS;
  - unsaved text is kept in `localStorage`, not IndexedDB.

**P3, the index** (Oct 28, 2026), with these differences:

- **Tables.** `DevArtifactFile` (each file's hash, and whether its rows are stale because it no longer parses) joins the
  three of §7.1. An extension is a reference: an `extend-entity`'s entity, a `screen-extend`'s base screen and each
  anchor it names, an ECA rule's service or entity, a DbForm's form are `extend` or `anchor` references, so an origin is
  the defining component and the incoming `extend` references, and a broken extension is an unresolved one. The search
  key is the short name's segments; a qualified name also matches the full name.
- **What is read.** Every component's entity, service, ECA, REST, screen, `screen-extend`, data (XML), script and skill
  files, and as rows service jobs, data documents, email templates and DbForm forms. A script's names come from the
  string literals of its service and entity calls by pattern, not by oxc. A `link` or response URL is a reference when it
  is a transition of the same screen; relative screen paths, service parameters and artifact group patterns are not
  indexed yet. Screen anchors are every named widget outside a form, as `check --extensions` read them before.
- **When.** A `moqui run` server builds it in the background at start (`Artifact index built in …` in the log); a test's
  server builds it at first use, or with `rebuild#ArtifactIndex`. Each reload's files are read again through
  `on_reload`; a pull reloads the files it changed at once and answers the site's extensions it broke.
- **Measured** (release build, embedded Postgres, a developer laptop, `it::devaide::the_index_scales`): the corpus of this
  repository's components, 1,659 files, 20,652 artifacts and 30,563 references, builds in 1.4 s (reading 0.2 s, writing
  1.15 s); a per-file update of `OrderEntities.xml` (277 artifacts) takes 41 ms; a find 6 ms. The synthetic corpus ten
  times its size (16,590 files, 206,520 artifacts, 305,630 references) builds in 18.8 s, updates a file in 198 ms and
  finds in 157 ms: linear, with the write the cost. Rows go a hundred files to a transaction with the data loader's
  pipelining; a batch that fails is written again file by file.
- **Command line.** `moqui refs` and `moqui check --extensions` read the files into a memory index on the model-only boot
  (about 0.5 s for the corpus); `refs` takes ids, names and files, `--kind` over the index's kinds, and `--json`.
- **Editor and screens.** F12 and Mod+click go to a definition, Shift+F12 lists the references under the editor, and
  Ctrl+Shift+O goes to an artifact; `dev/Artifacts` has Search, References (the List view) and Customizations; Home shows
  "Site extensions: n, m broken"; the shell palette has an Artifacts group.
- **MCP.** `artifact`, `source` and `check` are tools of the dev-aide's crate with the `dev.aide` capability and
  `ToolSpec.mcp`; the MCP server lists them for a caller with a Dev AIDE permission (`it::mcp::the_dev_tools_over_mcp`,
  over HTTP rather than the Python SDK script).

**P4, agents** (Oct 28, 2026), with these differences:

- **The dev agent** (`DevAideAgentData.xml`): Dev Assist, `startPath` `/apps/dev`, `canvasDialects` `openui,screen-xml`,
  its policy pinning `artifact`, `source`, `check`, `propose_edit` and `try_service` (`vcs` stays in the lazy catalog).
  The `dev.aide` capability goes to a run of an agent whose policy pins a dev tool, for a Dev AIDE user on a dev instance,
  so the general Assist does not carry the dev tools. No role group: the user's own permissions bound it. The skills and
  the nine recipes (`skill/`, `skill/recipes/`) are for the dev agent only (`agents: [dev]`); the recipes reach the drawer
  through the prompts the skill index already offers for a page (`screens`), so the crate has no prompt provider of its
  own.
- **Change sets.** `propose_edit` takes files by XML operations, exact replacements or a whole text; every file is
  checked first, and an error (an upstream edit while the lock is on among them, rule `upstream.edit`, skill
  `site-customizing#Extend, do not fork`) comes back with nothing proposed. The approval is target kind `file`; Accept
  writes through the save pipeline (`workspace::save_text`, audited with the run and approval) after reapplying the
  change to the file as it is now, which is the three-way merge: the change's own operations or replacements on the new
  text, refused when they no longer apply. The card's editable values are the titles the proposal adds.
- **Preview** frames the app page with the proposal standing at its files' locations: a `moquiPreview` token
  (`approval:<id>` or `canvas:<artifactId>`), carried as `X-Moqui-Preview` by the frame's later requests, answered by
  F14's overlay under the read fence; the frame runs no drawer or heartbeat. The fence refuses a write (a 500 for that
  request) rather than showing "would post"; there is no separate time limit or tarpit count.
- **Review** opens the Workspace on the change set: each file in CodeMirror's unified merge view with Accept and Reject
  on each chunk; Save accepted saves the files, then `complete#Review` ends the hand-off, on a thread of its own so its
  decision commits before the run resumes.
- **`screen-xml`** is a dialect whose program's first line names its location and the page that shows it; the canvas is
  that page framed with the program in place. Open in the builder waits for P6.
- **Customize this screen** is a `page-action` shell provider in the drawer's header on every app page, opening Dev
  Assist with the recipes for the page; the shell's page menu has none (it has no page menu yet).
- **Try in sim** is the agents' `try_service` on `approval::preview::run_in_sim`; `moqui test` now starts the sim overlay
  as a server does. The Try panel and Save as spec are P7.
- **Evaluation**: four tasks in `test/agent/dev.agent.js` with hand-authored recordings, all passing on replay; a live
  run is `--llm-profile` (there is no `--live` flag), not yet run against a model. The Chromium smoke
  `a_dev_agent_change_set_is_previewed_reviewed_and_saved_in_chromium` covers the card, Preview, Review with a rejected
  chunk, and the approval's outcome.

**P5, the entity modeler** (Oct 28, 2026), with F5 taken up and the visual views' groundwork, with these differences:

- **The outline is the server's.** `moqui_lib::xml_edit::outline` reads the elements with the paths the operations take
  (comments, CDATA and processing instructions not counted), their attributes as values, lines and UTF-16 spans;
  `apply#XmlOps` answers it with the new text and `get#XmlOutline` is new, so one parser defines what a view edits. Lezer
  keeps highlighting. Both services, and the schema digest, are for any dev-aide permission (they read and write nothing).
- **Views.** A registry by the file's root element offers Text and the views; Text stays mounted beneath a view, so the
  two share one undo history (Mod+Z in a view), and `m-code-editor` keeps each tab's state by key across tab switches. A
  view may open companion buffers in any root (the site's entity file, its seed data file), made on their first save;
  Save writes them by kind with the tab. The guided arrangement is `dev/Entities/Edit?artifact=`: the Workspace's client
  with no tree or tabs, and Open in Workspace.
- **F5.** `SharedModel` holds an `Arc` behind a lock; each facade call takes the model once, so a find in flight keeps the
  one it started with. `EntityFacade::relink` reads every entity file again with the configuration kept as text, swaps
  the model in, drops what was read from the old one (definitions built at runtime and every cache kept by definition,
  the read shapes and their SQL, the views by member, the list and count caches, data documents, what is known of the
  tables, the EECA index) and runs the table check of the changed entities whatever the group's add-missing settings,
  answering its DDL. A component's `entity` directory that appeared since the start (a site's first entity file) is read
  from then on. The reload of an entity file relinks, clears the screen definitions (forms over the entity), and reports
  the DDL; a file that does not parse, or a model that does not link, keeps the old model with "restart to apply".
- **The destructive changes** are planned by the framework (`plan_ddl`: drop a column no field names, change a column's
  type, copy a renamed field's old column into its new one and drop the old, drop and create the table) and run by the
  dev-aide for the full tier only, after the table's name is typed back, statement by statement through `run_sql`, each
  audited (`DAA_DDL`). A type change on SQLite says to recreate the table.
- **The view.** Fields show the server's merged definition with the target element of the buffer over it, so an edit shows
  before Save (marked unsaved); an attribute of an upstream field becomes an override field of the site's
  `extend-entity`. Remove is offered for the target's own fields, with their uses from the index first; reorder is a
  drag. Picklist and Status write the field and its relationship as one operation list (one undo step) and the seed
  records into the site's seed data file, which Save loads (`load#SeedFile`, audited `DAA_LOAD`). "Show it on a form"
  lists the screens using the entity and opens them in the Workspace.
- **Not built:** the header's panel of the entity's other attributes from the digest (edited in Text); a label in another
  language to the L10n messages; the view entity's condition editor (Text) and its member diagram (P9, with the
  pan-and-zoom layer); the SQL of a view entity's Run (its first rows show); a link to Tools' Data Edit; data files' Load
  the changed records, Check and Export (§12; Load this file is built).

**P6, the screen builder** (Oct 28, 2026), with these differences:

- **Stamps (F10).** On a dev instance each widget element of a screen keeps its file, line and column as an `mq-src`
  attribute through the fragments (a `screen-extend`'s merged elements their own file; a form merge's fields theirs,
  the form keeping the base's), and the walker's generated code names a stamped element to `sri.stamp` before its macro
  runs; the form macros do so for each field's box and list cell. A render that asks (`moquiAide=source` or
  `X-Moqui-Aide: source`, a Dev AIDE user on a dev instance) writes the pending stamp on the first element tag written,
  as `data-m-src` (a `template` passed over), and answers `frame-ancestors 'self'`; MoquiLib carries `moquiAide` from a
  frame's URL on its requests as it does `moquiPreview`. Stamped: the containers, boxes, rows and columns, panels,
  dialogs, menus, trees, links, images, labels and the other widgets that write an element of their own, forms and their
  fields; a section writes none and is not. DbForm fields are not stamped yet, and the server-static path neither.
- **The canvas** renders the tab's buffers through F14: `put#CanvasBuffer` keeps a user's unsaved texts under a buffer
  id, and the frame's `buffer:<id>` token renders them (an empty buffer renders the files), under the read fence. A
  frame whose source changes no longer warns of the requests it cut off.
- **Extension mode** writes form merges (`form-list` and `form-single` of the form's name, the field with `after=`) and
  `widgets-extend` (after, before, or append inside a named container) into the site's `screen-extend` of the screen,
  a companion buffer made on its first save, with Replace section and Override transition (the base element copied in
  to change there) and Actions before and after (`actions-extend`); in place the field goes after the selected one with its `field-ref` in the
  `field-layout` or a `form-list-column` of its own. A site's first `screen-extend` file now drops the cached screen it
  extends (the screen facade's `forget`), as no source of the screen named the component's missing directory.
- **"Open in screen builder"** is a `page-action` with a `screen` (new: it opens that screen of the app with the page's
  path and query as `page`), in the Assist drawer's header beside "Customize this screen"; `dev/Screens/Design?page=`
  resolves a shell page under the apps root. The entity view's "Show it on a form" opens the screen in the builder.
- **The tree** is `dev/Screens`, a list a level at a time (Children goes down) with how each screen is mounted (the
  screen facade's new `mounted_by`: directory, item, conf mount, row), its menu index, default, modes and subscreens, and
  New subscreen from a gallery archetype; a list, so the terminal reaches it.
- **The Outline** is the block tree (`DevAideBlocks.js`, SortableJS nested groups, Alt+Up and Down, Delete), on the
  screen file's widgets in place or the `screen-extend`'s elements in extension mode; Properties edits the selected
  element's attributes from the schema digest.
- **Not built:** Move, Remove, Set default and Reorder in the tree (Text, Properties and the Workspace's rename do
  them); Add filter, Add section, Add button and Add row action; double-click to edit a title in place; "Add an anchor here"
  (the full tier names the element in Text with the upstream toggle on); the archetype's gallery preview; the
  `screen-xml` canvas's "Open in the builder" carrying the proposal as the builder's buffer.

**P7, the service writer and ECA rules** (Oct 28, 2026), with these differences:

- **The framework** gains `service::info::service_detail` (the parameters nested with their checks, those
  `auto-parameters` imply marked, an entity-auto service's from its entity), `script::generated_source` (an actions
  block's JavaScript, each statement after its element) and `devtools::eca::rule_list` (every SECA, EECA, EmECA and
  gitECA rule with its file and line, read in the loads' order with their replacement by id and their priorities, each
  replaced rule after the one that replaced it). The rule holders themselves are unchanged: the list reads the files.
  The service info of an entity-auto service is the definition a call makes for it.
- **The service view** has the header's settings, the parameters (Add and Remove on the declared ones), the
  implementation (inline actions in the composer; a script service's file opened in the Workspace, not a second buffer of
  the view), the SECA rules with Add (into the site's `service/<Site>.secas.xml`, a companion buffer), what uses it and the
  last spec run's calls (the coverage column of `dev/Services`, from Run spec here, not `moqui test --coverage`).
- **Try** runs in the sim as the user (`try#Service`), or for real behind a confirm, audited (`DAA_TRY`); the agent's
  `try_service` keeps its own code over the same `run_in_sim`. The sim gives no log lines. A snippet (`try#Snippet`) runs
  XML Actions or JavaScript in the sim, its `result` its last message. **Save as spec** appends a feature of `call`,
  `then` and `checkData` to `test/<Services file>.spec.js` of the service's component, or of the site's when the
  service is upstream; **Run spec** runs the file in process inside the sim (`testkit::run_spec`), so it writes nothing,
  rather than a `moqui test` child process.
- **The composer** is the block tree of P6 with a sentence table over the XML Actions elements (each chip an attribute
  the XSD gives the element, which a page test checks against the XSD), chips that edit one attribute (entity and
  service names from the index), the palette by §9.2's groups with snippets, the names known before each block warning on
  an unknown one, a service-call's in-parameters added as `field-map`s, a script's popup editor, and Generated. Measured
  in Chromium: 300 blocks drawn in about 10 ms; the nested drag's moves and a chip edit are in the shared vectors
  (`composer: ...` in `xml-edit-vectors.json`). Tiptap was not needed. It edits the actions of services and ECA rules;
  screen and transition actions are edited in Text.
- **ECA**: `dev/Eca` lists the active rules with filters; the ECA view of a rules file lists its rules with a rule's
  attributes from the digest, its condition as an expression, its actions in the composer, and Add, Remove and Move.
  Not built: a rule's condition against a recorded context.

---

## 19. Tests

- **Framework, Rust**: discovery by purpose; each reload kind, including a failing file; the service registry rebuild;
  the `screen-extend` merges of F16 (prepend and append, a field into a layout, a column after another, two
  components' order); the restart re-exec; the model swap under concurrent finds (when F5 lands); the lossless XML
  vectors; the digest of every XSD; the reference rules on fixture files; the index's references, origins and
  extension anchors, its per-file replacement after a reload, and that a restart keeps it.
- **Client, node** (run by `cargo test` when `node` is on the path, as the framework's are): the `xml-op` client (and
  the WASM module when built) against the vectors; the outline from Lezer; the browser's layout; the composer's sentence
  table against the digest; the change set review's merge; the card's artifact summary from a patch.
- **dev-aide specs** (`moqui test dev-aide`): confinement (traversal, symlinks, `.git/`), the tiers (an extend user
  refused an upstream write and a destructive DDL, allowed an `extend-entity`), saves with a stale base, audit rows,
  git operations on scratch repositories with git isolated from the desktop (author and committer identities), Try in
  sim, Save as spec, a Picklist's seed rows.
- **Chromium smokes** in the `gate3_browser.rs` style: per phase, as §18 names them, with light, dark and 390 px
  screenshots for the reading views.
- **Agents**: the evaluation tasks of P4 on recordings (`moqui test dev-aide --agents`); the same with a live model
  (`--live`, a key from the environment; nightly and before P4 closes; scored by `moqui check` clean, the spec passed
  and the diff confined to the expected files, against a pass rate; passing runs recorded into the replay set), since
  the Assist runtime has only ever run against hand-authored recordings; and the MCP SDK client.
- **Platforms**: the path confinement and the git layer's specs run on Windows in CI from P1a.

---

## 20. Risks and open questions

1. **Eval in development.** Phase 11 decided against a web eval endpoint even in development. The dev-aide is code
   execution on dev instances by design (D15), and Try's snippet is close to a console. Proposed: accept it for dev
   purpose with `DEV_AIDE`, keep no separate console endpoint, and run snippets in sim by default.
   - Answered Oct 24, 2026: yes. It is the main exception to the rule that no code comes in and runs on the server. It
     is mostly a sandboxing concern, and as the sandboxing improves and earns confidence the rule will likely be relaxed
     for particular things in particular contexts (D15, §15).
2. **The entity model swap** (F5) touches about 20 places that borrow the model and some 30 signatures that take
   `&EntityModel`; a relink must keep finds in flight on the old model. Proposed: an `Arc` swap taken once per facade
   call; if it proves wide, the first version relinks only on the dev-aide's explicit save and says "restart for the
   rest".
   - Reviewed Oct 24, 2026: restart from the app (§6.5) is the first version for entity changes; F5 is staged to the
     modeler's phase (P5) and taken up when the restart proves tiresome in use.
3. **Tiptap for blocks** (D10) is unproven for nested, attribute-heavy trees; P7's spike decides, with SortableJS and a
   Vue tree as the fallback on the same operations.
   - Reviewed Oct 24, 2026: flipped. The Vue tree over SortableJS is the engine and Tiptap the time-boxed spike, both
     against the measures of §9.2 (D10).
4. **Where the dev-aide lives**: this repository (proposed: it moves with the framework's internals) or its own
   repository fetched with `./moq component-get`.
   - Answered Oct 24, 2026: this repository. `runtime/dev-component` is to the framework's dev components what
     base-component is to its others (more will come), and a component directory as `runtime/component` is, for a
     user's own (D1, §3.1).
5. **Production binaries**: whether Maximal links the dev crates by default (proposed: no).
   - Answered Oct 24, 2026: Lite has neither git nor ssh, so a new Dev package becomes `./moq`'s default: Lite with git,
     ssh and the dev tools; `--lite` builds today's Lite (D16, §3.3).
6. **Team dev servers**: several developers on one dev instance get conflicts by hash and Compare in the first version;
   co-editing waits for F15.
   - Decided Oct 24, 2026: a mode from the first version (D19): the author from the logged-in user, network commands
     behind an admin's token, presence and announced saves; one working tree with Compare on conflict; co-editing F15.
7. **Tasks**: whether `moqui-rest` against a team server is enough, or GitHub and Gitea issues are wanted as sources too.
   - Reviewed Oct 24, 2026: deferred with the panel to P8 (§14); the first version keeps the current task's trailer.
8. **Git operations**: whether rebase, reset and force push belong in the first version (proposed: only
   `--force-with-lease` behind a confirm).
   - Reviewed Oct 24, 2026: none in the first version; `--force-with-lease` behind its confirm with the git-depth phase
     (P8).
9. **Agent previews**: `screen-xml` runs agent-written server actions under the read fence and a time limit as the user
   ∩ the agent; that reads what the agent's `data` tool can read already, but a preview's loop costs CPU until its
   limit. Proposed: the limit plus the AT_LLM tarpit's counts.
   - Answered Oct 24, 2026: agreed (§13.5).
10. **JavaScript types**: completion from the `ec` digest and names may be enough; a TypeScript worker over
    `moqui types` costs a few MB. Proposed: later and optional.
    - Answered Oct 24, 2026: later. The framework may take TypeScript for scripts generally, which would put the
      TypeScript tools in the binary; a checker would then be the server's, not a browser worker (§5.4).
11. **Scale**: the index for the corpus of §2 should be tens of MB; a much larger set of components needs measuring.
    - Answered Oct 24, 2026: how far it scales is to be tested, not assumed. The index lives in database tables shaped
      for what the app asks, so the bulk of the indexing, sorting and searching is the database's (D8, §7.1); P3
      measures the corpus and a synthetic one ten times its size.
12. **Platforms**: Windows paths, line endings and the git command's quirks need a pass before anyone uses it there.
    - Reviewed Oct 24, 2026: in P1a's check and CI, not later.
13. **IronStone Projects**: whether Projects adopts `m-file-tree`, `m-code-editor` and `m-diff-view` in its Code screens
    is its own plan's decision (`boss/plan/design-projects.md` §7); this plan only puts them in the framework.
    - Confirmed Oct 24, 2026: Projects' Code stays separate and different, with only the safe repository features; the
      two share what is underneath and stay independent (D7, §4.10).
14. **Anchors in upstream screens** (D17, F16). `screen-extend` needs named forms, containers, sections and
    transitions, and many upstream screens leave containers unnamed. F16's lint asks upstream authors to name them;
    whether the framework should also give stable generated names (by path and position, which an upgrade may move) is
    open. Proposed: names by lint, no generated anchors, and the builder's "nearest anchor is X".
    - Answered Oct 24, 2026: agreed, no generated anchors. Anchors, and extension points in general, are added to the
      base screen as an edit of the base screen; the builder offers "Add an anchor here" to users who may edit
      upstream (§11.2), and the `site-customizing` skill says the same (§13.1).
15. **Review before Publish on a shared server** (D18, D19). The extend tier's Publish pushes to the site's remote.
    Whether a shared server should publish to a branch per user and open a merge request on the hosting side (Projects,
    or any host) rather than to the shared branch is open. Proposed: a `publish-branch` setting per root, off by
    default, in P2.
    - Answered Oct 24, 2026: agreed; a publish branch is a safeguard worth knowing about. The automatic branch and
      merge request are off by default and enabled, or required, per user by an admin (a preference set on the User
      screen in System, not shown to the user) or per repository in its settings (§4.3, §16); in P2.
16. **DbForm and the site component.** Whether sites keep using DbForm for production-time form changes beside the
    site component's files, and whether "Convert to site extension" (§7.2) should also run the other way for a change
    that must reach production without a deploy. Proposed: both stay, the views show the origin, the converter is one
    way.
    - Answered Oct 24, 2026: agreed.
