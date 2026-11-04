# Autodoc

Built Nov 3, 2026 (the checks still to do are under "Built" at the end). Generated Markdown docs for Moqui artifacts, like JavaDoc: shown in the Dev AIDE ([design-dev-aide](design-dev-aide.md) §7) and exported as a `.md` tree; the guide for what is built is [docs/dev/Autodoc.md](../docs/dev/Autodoc.md).

## Context

Moqui has no JavaDoc equivalent. The Dev AIDE's artifact index (`moqui_devkit::index`) and the loaded definitions
already know every entity, service, screen, ECA rule and REST resource, with their files, lines and references, but
nothing turns that into reading material. Autodoc generates Markdown from templates, one document per artifact plus
index pages per package. It is shown inline in the Dev AIDE, with a copy icon button, and exported as a `.md` tree into
a repository (default `autodoc/`). The same generator backs a `moqui autodoc` CLI so CI can publish docs without a
server.

Decisions (asked and answered):
- The generator is in `moqui-devkit` with a CLI.
- Mermaid fences and the copy button go into the framework's `m-markdown` and `markdown-view`.
- The first version has five trees: entity, service, screen, eca and rest.

Findings that shape the design:
- **Descriptions are not in the loaded models.** `EntityDefinition`, `ServiceDef`, `ScreenDef` and `EcaRuleInfo` have
  no `<description>` (only `ParamDef.description`). The index `summary` holds only the first sentence, cut at 160
  characters. Full text is read again from the XML at the indexed file and line.
- **`m-markdown` does not draw Mermaid.** It renders a ```` ```mermaid ```` fence as plain code (`WebrootVue.qvt.js`
  6917–7095). The Assist docs claim otherwise.
- **`.md.fancy` templates produce HTML.** Templates that output Markdown must be plain `.fancy` (`FancyRenderer`,
  trusted, no escaping).
- **`m-markdown` already supports what in-place links need:** `basePath`, `resolveLink` and the `link` event. Screens
  support `allow-extra-path` (`ViewWikiPage.xml` is the pattern), and `markdown-view` renders in tui as a markdown
  node.

## Output layout

The artifact type is the top level, then the type's package structure:

```
autodoc/
  README.md                                   types with counts, links to each tree
  .autodoc.json                               manifest of generated files (path, id, kind, package, title, summary, hash)
  entity/README.md                            packages
  entity/mantle/order/README.md               package: entity table + Mermaid erDiagram
  entity/mantle/order/OrderHeader.md          one entity or view entity
  service/README.md
  service/mantle/order/README.md              directory: service files + classDiagram
  service/mantle/order/OrderServices.md       one service file: its services table
  service/mantle/order/OrderServices/place-Order.md   one service ("#" becomes "-")
  screen/README.md
  screen/SimpleScreens/Order/README.md        directory: screens + subscreen tree flowchart
  screen/SimpleScreens/Order/OrderDetail.md   one screen (by path under screen/, not by mount)
  eca/README.md
  eca/service/mantle/order/OrderServices.secas.md   one rules file, a section per rule (path in component)
  eca/entity/OrderEntities.eecas.md
  rest/README.md
  rest/mantle.md                              one REST API file (root resource): resource tree + a section per method
```

- **Path rules** live in one table (`autodoc/plan.rs`): entity package segments become directories; services and
  screens use their path under the component's `service/` or `screen/`.
- **Collisions** across components (the same screen path in two components): both get `<Name>.<component>.md`, and
  the package page lists both with their component.
- **Links** between docs are relative Markdown links, so the viewer and the exported tree show the same text, and Copy
  copies exactly what Export writes.

## Document content (per template)

Every artifact doc has:
- the title;
- a facts line (kind, component, `component://` location and line, site or upstream);
- the full description;
- type-specific sections;
- **Used by / Uses:** a Mermaid `flowchart LR` in the Artifact Browser's shape. What uses the artifact is to the left
  and what it uses is to the right, one subgraph per kind with up to 8 nodes and "+ n more", one connector per group,
  and the center node styled. It is followed by the same references as tables with links, since Mermaid `click` is
  off under strict security and on GitHub.

| Type | Sections |
| --- | --- |
| Entity / view entity | Attributes (group, table, cache, audit). Fields table: name, type, pk, not-null, default, description, and the component when an `extend-entity` added the field. Relationships: type, related (linked), title, key map, fk. Indexes. View: members, aliases. Extensions (`extend-entity` files). EECA rules. A small `erDiagram` of the direct neighbors. |
| Service | Verb and noun, type and location, authenticate, transaction, read-only. In and out parameter tables, nested shown as `a.b`, with the implied and inherited ones marked. Implements. SECA rules on it. The inline actions in a fenced `xml` block, capped at 200 lines, and only with `includeSource`. |
| Service file | Its services table (name, type, summary) with links, and the file comment. |
| Screen | Title (`default-menu-title`). Description from the XML comment before `<screen>`, skipping the CC0 license comment. Render modes. Parameters. Subscreens items (linked). Transitions: method, service, responses. Forms with their fields. Where it is mounted (incoming `subscreens-item`). `screen-extend` files from other components. |
| ECA file | A section per rule: target (linked), when, condition, actions in a fenced block, and the `<description>`. A file-level flowchart: targets, then rules, then what the actions call. |
| REST file | A resource tree as a `flowchart TD`, then a section per resource and method: the service or entity it calls (linked) and its parameters. |
| Entity package | Entity table with summaries. An `erDiagram`: the package's entities with pk and fk fields; related entities outside the package as attribute-less nodes; one line per relationship (`}o--||` for one, `||--o{` for many, dotted for one-nofk); auto-reverse relationships skipped; names only when the package has more than 60 entities. |
| Service directory | Files table. A `classDiagram`: each file a class with its services as members, `<|..` to interfaces it implements, `..>` to files its services call. |
| Screen directory | Screens table. A `flowchart TD` of subscreen edges, with screens outside the directory as stubs. |
| Root / type index | Counts and package lists, built from the merged manifest so partial exports add up. |

## Architecture

### 1. Framework client (webroot, XSD)

- **`WebrootVue.qvt.js`.**
  - Factor `m-mermaid`'s load, initialize and run into `moqui.mermaid.run(nodes, config)`.
  - In `moqui.markdown.render`, add a `code` renderer: a `mermaid` fence becomes `<pre class="mermaid m-md-mermaid">` with
    the text escaped. Other fences are unchanged.
  - `m-markdown` gains:
    - a `mermaid` prop (default true): after `rendered`, it runs Mermaid (strict) on `.m-md-mermaid`. A diagram that
      fails stays as its source with a warning.
    - a `copyable` prop: a top-right icon button (`flat round dense`, `sym_o_content_copy`, aria-label and tooltip
      "Copy Markdown") that copies `text` with `navigator.clipboard.writeText` and calls `$q.notify` with "Copied".
- **`ScreenQvt.js`** (around 2070): pass `markdown-view`'s new `copy` attribute as `copyable`.
- **`xml-screen-3.xsd`:**
  - `markdown-view` gains a `copy` attribute (boolean) with its documentation.
  - The `markdown-view` documentation says mermaid fences are drawn in qvt and are text in tui, html and print.
- **`docs/ui/Qvt-Client.md`:** update the `m-markdown` row.

### 2. Generator: `crates/moqui-devkit/src/autodoc/`

Each module opens with a `//!` doc citing `plans/Autodoc.md`, and tests go in sibling files.

- **`mod.rs`** holds the API and shared types:
  - `Options { kinds, components, packages (prefixes such as mantle.order, mantle/order, SimpleScreens/Order),
    include_source, template_dirs }`.
  - `DocKind { Entity, Service, Screen, Eca, Rest }`.
  - `generate(factory, source, options) -> Generated { files: Vec<DocFile{path, id, kind, package, title, summary,
    text}>, failures: Vec<Failure{id, message}> }`.
  - `one(factory, source, doc_path_or_id) -> DocFile`.
  - Failure tolerance: a doc that fails to build or render is logged with its id and goes into `failures`; the rest go
    on.
- **`source.rs`:** a small `DocIndex` trait (`of_kinds`, `describe`, `children`, `refs(id, Direction)`) with two
  implementations:
  - `MemoryIndex` (export and CLI: static references, no database);
  - a database wrapper over `index::of_kinds` / `describe` / `children` / `refs` (the viewer: fast single lookups,
    including observed calls, drawn as dashed `-.->` edges).
- **`plan.rs`:** the bidirectional id ↔ doc path map, built from `of_kinds` over the five kinds:
  - path rules, collisions and package keys;
  - `relative_link(from_path, to_path)`.
  - It is cached on the factory, keyed by the index stamp (`index::stamp` / `MemoryIndex::stamp`).
- **`describe.rs`:** the full description text, read again from the source.
  - Per file, parse once with `MNode::parse_file` and map each element's line to its `<description>` text (entity,
    field, relationship, index, view parts, service, parameter, seca, eeca).
  - For screens and service files, take the leading comment before the root element using
    `moqui_lib::xml_edit::tokenize`, skipping the license comment.
  - A cache per run.
- **`model.rs`:** builds the template data (`Value` map) per doc type. Sources:
  - `moqui_core::entity::info::entity_detail`;
  - `moqui_core::service::info::service_detail`;
  - `ScreenFacade::of(factory).screen_def`;
  - `moqui_devkit::eca::rule_list`;
  - index `children` for REST resources;
  - `refs` in and out, grouped by kind as `dev-aide/src/artifacts.rs` `get_refs` does (move that grouping into this
    module and have dev-aide call it).

  Every linked artifact carries its `href`, a relative path or nothing when it has no doc. Escaping is done in the
  templates through the helper library.
- **`mermaid.rs`:** pure functions, unit-tested with goldens in `autodoc/testdata/`:
  - `usage_flowchart`, `er_diagram`, `class_diagram`, `screen_tree` and `rest_tree`;
  - safe ids (`[A-Za-z0-9_]`, deduplicated) and label escaping (`"` → `#quot;`, newlines dropped);
  - the caps.

  The model passes both the rendered text (`diagrams.usage`, ...) and the node and edge lists, so a custom template can
  build its own diagram.
- **`render.rs`:**
  - Template lookup by name (`Entity.fancy`, `Service.fancy`, `ServiceFile.fancy`, `Screen.fancy`, `EcaFile.fancy`,
    `RestFile.fancy`, `EntityPackage.fancy`, `ServiceDirectory.fancy`, `ScreenDirectory.fancy`, `TypeIndex.fancy`,
    `Index.fancy`): the `template_dirs` first, then `component://moqui-framework/template/autodoc/`. Use the
    framework's actual component name.
  - Rendering goes through `TemplateRegistry::of(factory).render_location(...)`.

**Templates:** `runtime/base-component/framework/template/autodoc/*.fancy`, plus `AutodocLib.js`.
- `AutodocLib.js` is a script module loaded with `<#assign doc = ec.resource.script(...)>`, holding `cell` (escapes `|`
  and newlines), `code`, `table(headers, rows)`, `link(text, href)` and `fence(lang, text)`.
- The templates use `<#if>`, `<#list>` and `${}` only.

**Overrides:** a site component's `template/autodoc/<Name>.fancy` replaces the default of that name. The Dev AIDE
passes its site components' directories; the CLI takes `--templates <dir>`.

### 3. CLI: `moqui autodoc`

- **Where:**
  - `crates/moqui-host/src/devtools_cmd.rs` gets `dispatch_autodoc`, beside `dispatch_refs`, using `model_only` and
    `MemoryIndex::build`;
  - `crates/moqui-host/src/lib.rs` dispatches it;
  - usage text goes in `crate::usage`.
- **Flags:** `--out <dir>` (default `autodoc`), `--kinds`, `--components`, `--packages`, `--include-source`,
  `--templates`, `--clean`, `--project`.
- **Writing:** it shares the manifest writer with the Dev AIDE (below), kept in `autodoc/write.rs`.
- **Writes:** only changed files, so a git diff shows only real changes. With `--clean` it removes files listed in the
  previous manifest that were not generated this time, and never any other file.

### 4. Dev AIDE (`runtime/dev-component/dev-aide`)

- **`service/org/moqui/devaide/DocServices.xml`**, implemented in `src/docs.rs` and registered in `src/lib.rs`:
  - `get#ArtifactDoc`:
    - in: `artifactId` or `docPath`;
    - out: `docPath`, `title`, `markdown`, `artifactId`, `kind`, `crumbs`;
    - tier View, read-only;
    - uses the database `DocIndex`;
    - template dirs: the site components'.
  - `export#ArtifactDocs`:
    - in: `rootId`, `path` (default `autodoc`), `kinds`, `components`, `packages`, `includeSource`, `removeStale`;
    - out: `written`, `unchanged`, `removed`, `failures`, `url` (Workspace link to the directory);
    - tier Extend.
- **The export writes one batch, not a `save_text` per file**, which would run a reload, a check and a git status for
  each of thousands of files.
  - Each file path goes through `confine::resolve` and `Context::may_write` (read-only and upstream lock as for any
    write; an upstream root needs the Full tier with the toggle). Make `may_write` `pub(crate)` if it is not already.
  - Files are written atomically: lift `write_atomic` to `pub(crate)`.
  - It writes only changed files and keeps the manifest.
  - It records one audit row with a new `DAA_EXPORT` enum (`data/DevAideSecurityData.xml`) and runs one git status at
    the end. No reload: `.md` files are not artifacts.
  - It uses `MemoryIndex::build` for a consistent static snapshot.
- **Screens:**
  - **`screen/DevAide/Artifacts/Doc.xml`** (menu "Docs", index 5):
    - `allow-extra-path="true"`, so the doc path is the extra path (`/qapps/dev/Artifacts/Doc/entity/mantle/order/OrderHeader.md`);
    - an `artifactId` parameter redirects to its path;
    - actions call `get#ArtifactDoc`;
    - widgets:
      - breadcrumbs as links;
      - a search box to jump to an artifact's doc;
      - `markdown-view text="${markdown}" base-path="<the doc's directory URL>" copy="true"`, so relative `.md` links
        open in place through the shell;
      - links to Open in Workspace and Open in the Browser;
      - a not-found state that links back to the Docs index.
    - Plain XML, so it renders in tui too.
  - **`Artifacts/DocExport.xml`** (menu "Export docs", index 6):
    - a form with the target root (a drop-down of writable roots, site roots first), the path (default `autodoc`),
      checkboxes for kinds, a components multi-select, package prefixes, include source, and remove stale generated
      files;
    - one primary button, "Export docs";
    - a result box with the counts, the failures list and a link to the directory in the Workspace.
  - Add both to `Artifacts.xml`.
- **`Workspace.xml`:** a read-only `artifactDoc` transition for the JS views, in the existing transition shape.
- **JS:**
  - **`DevAide.js`:** a small `m-dev-artifact-doc` component (props `base`, `artifactId`). It fetches `artifactDoc`
    and renders `<m-markdown copyable :resolve-link>`, mapping relative `.md` links to the Doc screen. Register it with
    the others.
  - **`DevAideEntity.js` and `DevAideService.js`:** a "Doc" tab in their `q-tabs`, refreshed after Save.
  - **`DevAideScreen.js`:** a "Doc" toggle in `#DevScreenPanel`.
  - **`DevAideBrowser.js`:** a "Doc" link in the center card's actions.
  - **`Refs.xml`:** an "Open the doc" link in `RefsCenter`.

## Tests

The tests named here are new. None are slow, all run on small fixtures, and the existing Chromium (gate3) tests are
not run without your permission.

- **`cargo test -p moqui-devkit autodoc`:**
  - `mermaid.rs` goldens (each diagram kind, escaping, caps, collisions);
  - `plan.rs` path rules and the round trip of id and path;
  - `describe.rs` descriptions and the license-comment skip;
  - a golden of a whole generated tree over a fixture project in `autodoc/testdata/project/`: a component with two
    related entities, an extend-entity, a service file with an interface, a screen with a subscreen and transition, a
    SECA and an EECA file, and a REST file.

  Goldens are rewritten with `UPDATE_GOLDEN=1`, and the diff is read.
- **Webroot client, offline** (`WebrootVue.qvt.markdown.page.js`): a mermaid fence renders to `.m-md-mermaid` with an
  SVG; `copyable` shows the button and copies the text. A page test calls `mermaid.parse` on every golden diagram, so
  the generator's syntax is checked against Mermaid 11.17.2.
- **dev-aide server spec `test/ArtifactDocs.spec.js`:**
  - `get#ArtifactDoc` for an example entity, a service and a screen has the expected headings, tables and a mermaid
    fence;
  - `Artifacts/Doc/...` renders in qvt and tui with no render errors (added to the screen render specs);
  - `export#ArtifactDocs` is refused for the View tier with the same parameters an Extend user succeeds with.
- **`cargo test -p moqui-host --test it devaide::autodoc`:**
  - an export into a site root writes the tree and the manifest;
  - a second export reports everything unchanged;
  - `removeStale` removes a file that is only in the manifest and leaves a hand-written `.md` alone;
  - an upstream root is refused.
- **Also:** `cargo lint`, `cargo test -p dev-aide`, `moqui test-client runtime/dev-component/dev-aide --offline`, and
  the webroot client suite offline.
- **Measure** the full corpus once:
  - a viewer doc should take under 100 ms;
  - a full export via the CLI is to be measured. If it goes over about 15 s, the Dev AIDE export runs in the background
    with progress; this decision is recorded in `plans/Autodoc.md`.
- **End to end:**
  - `./moq build`, then `./moq run` on ports 8091 and 2232 (another server holds 8080);
  - open `/qapps/dev/Artifacts/Doc/entity/...` and check the ER diagram and the copy button;
  - follow links in place;
  - export into a scratch site component and look at the tree in the Workspace;
  - `moqui autodoc --out <scratch dir>` and spot-check a few files on GitHub-style rendering.

## Docs

- **`docs/dev/Autodoc.md`:** the layout, what each doc holds, templates and overriding, the CLI and Export.
- **Links:** from `docs/README.md` and the dev-aide `README.md` (the Artifacts bullet: Docs and Export docs; Tests).
- **`plans/design-dev-aide.md` §7:** a short 7.5 pointing to `plans/Autodoc.md`. It notes that D9's "no Mermaid" is
  about the browser canvas and still holds.

## Order of work

1. `m-markdown` Mermaid and copy, and `markdown-view copy`, with their client tests.
2. The devkit `autodoc` module and templates, with goldens.
3. The CLI and the manifest writer.
4. The Dev AIDE services, screens and JS hooks, with the spec and the `it` test.
5. Docs, then the measurement.

## Built

Nov 3, 2026, all five steps. As built, beside the design above:

- **Usage groups by doc.** A doc's "Used by" and "Uses" group the other side by the doc it is shown in, not by its own
  index kind. A transition is its screen ("service-call in transition createOrder"), a field or relationship is its
  entity, a rule is its file, and a data file's records are the file. So the dev-aide's `get#Refs` grouping (by kind,
  for the Browser) was left where it is, not moved.
- **Collisions** take the component's name at the unit: a screen, a service file (its services' directory with it), an
  entity, an ECA or REST file.
- **Stale files** are the manifest's files that the plan has no doc for any more, not the manifest's files left out of
  this export. So "Remove stale docs" is safe on a partial export.
- **Blank lines.** `render::tidy` keeps one blank line between blocks and puts one before each heading, table and
  fence, so the templates need not count them.
- **Moved tests.** `crates/moqui-host/src/devtools_cmd.rs`'s 176-line inline test module moved to
  `devtools_cmd_tests.rs` (Practices-Rust §3) before `moqui autodoc`'s test was added.
- **Diagram checks.** Every diagram of the fixture's tree is a golden (`testdata/mermaid/tree.json`) that the webroot's
  page test parses with Mermaid 11.17.2. A one-off run of the same check over the whole corpus parsed all 8,018
  diagrams.

Measured on the Debug package (unoptimized):
- **`moqui autodoc` over every component:** 7,642 docs in 18.7 s, with the boot, the memory index and the writes; 38 MB.
- **The viewer:** the first doc of a server builds the artifact index (16 s on Debug; its target is under 2 s on the
  Dev package, design-dev-aide §7.1). After that a doc takes about 50 to 150 ms.

The Dev AIDE export runs in the request. Whether it needs to run in the background with progress turns on the time on
the Dev package, which is still to be measured.

Still to do:
- **The Dev package time** of a full export.
- **A look in a browser:** the Docs page in light, dark and at 390 px, the diagrams of a large package (`mantle.order`,
  about 60 entities, drawn with key fields), and the copy button.
- **A Chromium check** (gate3, slow, run only with permission) that a Doc tab draws its diagrams.
