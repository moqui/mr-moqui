# Autodoc

Markdown docs of a system's artifacts, like JavaDoc for Moqui: an entity, service, screen, ECA rules file or REST API
as the server loaded it, with its descriptions, its tables and Mermaid diagrams of what uses it and what it uses. They
are made by templates. The Dev AIDE shows them as you work, and `moqui autodoc` or the Dev AIDE's Export docs writes
them as a tree into a repository. The design is [plans/archive/Autodoc.md](../../plans/archive/Autodoc.md).

## The tree

The artifact type is the top, then the type's packages:

| Path | What it is |
| --- | --- |
| `README.md` | Each tree with its count |
| `entity/<package as directories>/<Entity>.md` | An entity or view entity; `entity/<package>/README.md` lists the package's entities with an E-R diagram |
| `service/<path under service/>/<File>.md` | A service file and its services; each service in `<File>/<verb>-<noun>.md`; `service/<dir>/README.md` lists the directory's files with a class diagram |
| `screen/<path under screen/>.md` | A screen, by its file under the component's `screen/` (not where it is mounted); `screen/<dir>/README.md` lists the directory's screens with their subscreen tree |
| `eca/<path in the component>.md` | An ECA rules file (`.secas.xml`, `.eecas.xml` ...), a section per rule |
| `rest/<path under service/>.md` | A REST API file (`.rest.xml`): its resource tree and each method |
| `<tree>/README.md` | A tree's packages or directories, and the docs at its top |
| `.autodoc.json` | The manifest: every file the tool wrote here |

When the same path is in two components (a screen a site component puts at the path of an upstream one), each doc
takes its component's name: `Orders.my-site.md` and `Orders.mr-marble.md`. Links between docs are relative, so a tree
reads the same wherever it is put, on GitHub and in the Dev AIDE alike.

## What a doc holds

Every artifact's doc has its title, where it is defined (component, `component://` location and line), its full
description and what uses it and what it uses. That last part is a Mermaid flowchart in the Artifact Browser's shape,
users to the left and uses to the right, one box per kind with eight names and "+ n more". Tables under it link each
one to its doc.

- **Entity:** the table, group and cache; the fields with their descriptions and, for a field an `extend-entity` adds,
  its component; the relationships, indexes, view members and aliases; the `extend-entity` files and EECA rules; an
  E-R diagram of the entity and the entities it relates to.
- **Service:** its type, location, authentication and transaction; in- and out-parameters (a nested one by its dotted
  name, an inherited or implied one marked); what it implements; the SECA rules on it; and with "include source" its
  definition.
- **Screen:** its menu title, render modes and login; the comment that heads its file (the license comment left out) as
  its description; parameters, subscreens, transitions and forms. What mounts, extends and links to it is in the
  "Used by" part.
- **ECA rules file:** a section per rule (what it runs on, when, the condition and the actions), and a flowchart of the
  services and entities the rules run on, the rules, and what they call.
- **REST API file:** a flowchart of its resource tree to two levels, and a table of every method with the service or
  entity it calls.

Descriptions are the `<description>` elements of the XML, read from the file again: the loaded definitions keep only
their first sentence. A description is Markdown as its author wrote it.

## In the Dev AIDE

- **Artifacts › Docs** (`/qapps/dev/Artifacts/Doc/<path>`) shows any doc of the tree. The doc's path is the page's
  path, so its links open the docs they name. You can also type an artifact's id or name (`OrderHeader`,
  `place#Order`). The icon button in the top right copies the Markdown, and the diagrams are drawn in the page. Open
  in Workspace and Open in the Browser go to the artifact. A service's doc can show its definitions. It is a plain
  `markdown-view`, so `moqui tui` reads it too.
- **Doc tabs:** the entity modeler and the service writer have a Doc tab, and the screen builder has a Doc panel. Each
  shows the doc of the artifact open there and makes it again after a Save. The Artifact Browser's center card and
  References have a Doc link.
- **Artifacts › Export docs** writes a tree into a directory of a repository or directory of the Workspace, `autodoc`
  by default. You choose the artifact types, components and package prefixes (none means all). It needs the extend
  tier and makes the checks a save makes: a read-only root is refused, and so is an upstream component while the lock
  is on. It writes in one batch, with one audit row (`DAA_EXPORT`) and no reload.

The viewer reads the dev server's artifact index, so a doc counts the calls observed at run time. An export reads the
files as they are, as `moqui autodoc` does.

## `moqui autodoc`

```sh
moqui autodoc                                                     # every artifact, into ./autodoc
moqui autodoc --out ../my-site/autodoc --components my-site --clean
moqui autodoc --kinds entity,service --packages mantle.order --out /path/to/order-docs
moqui autodoc --project ~/src/other-moqui --out ~/src/other-moqui/autodoc
```

From a checkout, `./moq autodoc` runs it with the same options (`./moq help autodoc`). It writes into `./autodoc` of the
directory you run it from, and reads a relative `--out`, `--templates`, `--project` or `--java` from there too.

It boots the model with no database (as `moqui refs` does), reads every component's files and writes the tree.
`moqui help autodoc` has every option.

- `--kinds`: entity, service, screen, eca, rest.
- `--components`: the components to document.
- `--packages`: prefixes by whole segments. A dot or a slash separates them: `mantle.order`, `mantle/order`,
  `SimpleScreens/Order`.
- `--include-source`: put each service's definition in its doc.
- `--templates DIR`: templates to use in place of the framework's.

A doc that does not build is reported, the others are written, and the command exits 1.

## Writing into a repository

Only the files whose text changed are written, so the repository's diff is the docs that changed. The directory's
`.autodoc.json` lists every file the tool wrote there.

- **A partial export** adds to the manifest, and the indexes list what it wrote together with what was there before.
- **`--clean` (Remove stale docs in the Dev AIDE)** removes the files the manifest lists whose artifacts are gone. It
  never removes a file the manifest does not list, so a hand-written page in the tree stays.

## Templates

The templates are trusted fancy templates ([JS-Fancy-Templates](JS-Fancy-Templates.md): `<#if>`, `<#list>`,
`<#assign>` and `${}`, in JavaScript) in
`component://framework/template/autodoc/`:

| Template | Doc |
| --- | --- |
| `Entity.fancy`, `Service.fancy`, `Screen.fancy` | An artifact |
| `ServiceFile.fancy`, `EcaFile.fancy`, `RestFile.fancy` | A file |
| `EntityPackage.fancy`, `ServiceDirectory.fancy`, `ScreenDirectory.fancy` | A package |
| `TypeIndex.fancy`, `Index.fancy` | A tree's index, the root |

A site component's `template/autodoc/<Name>.fancy` takes the place of the framework's template of that name, in the
Dev AIDE's viewer and export alike. `moqui autodoc --templates DIR` does the same from a directory. A template's names
are its doc's data: `title`, `name`, `location`, `line`, `component`, `description`, `up`, `rootHref`, then the kind's
own (`entity`, `service`, `screen`, `rules`, `rest`), `usedBy` and `uses`, and `diagrams`. The framework's templates
show each one.

`AutodocLib.js` holds the Markdown helpers a template takes with
`<#assign md = ec.resource.script("component://framework/template/autodoc/AutodocLib.js")>`: `plain`, `code`, `link`,
`codeLink`, `table`, `fence`, `mermaid`, `usage`, `nav` and `defined`. Names are escaped by them, and a description is
used as written. The diagrams come as Mermaid text (`diagrams.usage`, `diagrams.er` ...), so a template places them
with `md.mermaid(...)`.

The files end in `.fancy`, not `.md.fancy`: a `.md.fancy` template is rendered to HTML.

## Tests

| Command | What it checks |
| --- | --- |
| `cargo test -p moqui-devkit autodoc` | The paths, the descriptions, the diagrams, and every file of a fixture project's tree against goldens in `crates/moqui-devkit/src/autodoc/testdata` (`UPDATE_GOLDEN=1` writes them) |
| `moqui test-client webroot --offline --spec WebrootVue.qvt.markdown` | Every diagram golden, and every diagram of the fixture's tree, parses with the vendored Mermaid |
| `./moq test dev-aide --spec ArtifactDocs` | The viewer service and the screens |
| `cargo test -p moqui-host --test it devaide::autodoc` | Exports on a server: the tiers, the manifest, stale removal and the upstream lock |
