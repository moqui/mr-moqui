# Themes

The theme composition adapter renders a merchant theme on the locked renderer: a package of a layout, JSON page
templates, sections, blocks, snippets, typed settings and assets, given as data, validated and rendered within budgets.
Theme source never reaches `ec`, services, entities, resources or the network. The framework has no records of its own
for themes: an app (IronStone Shop's `mantle.public.ThemeServices`) keeps the releases, passes a release's files in, and
assembles the view. The design is mr-pass `plan/design-shop.md` §12; the code is `crates/moqui-core/src/script/theme`.

## The service API

```js
const pkg = {files: {"theme.json": "...", "templates/product.json": "...", "sections/hero.html.fancy-locked": "..."}}

const v = ec.resource.theme.validate(pkg, {requiredPageKinds: ["home", "product"], apiVersion: 1})
// {ok, engine: "js-fancy", diagnostics: [{severity, code, file, line, message, section?, block?}]}

const r = ec.resource.theme.render(pkg, {
    pageKind: "product",                 // or template: "product.featured"
    view: {site, shop, locale, page, navigation, product, productList},
    components: {"shop.productPurchase": {html: trustedHtml}},   // or {error: "..."}; required: true
    references: {"page:about": {url: "/shop/pages/about", label: "Our story"}},
    media: {"m100": {url: "/shop/assets/sha256-…/mug.webp", alt: "Mug"}},
    sources: {"featured": [...]},
    assetBase: "/shop/assets/sha256-…",  // or assetUrls: {"assets/store.css": url}
    allowedOrigins: ["https://cdn.example.com"],
    releaseId: "R100",                   // for log lines
    limits: {pageTimeMs: 2000}           // lowers the operator's limits, never raises them
})
// {ok, html, diagnostics, omitted: [{section, block?, reason}], failedRequired: ["section:main", "component:shop.cart"],
//  dependencies: ["theme.json", "templates/product.json", "sections/hero.html.fancy-locked", ...]}

ec.resource.theme.engines()          // ["js-fancy"]
ec.resource.theme.limits()           // the operator's limits
ec.resource.theme.sanitizeSvg(text)  // an SVG asset cleaned to the safe set
ec.resource.theme.sanitizeRichText(html)
```

`ok` is false when the page cannot be used for a purchase. With the layout, the template or the page budget failed,
`html` is empty and the caller shows its own safe not-available page. With a required section or component failed,
`html` is the rest of the page and `failedRequired` names what failed, so the caller turns purchase off. An optional
section or block that fails is left out (or replaced by its schema's `placeholder`), logged with the release, template
and section, and listed in `omitted`. Diagnostics are for the developer (preview, Commerce logs), not public pages.

`validate` checks the files, schemas, settings and templates; it does not render. The representative renders design-shop
§12.5 asks for are `render` calls the app makes with sample views. From Rust the same calls are
`moqui_core::script::theme::{validate, render}` with `Package`, `ValidateOptions` and `RenderRequest`.

## The package

| Path | What it is |
|---|---|
| `theme.json` | `{schemaVersion: 1, name, engine: "js-fancy", viewApiVersion: 1, layout, requiredComponents, styles, defaultTemplates}` |
| `layouts/<name>.html.fancy-locked` | The page around the sections: `${content}` places them, `${styles}` the stylesheet links |
| `templates/<name>.json` | `{layout?, sections: {id: {type, settings, blocks, blockOrder, required, disabled}}, order}`; `layout: false` for none. `<kind>.<alt>.json` is an alternate of a page kind |
| `sections/<type>.html.fancy-locked`, `.schema.json` | A section and its schema: `{settings, blocks: [{type, limit}], maxBlocks, allowedPageKinds, required, placeholder}` |
| `blocks/<type>.html.fancy-locked`, `.schema.json` | A block; its schema may allow nested blocks |
| `snippets/<name>.html.fancy-locked` | Included with `<#include "name">`, which the adapter puts in before the render |
| `config/settings.schema.json`, `config/settings.json` | The global settings (`{settings: [...]}` or groups `[{name, settings}]`) and their values |
| `locales/<name>.json` | Texts by key |
| `assets/...` | css and svg as text (checked), images and fonts as anything else (named only) |

Anything else, `.liquid` included, is refused. Ids and names are letters, digits, `-` and `_`. A JSON object with a key
given twice is an error. Without `order` a template has at most one section.

Templates are the fancy dialect (`${}`, `<#if>`, `<#list>`, `<#assign>`) with HTML escaping, in the locked renderer.
What a template sees:

- the caller's view (design-shop §12.3's contract), with `settings` (the global settings), `components` (placeholders
  of trusted fragments by name), `assets` (URL by path) and `theme` (`{name}`);
- in a section: `section` (`{id, type, settings, blocks}`) and `blocks`, each `{id, type, settings, html}`, where
  `${block.html}` places the rendered block;
- in a block: `section` (`{id, type}`), `block` and its own `blocks`;
- `format(value, pattern)` and `localize(text)`.

A view value `{"$richText": html}` is sanitized and placed as a fragment, as a `richText` setting is.

## Settings

Types: `text` (one line), `multilineText`, `richText` (sanitized), `boolean`, `number` (`min`, `max`, `step`), `enum`
(`options`), `color` (`#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`), `image` (a package `assets/` path or a media id resolved
through `media`), `resourceLink` (`kind:id`, resolved through `references`), `product`, `category` and `page` (an id,
resolved as `product:<id>`...), and `source` (an approved source name, `sources` limiting it, resolved through
`sources`). A definition has `id`, `type`, `default`, `required`, `maxLength` and, for the text types, `localized` (the
value may be an object by locale). A default applies only when the value is absent; an explicit empty, false, zero or
null is kept. A reference that does not resolve is null with a warning, or a failure of its section when required.

## Safety

- Expressions name no framework object (`ec`, `sri`, `context`), no `raw()`, nothing of the renderer's own (`$…`), and
  nothing that reaches the global object or makes code (`globalThis`, `Function`, `constructor`, `__proto__`,
  `Reflect`…); no `this`, `new`, function bodies, assignments (use `<#assign>`), `delete` or `import`. A file is checked
  alone and again with its snippets put in. A property name built at run time (`x["con" + "structor"]`) is not seen;
  it reaches only the frozen sandbox, which holds the snapshot, `format` and `localize`.
- Markup is checked in source (with each `${}` as a mark, so a finding has its line) and again in each render's output:
  no `<script>`, frame, plugin, `<form>`, `<base>`, `<noscript>`, `<math>`; no `<foreignObject>` or animation anywhere,
  and no `<style>` in an `<svg>`; no `on…` attribute, `srcdoc`, `action`, `form` or `formaction` (and the other `form…`
  overrides) or `http-equiv`; no expression in a tag or attribute name. The check is a tokenizer's, not a tree
  builder's, so what a browser would parse as markup inside an SVG is refused everywhere: no `<` in a `<style>` (CSS
  writes it `\3c`) and no tag in a `<title>` or `<textarea>`. Links (`<a href>`) take http, https, mailto, tel and
  relative URLs. What the page loads (`src`, `srcset`, `<link href>`, CSS `url()`, an SVG `fill`, `filter`, `mask`…
  `url()`) is the page's own origin, an `allowedOrigins` origin, or a raster `data:` image; a `\` in a URL is read as
  `/`, as a browser reads it. CSS has no `@import`, `expression()`, `image-set()` or binding, read after its comments
  and escapes. `<link rel>` is one of
  stylesheet, icon, apple-touch-icon, preload, canonical, alternate, next, prev, author and license.
- Fragments (rendered blocks and sections, sanitized rich text, trusted components) are placeholders until the end and
  may stand only in text outside an `<svg>`, where the render's own view gave them, and a render's output leaves no
  `<svg>` open. A reserved character in data is removed first.
- Snippet includes are static: one that includes itself, directly or through others, is a `recursion` error.
- SVG assets are cleaned to drawing elements and attributes, in the SVG and XLink namespaces only; validation reports
  each unsafe removal.

Codes: `syntax`, `forbidden-name`, `forbidden-expression`, `unsafe-markup`, `unsafe-url`, `unsafe-css`, `unsafe-svg`,
`recursion`, `include-depth`, `invalid-include`, `budget`, `invalid-json`, `invalid-manifest`, `invalid-template`,
`invalid-schema`, `invalid-setting`, `invalid-reference`, `unknown-setting`, `unknown-type`, `unknown-file`,
`unknown-engine`, `liquid`, `missing-file`, `missing-template`, `not-allowed`, `not-rendered`, `duplicate-id`,
`unresolved`, `render-failed`, `required-failed`.

## Budgets

| Limit | Default | Conf attribute | Request key |
|---|---|---|---|
| Sections a page | 100 | `max-sections` | `maxSections` |
| Blocks a section (nested included) | 100 | `max-blocks` | `maxBlocks` |
| Composition depth (layout 1, section 2, block 3…) | 8 | `max-depth` | `maxDepth` |
| Snippet include depth | 8 | `max-include-depth` | `maxIncludeDepth` |
| Page HTML | 2 MiB | `max-output-bytes` | `maxOutputBytes` |
| One render (layout, section or block) | 1000 ms | `render-time-ms` | `renderTimeMs` |
| The page | 5000 ms | `page-time-ms` | `pageTimeMs` |
| Sandbox memory | 32 MiB | `memory-bytes` | `memoryBytes` |

An operator lowers them in conf, `<resource-facade><theme-limits max-sections="50" page-time-ms="2000"/>`, and a call's
`limits` lowers them again; nothing raises a default. The locked renderer's own 5 s and 32 MiB cap each render. A
section that loops or allocates past its budget is stopped and left out; a section whose output would pass what the
layout leaves of the page budget is left out; an include past the depth or a page past the output fails as above.

## Tests

`cargo test -p moqui-core --lib script::theme` runs the adapter's tests on `script/theme/testdata/stone` with goldens
in `testdata/golden` (`UPDATE_GOLDEN=1` rewrites them; read the diff). `ec_extra_tests` checks the service API.
