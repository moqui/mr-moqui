# Dev AIDE by HEMP

November 18, 2026 — design, not implemented. This plan makes HEMP the structure of [Dev AIDE](design-dev-aide.md):
its terms, its process as the menu, Coarchy's screens adapted for the requirements work, and a diagram-like overview
of a site. It adds to the Dev AIDE design and changes its menu (§4); the decisions D1 to D22 cited here are that
design's. The storage, the Coarchy scope, the overview and the place of agents were decided with the user the same
day (H3, §5, H8, H9, H13), and the book in the component after the first draft, also that day (H14, §9).

Sources. HEMP: "HEMP: An agile approach to analysis and design", David E. Jones, revision 1.0, 2013
(`~/Dropbox/Business/MyBooks/hemp/HempTheBookv1.0.pdf`; "p." is its printed page) and its back-cover artifact diagram
(`diagrams/NewHempArtifactDiagram.pdf` there). Coarchy: `~/work/moquiwork/3p_moqui/coarchy`, a Moqui Java component
written around HEMP.

Paths: `DA/` is `runtime/components/dev/dev-aide/`, `fw/` is `runtime/components/base/framework/`, `coarchy/` is the
Coarchy clone, and `my-site` stands for a site's own component (D17). Every other path is relative to this repository.

---

## 0. Decisions

| # | Decision | Why |
| --- | --- | --- |
| H1 | The book's names are the UI's names, spelled as the artifact diagram spells them: Process Story, activity, Actor Definition, Requirement Statement, Idea to Incorporate, Experience Story, Business Case, Overlap Description, Gap Description, Screen Outline, Functional Wireframe, Screen Flow Diagram, Data Statement, Data Model, Data Mapping, Initial Data, Process Outline, System Interface, Screen, Service & API, High Level Test Scenario, Low Level (Task) Test Scenario. The roles are Expert User, Business Analyst, UI Designer, System Architect, Software Developer and QA Technician. | Asked for. One vocabulary across the book, the screens, the files and the agents' skills. |
| H2 | Names already taken keep their meaning: "spec" is a `moqui test` spec, "project" is `moqui.devtools.DevArtifactProject` and a task source's projects, "site" is the site component (D17). A HEMP set belongs to a **component**, and the overview is of a component, the site's by default. HEMP's "artifact" and the index's "artifact" become one thing (H5). | "Project" has two meanings here already; a third would mislead. |
| H3 | HEMP artifacts are XML files under `<component>/hemp/` with a schema (§3), written through Dev AIDE's save pipeline and the lossless XML layer, so a save is a minimal text edit and the git diff is the change (D3). No `moqui.dev` entity holds requirements or designs. | Decided with the user. A dev instance is one developer's (D19), so database rows would not be shared. Git gives the history, the diff between two revisions and the review that the book's chapter 8 asks of a tool, and agents read the files as source. |
| H4 | An activity has a stable id in its story file. Order is the order in the file. A paragraph break is an element of its own. A high-level activity names its detailed story, to any depth, and a save that would make a loop is refused. | Coarchy orders by a `sequenceNum` that several transitions read without the story (a move can renumber another story), writes a break as an empty activity, and allows two levels of story, enforced only in what it displays (§2.2). |
| H5 | Every HEMP artifact and every link between two of them is in the artifact index: a file kind, artifact kinds, and the links as references. A link is written once, in the file §3 names for it, and the index answers the other direction. | Search, References, the Browser and the agents' `artifact` tool then work on requirements and designs with no second store. The book wants incoming links shown automatically. |
| H6 | The menu is the HEMP process: Overview, Requirements, Existing System, User Interface Design, Technical Design, Implementation, Review, and then the tools (Workspace, Repositories, Artifacts, Tasks, Settings). Entities becomes Technical Design › Data Model, Data becomes Initial Data, Screens becomes Implementation › Screen, Services becomes Service & API, and ECA Rules goes under Implementation. | Asked for. In the book the physical data model is design, and screens and services are implementation. |
| H7 | The list-shaped HEMP screens are XML screens, form-lists and dialogs as Coarchy's are, whose services apply one operation per submit to the file. They work in the browser and the terminal with no `.qvue`. Only the Overview has a `.qvue`, with a list as its twin. | Practices-App-Dev allows a `.qvue` only where XML widgets cannot express the page, and Coarchy's screens port most directly this way. |
| H8 | The Overview is the book's artifact diagram drawn for one component: its group boxes, a card per artifact type with its count and coverage in its role's colour, and the book's arrows, dashed where the book dashes them. Fixed coordinates on `m-pan-zoom` with one SVG layer for the connectors: no layout library and no Mermaid (D9). | Decided with the user. The diagram does not change shape, so nothing has to lay it out. |
| H9 | The trace of one item is the Artifact Browser, which learns the HEMP kinds: an activity in the centre, what it came from on one side, and the outlines, data statements, entities, screens and services it led to on the other. | Decided with the user. The Browser draws exactly this from the index's references. |
| H10 | An activity's state is derived, not typed in: not analysed, overlap, partial overlap, gap or no system interaction; then designed, implemented, functionally supported and accepted. Only the last two are recorded, in a review file. Coarchy's implementation status is dropped for it; an activity may carry a `phase` for trimming scope. | After the analysis the book's story "becomes an overview of what is supported in the existing system and what is not" (p. 44). |
| H11 | The existing system of a gap/overlap analysis is the upstream components (D17). An Overlap Description links activities to upstream screens and services; a gap is designed, and built as an extension in the site component. | Dev AIDE is extension first, which is the book's "reuse the existing system" path (p. 43). |
| H12 | Not taken from Coarchy: organizations as tenants and their entity filter (the component is the scope), invitations, premium gates, credits and Stripe, the public site, agreements, users assigned to actors, task emails and the search documents. | They serve its hosted, multi-tenant business. A component in git replaces the organization. |
| H13 | Agents come after the screens (P7): skills per HEMP role for the `dev` agent, every write a change set through `propose_edit` (D11). | Decided with the user. The files and their rules have to exist before a skill can be held to them. |
| H14 | The whole book is in the Dev AIDE component as Markdown, a file per chapter (`DA/book/`), as a re-release of the 2013 book under this repository's CC0 terms: corrected, improved, and adapted so that each chapter says how its step is done on the screens. Its release is the MR Moqui version it ships with, 0.1.0 first. It is the reference, the guide to the HEMP screens, the online help of the HEMP screens, and what the role skills cite. The appendix's examples are also a working HEMP set in the example component. | Asked for: Coarchy shows excerpts, and the book is short (about 21,000 words), so the whole of it explains the process better than a selection. Text that ships with the component is there for the help and for the agents, on any dev instance. The terms, the corrections, the adaptation to the screens and the version were decided by the author, Nov 18, 2026. |

---

## 1. HEMP in one page

**The process.** The book states it as a process story (chapter 2, p. 19 to 24). Its six high-level activities, as
written:

1. "Business Analyst and Expert User gather and document requirements."
2. "Business Analyst documents overlaps and gaps with an existing system."
3. "UI Designer designs screens and reports."
4. "System Architect models data and defines system interfaces."
5. "Software Developer implements user and system interface designs."
6. "System Architect, UI Designer, Business Analyst, and Expert User review implementation."

**The artifacts.** The diagram groups them and draws which feeds which:

| Group | Artifacts | Fed by |
| --- | --- | --- |
| Requirements | Business Case, Requirement Statement, Experience Story, Process Story, Actor Definition | Business Case, Requirement Statement and Experience Story feed the Process Story; the story feeds the Actor Definition |
| between | Gap Description, Overlap Description | Process Story |
| Design, user interface | Screen Outline, Functional Wireframe, Screen Flow Diagram, Graphic Design | Process Story with the Gap Description feeds the Screen Outline; the outline feeds the other three |
| Design, technical | Data Statement, Data Model, Data Mapping, Initial Data, Process Outline, System Interface | Process Story with the Gap Description feeds the Data Statement, Process Outline and System Interface; statements feed the model and the initial data; outlines, interfaces and the model feed the mapping |
| Implementation | Screen, Service & API | The user interface designs feed the Screen; System Interface and Data Mapping feed Service & API |
| Test Definition | High Level Test Scenario, Low Level (Task) Test Scenario | Process Story feeds the high level; it, Data Mapping and Initial Data feed the low level |
| Business Implementation | End User Documentation, Deployment and Migration Plan | no arrows |

The primary roles colour the cards: Analyst, UI Designer, System Architect, Developer.

**What a tool must hold to.**

- A requirement is who, what and when; a design is how and where (p. 7). "A business activity is an actor performing
  an action."
- Each sentence of a story is one activity, an active verb phrase with its actor. The system being built is never an
  actor ("Company automatically ..."). The verbs are "record" and "review", not "submit", "enter" or "display"
  (p. 27 to 30, 39). There is no "why" in a story; that is the Business Case.
- A story is a hierarchy: a summary story of high-level activities, each the title of a detailed story (p. 19, 38).
  Conditions and alternate flows stay next to the primary flow; time-triggered activities go in a time flow (p. 39).
- Links, both ways: story and business case; activity and its overlap or gap; outline element and activity; data
  statement and data model; a screen's, interface's or process's fields and the data model (p. 36, 44 to 49, 63).
- Whoever made an artifact reviews the result against it (p. 21 to 24). A change goes back to the earliest artifact
  it affects and forward again; developers do not change designs alone (p. 56, 68).
- The tool the book asks for (chapter 8): a hierarchy of documents, links to sections, history with who and a diff
  between revisions, comments, and one code repository. A component in git with Dev AIDE over it is that.

---

## 2. Where we start

### 2.1 Dev AIDE

- Its menu is by artifact type and the developer's loop: Home, Workspace, Repositories, Artifacts, Entities,
  Services, Screens, ECA Rules, Data, Tasks, Settings (`DA/screen/DevAide.xml:22-34`), with palette and page actions
  that name those paths (`:35-57`). Nothing is arranged by lifecycle. It has no story, actor, requirement or design
  concept, and no file in this repository mentions HEMP.
- Its seven entities (`DA/entity/DevAideEntities.xml`) hold roots, settings, servers, credentials, task sources and
  the audit. It depends on the framework only; Mantle is used only by a `mantle` task source.
- Every service is Rust and checks the tier first (`DA/src/tier.rs:60`). Files are written through one save
  pipeline (`DA/src/workspace.rs:601`, `:628`) that confines the path to a root, applies the upstream lock, audits,
  reloads and reports problems. Structural edits go through the lossless XML layer
  (`crates/moqui-lib/src/xml_edit/`), which the guided editors use (`DA/service/org/moqui/devaide/GuidedOps.js`).
- The artifact index is in the framework: the extractor in `crates/moqui-devkit/src/index/` (file kinds at
  `extract.rs:344`) and the tables `DevArtifactFile`, `DevArtifact` and `DevArtifactRef`
  (`fw/entity/DevToolsEntities.xml:20`, `:32`, `:61`). The Browser groups an artifact's references by the other
  side's kind (`DA/src/browser.rs`).
- For a diagram: `m-pan-zoom` (`runtime/components/base/webroot/screen/webroot/js/WebrootCode.js:609`) with a slot
  for an SVG layer, and the card layout the Browser and the entity member diagram share
  (`DA/screen/devstatic/DevAide.js:137`). Mermaid is vendored and drawn by `markdown-view`.
- A new site component is made from `DA/template/site/`. The agent's skills and recipes are in `DA/skill/`.
- The regression budget is 30 seconds of server specs and 2 of client specs (`DA/test/budget`).

### 2.2 Coarchy

- Five entities of its own, each with an `organizationId` (`coarchy/entity/CoarchyEntities.xml`): `ProcessStory`,
  `ProcessStoryActivity` (order, and the substory of a step), `Activity` (condition, action, implementation status),
  `Actor` with `ActivityActor`, and `ValueStatement` with `ValueStatementActivity`. A clipboard of three entities
  holds copied activities. Checklists are Mantle work efforts; the vision is three `PartyContent` rows.
- Fifteen screens of authoring under `coarchy/screen/coarchy/cointernal/` (§5 lists them). A sentence is rendered
  as condition in italics, actors in bold, then the action (`coarchy/template/ActivityStyled.html.ftl`).
- Defects an adaptation must not carry: the sequence queries of insert, paragraph and move do not filter by story
  (`Process/ProcessStory.xml:71-144`); deleting an activity leaves its row and its value links; the duplicate-name
  check matches substrings; substories stop at two levels; the two editors, Edit and Full Edit, duplicate each
  other; reordering is by arrows with drag-and-drop a TODO.
- The rest is a hosted business: tenancy, invitations, credits, Stripe, a public site (H12).
- It includes parts of the book: `coarchy/screen/coarchy-com/c/HempExcerpts.html`, about 8,400 words, an introduction
  and excerpts that begin with "Requirements versus designs", as one public page linked from its home page.

### 2.3 What exists elsewhere

mr-boss Insights has flow maps that cite Coarchy's stories
(`runtime/components/addon/mr-boss/plan/design-insights.md`, FlowMap and ProcessStory): lanes and steps that open
worklists in a running business. They document operations for the people doing them and stay as they are; Dev AIDE
cannot depend on mr-boss, and its stories are requirements for building (question 5).

---

## 3. The files

A component's HEMP set is its `hemp/` directory. One schema, `hemp-3.xsd`, covers the files (question 1).

| Path | Artifact | Holds | Links written here |
| --- | --- | --- | --- |
| `business-case.xml` | Business Case | Narrative sections (Coarchy's Origin Story, Mission and Vision); general objectives with detailed objectives, each with a priority | objective to activities |
| `actors.xml` | Actor Definition | Name, nature (person, system, organization), description | |
| `requirements.xml` | Requirement Statement, Idea to Incorporate | Statements; ideas, open or incorporated | idea to the activities that incorporate it |
| `story/<Name>.xml` | Process Story | Activities (id, actors, optional condition, action, optional phase), breaks, alternate flows, a time flow, supporting documents | activity to actors; high-level activity to its detailed story |
| `experience/<Actor>.xml` | Experience Story | Prose in paragraphs | paragraph to activities |
| `analysis/<Story>.xml` | Overlap Description, Gap Description | For an activity or a run of them: overlap, partial overlap, gap or no system interaction, and the instructions or the gap | to activities; to the upstream screens and services |
| `outline/<Screen>.xml` | Screen Outline, Data Mapping | Sections, elements, what each shows, the interaction and the response | element to activities; field to entity field; outline to the Screen it became |
| `data/<Concept>.xml` | Data Statement | Statements of one data concept | to activities; to the entity or field |
| `process/<Name>.xml` | Process Outline | Trigger and steps | to activities; to the Service it became |
| `interface/<Name>.xml` | System Interface | Format, transport, fields and their mapping | to activities; to entity fields; to the Service |
| `test/<Story>.xml` | High Level Test Scenario | Steps in the story's order | to activities; to the spec it became |
| `review/<Story>.xml` | Implementation Review | Per activity: functionally supported and accepted, by whom and when; the commit the review started from | to activities |
| `supporting/` | supporting documents | Forms, example reports, field lists | |

A story file:

```xml
<process-story name="Fulfil Order" xsi:noNamespaceSchemaLocation="http://moqui.org/xsd/hemp-3.xsd">
    <activity id="a1" actors="Customer"><action>places an order</action></activity>
    <activity id="a2" actors="Company"><action>automatically reserves inventory for the order</action></activity>
    <activity id="a3" actors="Packer" detail="PackShipment">
        <condition>When all items of the order are picked</condition>
        <action>packs the shipment</action>
    </activity>
    <alternate after="a3">
        <activity id="a4" actors="Packer">
            <condition>If an item is damaged</condition><action>records the damaged quantity</action>
        </activity>
    </alternate>
    <break/>
    <activity id="a5" actors="Carrier" phase="later"><action>collects the shipment</action></activity>
</process-story>
```

- It reads "*When all items of the order are picked,* **Packer** packs the shipment.", as Coarchy renders it.
- Ids are never reused in a file, so a link survives a move or a rewording. A link names the file and the id:
  `FulfilOrder#a3`, with a component in front when it is another component's.
- `detail` names the story file of a high-level activity. The story list is the tree those make.
- The index (H5) gets the file kind `hemp` and the kinds `story`, `activity`, `actor`, `objective`, `requirement`,
  `idea`, `experience`, `overlap`, `gap`, `outline`, `outline-element`, `data-statement`, `process-outline`,
  `interface`, `test-scenario` and `review`. A link's reference type says what it is (actor, detail, objective,
  incorporates, analysed, existing, designs, maps, implements, tests, reviews). A link to an artifact that is gone is
  an unresolved reference, as an extension's is today.
- Upstream components' `hemp/` files are read as theirs are: shown, searchable and locked (D17). A site story may
  link to an upstream activity.

---

## 4. The menu by process

| Menu | Screen (`/qapps/dev/...`) | From | Modes |
| --- | --- | --- | --- |
| **Overview** (default) | `Overview` | new (§6); Home's restart banner and Publish in its header | `.qvue`, list twin |
| **Requirements** | `Requirements/BusinessCase` | Coarchy Vision, ValueStatements, ViewValueStatements | XML |
| | `Requirements/Stories` (Find, Edit, View) | Coarchy FindProcessStory, EditProcessStory, ProcessStory, ViewProcessStory | XML |
| | `Requirements/Actors` | Coarchy Actor, ViewActors | XML |
| | `Requirements/Statements` | new: Requirement Statements and Ideas to Incorporate | XML |
| | `Requirements/Experience` | new: Experience Stories | XML |
| **Existing System** | `ExistingSystem` | new: overlaps and gaps (§7.1) | XML |
| **User Interface Design** | `UiDesign/Outlines` | new: Screen Outlines (§7.2) | XML |
| | `UiDesign/Flow` | new: Screen Flow Diagram, derived | XML, Mermaid |
| **Technical Design** | `TechnicalDesign/DataStatements` | new (§7.3) | XML |
| | `TechnicalDesign/DataModel` | Dev AIDE Entities | as today |
| | `TechnicalDesign/DataMapping` | new: the outlines' and interfaces' mappings by entity | XML |
| | `TechnicalDesign/InitialData` | Dev AIDE Data | as today |
| | `TechnicalDesign/ProcessOutlines`, `SystemInterfaces` | new (§7.4) | XML |
| **Implementation** | `Implementation/Screens` | Dev AIDE Screens | as today |
| | `Implementation/Services` ("Service & API") | Dev AIDE Services | as today |
| | `Implementation/Rules` | Dev AIDE ECA Rules | as today |
| | `Implementation/Customizations` | Home's "Your customizations" and Artifacts › Customizations | as today |
| **Review** | `Review/Implementation` | Coarchy FindChecklist, Checklist (§7.5) | XML |
| | `Review/TestScenarios` | new (§7.5) | XML |
| Workspace, Repositories, Artifacts, Tasks, Settings | unchanged paths | Home's create-component dialogs move to Repositories | as today |
| **Book** | `Book` | new: the HEMP book (§9) | XML, `markdown-view` |

- The existing screens' files stay where they are and are mounted under their phase with `subscreens-item`; only
  their paths and menu titles change. Nothing has been released, so no old path is kept.
- What names the old paths and changes with them: the palette and page actions (`DA/screen/DevAide.xml:36-51`), the
  specs under `DA/test/`, the skills and recipes in `DA/skill/`, the guided editors' links, and
  `docs/security/Attack-Surface-Dev.md`.
- Every HEMP screen has a component picker, the site component by default, and shows the file it edits with Text one
  click away (D18).
- The phase screens' own pages are short: what the phase is in the book's words, who does it, and its artifacts with
  counts. A user new to HEMP reads the process from the menu.

---

## 5. Coarchy's screens in Dev AIDE

| Coarchy (`coarchy/screen/coarchy/cointernal/`) | Dev AIDE | What changes |
| --- | --- | --- |
| `Home.xml`: links and a feed | Overview | The map replaces the links; the feed is dropped (git has the history) |
| `Vision.xml` | Business Case, narrative | Three sections of one file |
| `ValueStatements.xml`, `ViewValueStatements.xml` | Business Case, objectives and the read view | General objectives with detailed ones under them; Priority kept; connected activities both ways; value type dropped |
| `Actor.xml`, `ViewActors.xml` | Actors and the read view (the glossary, then each actor's activities) | Nature added; assigned users dropped; delete refused while an activity names the actor |
| `Process/FindProcessStory.xml` | Stories › Find | The tree of summary and detailed stories to any depth |
| `Process/EditProcessStory.xml` and `Process/ProcessStory.xml` | Stories › Edit, one editor | Sentence rows; add in the last row; Insert above or below; Paragraph; Move; select rows to Copy, Cut and Delete; filters on actor, text and state; each row shows its state (H10) |
| `Process/ViewProcessStory.xml` | Stories › View | High-level activities in bold, detailed stories below with show and hide; print |
| `InsertActivity.xml` paste, Copy To, Move To | Activity clipboard | Per user, in memory as the edit buffers are (`DA/src/buffers.rs`); between stories and components; pasted activities get new ids; an actor the target lacks is added to its `actors.xml` |
| `CreateOrSelectSubstory.xml` | Detailed story dialog | Any depth; the loop check of H4 |
| `ManageValues.xml` | Objectives of an activity | The same dialog; it writes `business-case.xml` |
| `FindChecklist.xml`, `Checklist.xml` | Review › Implementation | §7.5 |
| `settings/` (organizations, billing), `coarchy-com/` (public site) | none | H12 |

- The services are Rust under `org.moqui.devaide` (`HempServices.xml`, `DA/src/hemp/`), with the tier check first:
  reading needs View; writing needs Extend for a site component and the upstream toggle for any other.
- One operation per submit (H7): the service reads the file, applies the operation through the XML layer and saves
  through the pipeline, so the audit, the lock, the problems and the reload log are the ones every save has.
- Reordering is Move up and down in both modes. Drag in the browser is a later `.qvue` only if the arrows prove slow.

---

## 6. Overview and trace

**The map.** One page for one component, in the diagram's arrangement: Requirements top left, the gap and overlap
cards to its right, Design below right, Implementation below left, Business Implementation under it, Test Definition
at the bottom, and the role legend top right. Each card is an artifact type: its name, a count, one coverage line,
and its role's colour with the role named in the legend (colour is not the only sign). Selecting a card opens its
screen.

| Card | Count | Coverage line |
| --- | --- | --- |
| Business Case | objectives | activities linked to an objective |
| Requirement Statement | statements | ideas not yet incorporated |
| Experience Story | stories | |
| Process Story | stories, activities | activities without an actor definition |
| Actor Definition | actors | |
| Overlap Description, Gap Description | activities of each | activities not analysed |
| Screen Outline | outlines | gap activities that an outline element names |
| Screen Flow Diagram | derived | |
| Data Statement | statements | statements mapped to the model |
| Data Model | the component's entities and extensions | |
| Data Mapping | fields mapped | outline and interface fields without a mapping |
| Initial Data | data files | |
| Process Outline, System Interface | of each | those with a Service |
| Screen | outlines with a Screen | |
| Service & API | the component's services | |
| High Level Test Scenario | scenarios | stories with one |
| Low Level (Task) Test Scenario | the component's specs | |
| Functional Wireframe, Graphic Design, End User Documentation, Deployment and Migration Plan | shown dimmed | later (§11) |

- The arrows are the book's, with its dashes. An arrow carries nothing of its own; the coverage lines say how far
  each step has been taken.
- The header has the component picker, the site's restart banner and Publish.
- The counts come from the index in one service call. A component with no `hemp/` shows the map with zeros and
  "Start with a Process Story".
- The cards' positions are constants in the page's script, tested as the Browser's layout is. At phone width and in
  the terminal the same data is a list: a heading per group, a row per card with its count, coverage and link, in
  the process's order.

**The trace.** From a story row, an outline element or a card's list, Trace opens the Artifact Browser on that
item. For an activity: its story, objectives and actors on one side; its overlap or gap, outline elements, data
statements, and through them the screens, entities and services on the other. The Browser's kind filter gains the
HEMP kinds. Its terminal form, Miller columns, needs nothing new.

---

## 7. Design, implementation and review screens

### 7.1 Existing System

The story's activities in order, each with its analysis: overlap, partial overlap, gap, or no system interaction
with the reason (p. 44). An overlap names the upstream screens and services, picked from the index's search, and
holds the instructions an end user would follow. A run of activities can share one description. The story editor
shows the result on every row. A component built from scratch skips this screen, and every activity counts as a gap.

### 7.2 Screen Outlines

An outline is the screen's sections, their elements (label, link, button, form, table, field), and under each what
it shows, the interaction and the system's response with its target screen (p. 48). An element names the activities
it is based on. A field names its entity field: the Data Mapping. "Create screen" hands the outline to the screen
builder as a new subscreen and records the screen on the outline; from then the outline shows whether its screen
still exists. Shared sections have outlines of their own. The Screen Flow Diagram is drawn from the outlines'
targets, one per component.

### 7.3 Data Statements, Data Model, Data Mapping

A statement is a sentence of two data concepts and a verb: "Party has zero to many Contact Mechanisms." (p. 60).
The screen lists them by story flow, as they are written, and by data concept, as they are mapped. A statement names the
entity or field it maps to; "Add to data model" opens the entity modeler on that entity, an `extend-entity` in the
site component when the entity is upstream (D17). Data Mapping is a read view by entity: every outline, interface
and process field that maps to it.

### 7.4 Process Outlines and System Interfaces

A process outline is a trigger (a time or a user action) and steps in pseudo code; a system interface is the format,
transport and field mapping of one system-to-system interaction of the story (p. 65). Each names its activities, and
the Service it became once there is one.

### 7.5 Review

**Implementation Review** is the book's sixth activity, from Coarchy's checklist. Starting one for a story writes
`review/<Story>.xml` with an item for each activity, detailed stories included, and the commit it started from. The
Business Analyst marks an item functionally supported, not supported, or not applicable (Coarchy's "Condition not
met"); the Expert User then accepts it or not. Each mark records the user and the date. Coarchy's forced sequence,
assignments and emails are not taken. A later round is added to the same file.

**High Level Test Scenario.** "Create scenario" writes `test/<Story>.xml` with a step per activity. A scenario can
be saved as a spec skeleton through the service writer's Save as spec, and records the spec. The component's specs
are its Low Level (Task) Test Scenarios.

---

## 8. Story checks

Checks of a HEMP file run in the save pipeline and show in Problems, as an XML file's do:

- an actor that `actors.xml` does not define;
- "System" as an actor, and an activity with no actor;
- an action that starts with a verb the book replaces (submit, enter, display);
- a detailed story that does not exist, or a loop;
- a link to an activity, screen, entity or service that is gone;
- an idea marked incorporated that names no activity.

They are warnings, except the loop, which refuses the save. What needs judgement (a passive sentence, a "why" in a
story, a design idea in a requirement) is the agents' (§10).

---

## 9. The book

**What is included.** All of the book, a file per chapter, in `DA/book/`:

| File | Chapter | Helps with |
| --- | --- | --- |
| `README.md` | Title, author, the re-release note with its terms and its version, what has changed since revision 1.0, contents, the back-cover artifact diagram, and a table from each artifact to its section and its Dev AIDE screen | The Book screen's first page |
| `Preface.md` | Preface: audience, about the author, about the book | |
| `The-Same-Thing-Every-Night.md` | 1: requirements versus designs, why bother, planning for failure and change, scaling complexity, agile methodologies, diagrams | The Overview; the idea every screen rests on |
| `The-Story-of-HEMP.md` | 2: the process as a process story | The Overview and each phase page |
| `Gathering-Requirements.md` | 3: business process story, requirement statement and idea to incorporate, actor definition and experience story, business case, writing and reviewing | Requirements |
| `Building-on-Existing-Systems.md` | 4: overlap description, gap description | Existing System |
| `User-Interface-Design.md` | 5: screen outline, wireframe and prototype, screen flow and menu structure, graphic design, design review and testing | User Interface Design |
| `Technical-Design.md` | 6: data statement, data model, data mapping, initial and test data, automated process outline, system interface design | Technical Design |
| `Implementation.md` | 7: software development, quality assurance, business implementation, packaged software, ongoing development | Implementation, Review |
| `Tools-and-Systems.md` | 8: what a tool should offer | |
| `Case-Studies.md` | 9: four projects and their lessons | |
| `Examples.md` | Appendix: a story, actor definitions, an experience story, data statements, a data model, a screen outline, a wireframe, a screen data mapping | Every file kind of §3 |

- It is a re-release. The 2013 book is "© 2013 David E. Jones, All Rights Reserved"; these files are released by
  the author under the repository's terms, CC0 1.0 (`LICENSE.md`), and `README.md` says so and names the original
  (revision 1.0, its ISBN). No exception is needed in `LICENSE.md`.
- The printed edition stays for sale on Amazon as it is, and `README.md` names it only as the original. The book's
  website is no longer online, so its address in the front matter is taken out in the first correction. Nothing
  outside this repository has to change for the re-release.
- The first commit is revision 1.0 exactly: the same words, headings and emphasis, converted from `HempTheBook.html`
  beside the PDF and checked word for word against the PDF's text. Chapter 2's high-level activities stay bold, which
  is how a story marks them. Every change after it is a commit of its own, so git shows what differs from the book.
- Corrections and improvements are then made in the chapters as the work shows them: errors of spelling and
  wording; chapter 8's tools; the test scenarios, which only the diagram names; what building P0 to P7 teaches
  about the method. `README.md` keeps a short list of what has changed since revision 1.0.
- The chapters are adapted to the package, so the book and the screens are one thing. Each artifact's section
  keeps the method and then says how it is done here: the screen, the file under `hemp/`, the links to write, the
  checks that run and what the Overview shows for it. The names are the screens' (H1). Where the plan departs from
  the 2013 text (ids on activities, derived state, the review file, the upstream components as the existing system)
  the chapter says what Dev AIDE does.
- It names menus, screens and buttons and links to them. It has no screenshots, which go stale; its figures are the
  artifact diagram, the wireframe and file examples.
- Its release is the repository's. The book has no revision number of its own: `README.md` says it is released with
  MR Moqui and points to the repository's version (`Cargo.toml`, `[workspace.package]`, 0.1.0 now), and the Book
  screen shows the running version, as the component does (`DA/component.xml:20`). What changed between releases is
  the repository's history of `DA/book/`.
- The figures are the wireframe (`diagrams/GroupMilestoneWireframe1.png`) and the artifact diagram redrawn as an
  SVG; the cover image is left out.
- Section headings keep the book's names, so a link to a section is stable: `Gathering-Requirements.md#business-case`.
- Chapter 8 names the products of 2013. It is rewritten first: what a tool should offer stays, and the tool it
  describes is Dev AIDE over a component in git, with the Workspace, the Artifact Browser, Tasks and the group
  server in the places the old products had.

Each chapter is adapted in the phase that builds its screens, in the same commits:

| Phase | Adapted |
| --- | --- |
| P0 | The first commit of revision 1.0 as it was; then `README.md`, the Preface's "About the Book", chapter 8, and the appendix's examples shown as `hemp/` files |
| P1 | Chapter 2 with the menu and the Overview; chapter 1 where it speaks of tools and diagrams |
| P2 | Chapter 3 |
| P3 | Chapter 4 |
| P4 | Chapters 5 and 6 |
| P5 | Chapter 7: review, quality assurance and the test scenarios |
| P7 | The roles in chapters 2 to 7, where an agent's skill can do or draft a step |

**The Book screen.** `Book` takes the chapter as its extra path and shows it with `markdown-view`, as
`Artifacts/Doc` shows a doc (`DA/screen/DevAide/Artifacts/Doc.xml:21`, `:51`), so the chapters' relative links and
the terminal work. It has the contents beside the text, and Search finds text in the chapters.

**Help on the screens.**

- Each phase page opens with the book's high-level activity for it and links its chapter.
- Each HEMP screen has one help link named for its artifact ("About Process Stories") that opens that section of the
  book in a dialog, with Open in Book. The Overview's cards have the same link.
- A story check's problem (§8) links the rule it comes from: the verbs to "Writing and reviewing: the how".

**The examples as files.** The appendix's examples, the book's own project management story, are written as a
`hemp/` set in `runtime/components/dev/example`: the story, actors, an experience story, data statements, a screen
outline and its data mapping. A new user opens a complete set; the specs of P0 to P6 use it as their fixture; the
site template gets only an empty first story.

**Outside the app.** The book is the guide to the HEMP screens: `docs/README.md` links `DA/book/README.md`, and no
separate HEMP guide or second copy is kept in `docs/`.

---

## 10. Agents

P7 adds skills to the `dev` agent (`DA/skill/`), one per role, each holding to the rules of §1 and the checks of §8.
A skill is short, as the others are, and names the chapter sections of `DA/book/` it works from; the agent reads a
section with its `source` tool when it needs the detail or an example. Every write is a change set (D11).

| Skill | Role | Does |
| --- | --- | --- |
| Interview | Business Analyst | Asks the Expert User what the organization does, writes the activities, parks crosscutting ideas as Ideas to Incorporate, proposes splits into detailed stories |
| Analysis | Business Analyst | For each activity, searches the upstream components through the index and proposes an overlap or a gap |
| Outline | UI Designer | Drafts Screen Outlines from the gap activities, each element linked |
| Data | System Architect | Writes Data Statements from activities and outline fields, proposes their mapping and the model's extensions |
| Review | each role | Compares an implementation with the artifact it came from and lists the differences |

The existing recipes then start from a design: "add a field and show it" from a data statement and an outline
element. The Assist prompts on HEMP screens name the artifact open, as the artifact prompts do today.

---

## 11. Phases

| Phase | Delivers | Check |
| --- | --- | --- |
| P0 | The `hemp/` layout and schema; the index's file kind, kinds and references; an empty first story in `DA/template/site/`; the book in `DA/book/` with the Book screen (§9); the appendix's examples as a `hemp/` set in the example component | An index spec on the example set: a story file's activities are found, and a link is answered from both ends. The book's first commit matches the PDF's text, and every link from `README.md` resolves |
| P1 | The menu by process with the existing tools under their phases; the Overview with its counts and its list twin | One render of the Overview in each mode; the layout's client check merged into an existing spec; old paths gone from specs, skills and docs |
| P2 | Requirements: Actors, Stories (Find, Edit, View, detailed stories, clipboard), Statements, Business Case, Experience; the story checks | Specs of the operations (insert, move, detail, paste, delete) against a fixture file; one render of the editor |
| P3 | Existing System; the derived state on story rows and the Overview | A spec of the derived state |
| P4 | Screen Outlines with Create screen; Data Statements with Add to data model; Data Mapping; Process Outlines; System Interfaces; Screen Flow | Links indexed; the two hand-offs open the right tool on the right artifact |
| P5 | Implementation Review; High Level Test Scenario and its spec skeleton | A review file written and read back; a generated spec that parses |
| P6 | The trace: HEMP kinds in the Artifact Browser | The Browser spec extended, not a second render |
| P7 | The role skills | Agent evals with replays, as `DA/test/agent/` has |
| Later | Functional Wireframe, Graphic Design, End User Documentation, Deployment and Migration Plan; a design pinned to the story revision it was made from, with the differences since | |

**Rules for every phase.**

- Every screen works in the browser and the terminal; a `.qvue` only for the Overview.
- Run the specs of what was touched. The component stays inside its budget (`DA/test/budget`); a screen is rendered
  at most once in each mode across the regression suite, and no spec walks the HEMP menu.
- New routes go in `docs/security/Attack-Surface-Dev.md`. `DA/README.md` and the design's menu description follow
  the code.
- The book follows the code: a commit that changes a HEMP screen, file or term changes its chapter. A spec checks
  that every screen the book links to exists.
- The pre-commit skill before each commit.

---

## 12. Open questions

1. Where `hemp-3.xsd` and the index's HEMP extractor live. The index is the framework's, so both in the framework is
   the simple answer; the alternative is a kind a component can register.
2. Whether Home stays as a screen. This plan moves its panels to the Overview, Implementation and Repositories.
3. How a time flow is written: a section of a story file by period, or a story of its own kind.
4. A story that spans components: links across components work, but which component owns the summary story of a
   site built from several.
5. Whether mr-boss Insights' flow maps should be able to read a HEMP story as their source.
6. What a Functional Wireframe is here: the screen builder's preview of an outline's screen, or an attached image.
7. Whether tasks name the activities they deliver, so the task branch and the commit trailer carry the trace.
8. Whether the component repositories' existing `plan/` documents get HEMP sets, and for which components first.
