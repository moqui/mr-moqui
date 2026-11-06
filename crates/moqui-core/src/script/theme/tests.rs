/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! The theme adapter: the `testdata/stone` package validated and rendered against goldens in `testdata/golden`
//! (`UPDATE_GOLDEN=1` rewrites them), and each budget, escaping context and refusal on a package changed from it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value as Json};

use super::*;

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/script/theme/testdata")
}

fn read_dir(root: &Path, dir: &Path, files: &mut Vec<(String, String)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            read_dir(root, &path, files);
        } else {
            let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            files.push((relative, std::fs::read_to_string(&path).unwrap()));
        }
    }
}

/// The stone package with `changes` made: a text replaces a file, None removes it.
fn stone_with(changes: &[(&str, Option<&str>)]) -> Package {
    let root = testdata().join("stone");
    let mut files = Vec::new();
    read_dir(&root, &root, &mut files);
    for (path, text) in changes {
        files.retain(|(existing, _)| existing != path);
        if let Some(text) = text {
            files.push(((*path).to_string(), (*text).to_string()));
        }
    }
    Package::from_files(files.iter().map(|(path, text)| (path.as_str(), text.as_str())))
}

fn stone() -> Package {
    stone_with(&[])
}

fn expect(file: &str, actual: &str) {
    let path = testdata().join("golden").join(file);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {}", path.display()));
    assert_eq!(actual, expected, "{file}");
}

fn product_request() -> RenderRequest {
    RenderRequest::from_json(
        &json!({
            "pageKind": "product",
            "releaseId": "R1",
            "assetBase": "/shop/assets/sha256-abc",
            "view": {
                "site": {"displayName": "Stone <Market>"},
                "locale": "en-US",
                "page": {"kind": "product"},
                "product": {
                    "id": "10000", "name": "Stone mug \"classic\"", "sku": "MUG-1",
                    "price": {"amount": "24.00", "display": "$24.00"},
                    "description": {"$richText": "<p>Stoneware <b>mug</b><img src=x onerror=alert(1)></p>"}
                },
                "productList": [
                    {"name": "Bowl", "url": "/shop/products/10001/bowl", "price": {"display": "$18.00"}},
                    {"name": "Plate & saucer", "url": "/shop/products/10002/plate", "price": {"display": "$12.00"}}
                ]
            },
            "components": {"shop.productPurchase": {"html": "<div data-component=\"purchase\" data-variant=\"10000\"></div>"}},
            "references": {"page:about": {"url": "/shop/pages/about", "label": "Our story"}}
        }),
        &Limits::default(),
    )
}

fn errors(validation: &Validation) -> Vec<String> {
    validation
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.is_error())
        .map(|diagnostic| {
            format!(
                "{} {}:{} {}",
                diagnostic.code, diagnostic.file, diagnostic.line, diagnostic.message
            )
        })
        .collect()
}

fn validate_stone(changes: &[(&str, Option<&str>)]) -> Validation {
    validate(
        &stone_with(changes),
        &ValidateOptions {
            required_page_kinds: vec!["home".into(), "product".into()],
            api_version: Some(1),
            ..ValidateOptions::default()
        },
    )
}

/// The one error of a validation, which must have this code, file and line.
fn refused(changes: &[(&str, Option<&str>)], code: &str, file: &str, line: usize) -> String {
    let validation = validate_stone(changes);
    let found = validation
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.is_error() && diagnostic.code == code && diagnostic.file == file)
        .unwrap_or_else(|| panic!("no {code} in {file}: {:?}", errors(&validation)));
    assert_eq!(found.line, line, "{code} line: {}", found.message);
    found.message.clone()
}

#[test]
fn the_stone_package_validates() {
    let validation = validate_stone(&[]);
    assert!(validation.ok(), "{:?}", errors(&validation));
    assert_eq!(validation.engine, "js-fancy");
    assert_eq!(engine_ids(), vec!["js-fancy"]);
}

#[test]
fn golden_product_page() {
    let rendered = render(&stone(), &product_request());
    assert!(rendered.ok, "{:?}", rendered.diagnostics);
    assert!(rendered.omitted.is_empty(), "{:?}", rendered.omitted);
    expect("product.html", &rendered.html);
    assert!(rendered
        .dependencies
        .contains(&"snippets/price.html.fancy-locked".to_string()));
    assert!(rendered.dependencies.contains(&"assets/store.css".to_string()));
}

#[test]
fn golden_home_page_uses_defaults() {
    let mut request = product_request();
    request.page_kind = Some("home".into());
    request.components = Json::Null;
    let rendered = render(&stone(), &request);
    assert!(rendered.ok, "{:?}", rendered.diagnostics);
    expect("home.html", &rendered.html);
}

#[test]
fn each_escaping_context() {
    let rendered = render(&stone(), &product_request());
    let html = &rendered.html;
    // Text: markup in data is text.
    assert!(html.contains("<title>Stone &lt;Market&gt;</title>"), "{html}");
    // Attribute: a quote cannot end the attribute.
    assert!(html.contains("alt=\"Stone &lt;Market&gt;\""), "{html}");
    assert!(html.contains("Stone mug &quot;classic&quot;"), "{html}");
    // Rich text: sanitized, then placed as a fragment.
    assert!(html.contains("<p>Stoneware <b>mug</b><img src=\"x\"></p>"), "{html}");
    assert!(html.contains("<p>Safe <em>rich</em> text</p>"), "{html}");
    assert!(!html.contains("onerror") && !html.contains("<script"), "{html}");

    // URL: a javascript: URL in data is refused, so the section is left out.
    let mut request = product_request();
    request.references = json!({"page:about": {"url": "javascript:alert(1)", "label": "x"}});
    let rendered = render(&stone(), &request);
    assert!(rendered.ok);
    assert_eq!(rendered.omitted[0].section, "story");
    assert!(
        rendered.omitted[0].reason.contains("javascript"),
        "{:?}",
        rendered.omitted
    );
    assert!(
        rendered.html.contains("<section class=\"store-hero\"></section>"),
        "the neutral placeholder"
    );
    // A resource from an origin that is not approved, then approved.
    let mut request = product_request();
    request.asset_base = Some("https://cdn.example.com/a".into());
    let rendered = render(&stone(), &request);
    assert!(!rendered.ok, "the layout loads the logo from the CDN");
    request.allowed_origins = vec!["https://cdn.example.com".into()];
    assert!(render(&stone(), &request).ok);

    // CSS: a value that ends the declaration and loads from elsewhere.
    let mut request = product_request();
    request.page_kind = Some("home".into());
    let home = |background: &str| {
        let package = stone_with(&[
            (
                "sections/hero.schema.json",
                Some(r#"{"settings": [{"id": "background", "type": "text"}], "blocks": [{"type": "text"}]}"#),
            ),
            (
                "templates/home.json",
                Some(&format!(
                    r#"{{"sections": {{"hero": {{"type": "hero", "settings": {{"background": {}}}}}}}}}"#,
                    json!(background)
                )),
            ),
        ]);
        render(&package, &request)
    };
    assert!(home("red").omitted.is_empty());
    let exfiltrate = home("red; background-image: url(https://evil.example/x)");
    assert!(
        exfiltrate.omitted[0].reason.contains("origin"),
        "{:?}",
        exfiltrate.omitted
    );
    // Unquoted attribute: escaping leaves spaces, so data can add an attribute, which the output check refuses.
    let injected = stone_with(&[
        (
            "blocks/text.html.fancy-locked",
            Some("<p class=${block.settings.body}>x</p>"),
        ),
        (
            "templates/product.json",
            Some(
                r#"{"sections": {"main": {"type": "product-main"}, "story": {"type": "hero",
                "blocks": {"b1": {"type": "text", "settings": {"body": "a onmouseover=alert(1)"}}}}},
                "order": ["main", "story"]}"#,
            ),
        ),
    ]);
    let rendered = render(&injected, &product_request());
    assert!(rendered.ok);
    assert_eq!(
        rendered.omitted[0].block.as_deref(),
        Some("b1"),
        "{:?}",
        rendered.omitted
    );
    assert!(rendered.omitted[0].reason.contains("onmouseover"));
}

#[test]
fn refusals_have_a_file_and_line() {
    let file = "sections/hero.html.fancy-locked";
    let message = refused(
        &[(file, Some("<p>\n${ec.entity.find('Product')}</p>"))],
        "forbidden-name",
        file,
        2,
    );
    assert!(message.contains("`ec`"), "{message}");
    refused(&[(file, Some("<p>\n<#if sri>x</#if></p>"))], "forbidden-name", file, 2);
    refused(
        &[(file, Some("<p>${raw(product.name)}</p>"))],
        "forbidden-name",
        file,
        1,
    );
    refused(&[(file, Some("<p>${$o.push('<b>')}</p>"))], "forbidden-name", file, 1);
    refused(&[(file, Some("<p>${[].constructor}</p>"))], "forbidden-name", file, 1);
    refused(
        &[(file, Some("<p>${(() => { while (true) {} })()}</p>"))],
        "forbidden-expression",
        file,
        1,
    );
    refused(
        &[(file, Some("<div>\n\n<script>alert(1)</script></div>"))],
        "unsafe-markup",
        file,
        3,
    );
    let message = refused(
        &[(file, Some("<div>\n<a onclick=\"go()\">x</a></div>"))],
        "unsafe-markup",
        file,
        2,
    );
    assert!(message.contains("onclick"), "{message}");
    let message = refused(
        &[(file, Some("<a\n href=\"javascript:alert(1)\">x</a>"))],
        "unsafe-url",
        file,
        2,
    );
    assert!(message.contains("javascript"), "{message}");
    refused(
        &[(file, Some("<a href=\"JaVa&#x09;script:${x}\">x</a>"))],
        "unsafe-url",
        file,
        1,
    );
    refused(
        &[(file, Some("<iframe src=\"/x\"></iframe>"))],
        "unsafe-markup",
        file,
        1,
    );
    refused(&[(file, Some("<form action=\"/x\"></form>"))], "unsafe-markup", file, 1);
    refused(&[(file, Some("<div ${attrs}>x</div>"))], "unsafe-markup", file, 1);
    refused(
        &[(
            file,
            Some("<style>\n.x { background: url(https://evil.example/a) }</style>"),
        )],
        "unsafe-css",
        file,
        2,
    );
    // Recursion through snippets, and includes deeper than the budget.
    let message = refused(
        &[(
            "snippets/price.html.fancy-locked",
            Some("\n<#include \"product-card\">"),
        )],
        "recursion",
        "snippets/price.html.fancy-locked",
        2,
    );
    assert!(message.contains("includes itself"), "{message}");
    refused(
        &[("snippets/price.html.fancy-locked", Some("<#include \"price\">"))],
        "recursion",
        "snippets/price.html.fancy-locked",
        1,
    );
    // Liquid and unknown files, scripts as assets.
    let validation = validate_stone(&[("templates/home.liquid", Some("{{ x }}")), ("assets/app.js", Some("x"))]);
    let found = errors(&validation);
    assert!(
        found
            .iter()
            .any(|error| error.starts_with("liquid templates/home.liquid")),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|error| error.starts_with("unknown-file assets/app.js")),
        "{found:?}"
    );
}

#[test]
fn an_optional_section_failure_is_isolated_and_a_required_one_is_not() {
    // The story section throws at render; the rest of the page renders.
    let package = stone_with(&[(
        "sections/hero.html.fancy-locked",
        Some("<section>${section.settings.missing.deep}</section>"),
    )]);
    let rendered = render(&package, &product_request());
    assert!(rendered.ok, "{:?}", rendered.diagnostics);
    assert_eq!(rendered.omitted.len(), 1);
    assert_eq!(rendered.omitted[0].section, "story");
    assert!(rendered.html.contains("<h1>Stone mug"), "{}", rendered.html);
    assert!(rendered.html.contains("class=\"grid\""));
    assert!(rendered
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "render-failed" && diagnostic.section.as_deref() == Some("story")));
    // The main section is required, so its failure makes the page unusable for a purchase.
    let package = stone_with(&[(
        "sections/product-main.html.fancy-locked",
        Some("<p>${product.missing.deep}</p>"),
    )]);
    let rendered = render(&package, &product_request());
    assert!(!rendered.ok);
    assert!(
        rendered.failed_required.contains(&"section:main".to_string()),
        "{:?}",
        rendered.failed_required
    );
    assert!(rendered
        .failed_required
        .contains(&"component:shop.productPurchase".to_string()));
    assert!(
        rendered.html.contains("class=\"grid\""),
        "the rest of the page is still there"
    );
    // A required component that failed, or that no section places.
    let mut request = product_request();
    request.components = json!({"shop.productPurchase": {"error": "variant service down"}});
    let rendered = render(&stone(), &request);
    assert!(!rendered.ok);
    assert_eq!(rendered.failed_required, vec!["component:shop.productPurchase"]);
    let package = stone_with(&[(
        "sections/product-main.html.fancy-locked",
        Some("<h1>${product.name}</h1>"),
    )]);
    let rendered = render(&package, &product_request());
    assert!(!rendered.ok);
    assert_eq!(rendered.failed_required, vec!["component:shop.productPurchase"]);
    // The layout failing fails the page.
    let package = stone_with(&[("layouts/store.html.fancy-locked", Some("${site.nothing.here}"))]);
    let rendered = render(&package, &product_request());
    assert!(!rendered.ok && rendered.html.is_empty());
}

#[test]
fn each_budget() {
    let lowered = |limits: Json| {
        let mut request = product_request();
        request.limits = Limits::default().lowered_by(&limits);
        render(&stone(), &request)
    };
    // Sections on a page.
    let rendered = lowered(json!({"maxSections": 2}));
    assert_eq!(rendered.omitted.len(), 1);
    assert_eq!(rendered.omitted[0].section, "related");
    assert!(rendered.omitted[0].reason.contains("over 2 sections"));
    // Blocks in a section.
    let rendered = lowered(json!({"maxBlocks": 1}));
    assert_eq!(
        rendered.omitted[0].block.as_deref(),
        Some("b2"),
        "{:?}",
        rendered.omitted
    );
    // Composition depth: blocks are at depth 3.
    let rendered = lowered(json!({"maxDepth": 2}));
    assert_eq!(rendered.omitted[0].section, "story", "{:?}", rendered.omitted);
    assert!(rendered.omitted[0].reason.contains("nested deeper"));
    // Include depth: the grid includes product-card, which includes price.
    let rendered = lowered(json!({"maxIncludeDepth": 1}));
    assert_eq!(rendered.omitted[0].section, "related", "{:?}", rendered.omitted);
    assert!(rendered.omitted[0].reason.contains("deeper than 1"));
    // Output: a section over the budget is left out; a page over it is not rendered.
    let rendered = lowered(json!({"maxOutputBytes": 800}));
    assert!(rendered.ok, "{:?}", rendered.diagnostics);
    assert!(
        rendered.html.len() <= 800 && rendered.html.contains("<h1>"),
        "{}",
        rendered.html
    );
    assert!(
        rendered
            .omitted
            .iter()
            .all(|omitted| omitted.reason.contains("800 bytes")),
        "{:?}",
        rendered.omitted
    );
    assert!(!rendered.omitted.is_empty());
    let rendered = lowered(json!({"maxOutputBytes": 100}));
    assert!(!rendered.ok && rendered.html.is_empty());
    // Time: a section that loops forever stops at its budget and is left out; the rest renders.
    let looping = stone_with(&[(
        "sections/hero.html.fancy-locked",
        // Catastrophic backtracking: runs for ever without allocating.
        Some("<section>${/(a+)+$/.test('a'.repeat(60) + 'b')}</section>"),
    )]);
    let mut request = product_request();
    request.limits = Limits::default().lowered_by(&json!({"renderTimeMs": 150}));
    let started = std::time::Instant::now();
    let rendered = render(&looping, &request);
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert!(rendered.ok, "{:?}", rendered.diagnostics);
    assert_eq!(rendered.omitted[0].section, "story");
    assert!(
        rendered.omitted[0].reason.contains("time budget"),
        "{:?}",
        rendered.omitted
    );
    assert!(rendered.html.contains("class=\"grid\""));
    // Memory: a section that builds a large string fails at the lowered limit.
    let hungry = stone_with(&[(
        "sections/hero.html.fancy-locked",
        Some("<section>${'x'.repeat(4000000).length}</section>"),
    )]);
    let mut request = product_request();
    request.limits = Limits::default().lowered_by(&json!({"memoryBytes": 2000000}));
    let rendered = render(&hungry, &request);
    assert_eq!(
        rendered.omitted.first().map(|omitted| omitted.section.as_str()),
        Some("story"),
        "{:?}",
        rendered.omitted
    );
    // A limit of 0 is the least, not QuickJS's "no limit": nothing renders.
    request.limits = Limits::default().lowered_by(&json!({"memoryBytes": 0}));
    assert!(!render(&hungry, &request).ok);
    // The renderer still renders at its own limits afterwards.
    assert!(render(&stone(), &product_request()).ok);
}

#[test]
fn limits_are_only_lowered() {
    let limits = Limits::default().lowered_by(&json!({"maxSections": 500, "maxDepth": 3, "pageTimeMs": 60000}));
    assert_eq!(limits.max_sections, 100);
    assert_eq!(limits.max_depth, 3);
    assert_eq!(limits.page_time, Duration::from_secs(5));
    let conf = r#"<moqui-conf><resource-facade><theme-limits max-sections="20" max-output-bytes="99999999" render-time-ms="x"/></resource-facade></moqui-conf>"#;
    let limits = Limits::from_conf(conf);
    assert_eq!(limits.max_sections, 20);
    assert_eq!(limits.max_output_bytes, 2 * 1024 * 1024);
    assert_eq!(limits.render_time, Limits::default().render_time);
}

#[test]
fn settings_are_checked_by_their_schemas() {
    let product = |sections: &str| {
        let text = format!(r#"{{"sections": {sections}, "order": ["main", "story"]}}"#);
        errors(&validate_stone(&[("templates/product.json", Some(&text))]))
    };
    let main = r#""main": {"type": "product-main"}"#;
    let found = product(&format!(
        r##"{{{main}, "story": {{"type": "hero", "settings": {{"background": "red"}}}}}}"##
    ));
    assert!(found.iter().any(|error| error.contains("#rgb")), "{found:?}");
    let found = product(&format!(
        r#"{{{main}, "story": {{"type": "hero", "settings": {{"heading": "a\nb"}}}}}}"#
    ));
    assert!(found.iter().any(|error| error.contains("one line")), "{found:?}");
    let found = product(&format!(
        r#"{{{main}, "story": {{"type": "hero", "settings": {{"destination": "nowhere"}}}}}}"#
    ));
    assert!(found.iter().any(|error| error.contains("kind:id")), "{found:?}");
    let found = product(&format!(
        r#"{{{main}, "story": {{"type": "hero", "settings": {{"image": "assets/none.png"}}}}}}"#
    ));
    assert!(found.iter().any(|error| error.contains("not an asset")), "{found:?}");
    let found = product(&format!(
        r#"{{{main}, "story": {{"type": "hero", "blocks": {{"b": {{"type": "text"}}}}}}}}"#
    ));
    assert!(
        found.iter().any(|error| error.contains("`body` is required")),
        "{found:?}"
    );
    let found = product(&format!(r#"{{{main}, "story": {{"type": "nothing"}}}}"#));
    assert!(found.iter().any(|error| error.starts_with("unknown-type")), "{found:?}");
    let found = product(&format!(
        r#"{{{main}, "story": {{"type": "hero"}}, "story": {{"type": "hero"}}}}"#
    ));
    assert!(found.iter().any(|error| error.starts_with("duplicate-id")), "{found:?}");
    // Product-main is allowed only on product pages.
    let found = errors(&validate_stone(&[(
        "templates/home.json",
        Some(r#"{"sections": {"m": {"type": "product-main"}}}"#),
    )]));
    assert!(found.iter().any(|error| error.starts_with("not-allowed")), "{found:?}");
    // A schema's own definitions: an unknown type, an enum without options.
    let found = errors(&validate_stone(&[(
        "sections/hero.schema.json",
        Some(
            r#"{"settings": [{"id": "a", "type": "javascript"}, {"id": "b", "type": "enum"}], "blocks": [{"type": "text"}]}"#,
        ),
    )]));
    assert!(
        found.iter().any(|error| error.contains("type `javascript`")),
        "{found:?}"
    );
    assert!(found.iter().any(|error| error.contains("needs options")), "{found:?}");
    // An explicit false or empty value is kept; the default applies only when the value is absent.
    let package = stone_with(&[(
        "templates/home.json",
        Some(r#"{"sections": {"hero": {"type": "hero", "settings": {"heading": ""}}}}"#),
    )]);
    let mut request = product_request();
    request.page_kind = Some("home".into());
    let rendered = render(&package, &request);
    assert!(rendered.html.contains("<h2></h2>"), "{}", rendered.html);
}

#[test]
fn fragments_stay_in_text() {
    // A fragment placed in an attribute is refused, so the block that does it is left out.
    let package = stone_with(&[(
        "blocks/text.html.fancy-locked",
        Some("<p title=\"${block.settings.rich}\">x</p>"),
    )]);
    let rendered = render(&package, &product_request());
    let b2 = rendered
        .omitted
        .iter()
        .find(|omitted| omitted.block.as_deref() == Some("b2"));
    assert!(
        b2.is_some_and(|omitted| omitted.reason.contains("fragment")),
        "{:?}",
        rendered.omitted
    );
    // A reserved character in data cannot forge one.
    let mut request = product_request();
    request.view["product"]["name"] = json!("\u{E000}x:0\u{E001}");
    let rendered = render(&stone(), &request);
    assert!(rendered.ok);
    assert!(rendered.html.contains("<h1>x:0</h1>"), "{}", rendered.html);
}

#[test]
fn svg_and_css_assets() {
    let dirty = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 4 4\" onload=\"alert(1)\">\n<script>alert(1)</script>\n<use href=\"https://evil.example/s.svg#a\"/><rect width=\"4\" height=\"4\" style=\"fill:red\"/><foreignObject><p>x</p></foreignObject></svg>";
    let clean = sanitize_svg(dirty);
    assert_eq!(
        clean,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 4 4\">\n\n<use/><rect width=\"4\" height=\"4\" style=\"fill:red\"/></svg>"
    );
    let validation = validate_stone(&[("assets/logo.svg", Some(dirty))]);
    let found = errors(&validation);
    assert!(
        found
            .iter()
            .any(|error| error.starts_with("unsafe-svg assets/logo.svg:1") && error.contains("onload")),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|error| error.starts_with("unsafe-svg assets/logo.svg:2") && error.contains("<script>")),
        "{found:?}"
    );
    let found = errors(&validate_stone(&[(
        "assets/store.css",
        Some("a { color: red }\n@import url(https://evil.example/x.css);\n.b { background: url(missing.png) }\n.c { background: u\\72l(//evil.example/y) }"),
    )]));
    assert!(
        found
            .iter()
            .any(|error| error.starts_with("unsafe-css assets/store.css:2")),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|error| error.starts_with("missing-file assets/store.css:3")),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|error| error.starts_with("unsafe-css assets/store.css:4") && error.contains("origin")),
        "{found:?}"
    );
    assert_eq!(sanitize_rich_text("<p onclick=x>a<script>b</script></p>"), "<p>a</p>");
}

/// The errors of the stone package with `changes`, as `code file:line message`, which must include one with `needle`.
fn refuses(changes: &[(&str, Option<&str>)], needle: &str) {
    let found = errors(&validate_stone(changes));
    assert!(
        found.iter().any(|error| error.contains(needle)),
        "no `{needle}` in {found:?}"
    );
}

/// The check reads markup with a tokenizer, not a tree builder: what a browser parses differently inside an SVG
/// (`<style>`, `<title>` and `<textarea>` content as markup, animation) is refused wherever it stands, and a fragment
/// cannot be put inside an SVG or leave one open for what follows it.
#[test]
fn foreign_content_cannot_hide_markup() {
    let block = "blocks/text.html.fancy-locked";
    // `</svg>` inside an HTML element of an SVG's <desc> does not close it, so this <style> is the SVG's.
    refuses(
        &[(
            block,
            Some("<svg><desc><div></svg></div></desc><style><img src=x onerror=alert(1)></style>"),
        )],
        "a `<` inside <style>",
    );
    refuses(
        &[(block, Some("<title><img src=x onerror=alert(1)></title>"))],
        "a tag inside <title>",
    );
    refuses(
        &[(block, Some("<textarea><img src=x onerror=alert(1)></textarea>"))],
        "a tag inside <textarea>",
    );
    refuses(
        &[(
            block,
            Some("<a href=\"#\"><animate attributeName=\"href\" values=\"javascript:alert(1)\"/>x</a>"),
        )],
        "<animate> is not allowed",
    );
    refuses(
        &[(
            block,
            Some("<svg><rect fill=\"url(https://evil.example/p.svg#a)\"/></svg>"),
        )],
        "`fill` attribute of <rect>",
    );
    // A text with an escaped `<` is still text.
    let rendered = render(&stone(), &product_request());
    assert!(
        rendered.html.contains("<title>Stone &lt;Market&gt;</title>"),
        "{}",
        rendered.html
    );

    // A block placed inside the section's SVG, and a block that leaves an SVG open, are left out.
    let inside = stone_with(&[(
        "sections/hero.html.fancy-locked",
        Some("<section><svg><#list blocks as block>${block.html}</#list></svg></section>"),
    )]);
    let rendered = render(&inside, &product_request());
    assert!(
        rendered
            .omitted
            .iter()
            .any(|omitted| omitted.section == "story" && omitted.reason.contains("inside an <svg>")),
        "{:?}",
        rendered.omitted
    );
    let open = stone_with(&[(block, Some("<svg><g>"))]);
    let rendered = render(&open, &product_request());
    assert!(
        rendered
            .omitted
            .iter()
            .any(|omitted| omitted.block.as_deref() == Some("b1") && omitted.reason.contains("not closed")),
        "{:?}",
        rendered.omitted
    );
}

#[test]
fn urls_forms_and_assembled_expressions() {
    let block = "blocks/text.html.fancy-locked";
    // A browser reads `\` as `/` in these URLs.
    refuses(
        &[(block, Some("<img src=\"\\\\evil.example/x.png\" alt=\"\">"))],
        "not approved",
    );
    refuses(
        &[(block, Some("<img src=\"/\\evil.example/x.png\" alt=\"\">"))],
        "not approved",
    );
    let found = errors(&validate(
        &stone_with(&[(
            block,
            Some("<img src=\"https://evil.example\\@cdn.example/x.png\" alt=\"\">"),
        )]),
        &ValidateOptions {
            allowed_origins: vec!["https://cdn.example".into()],
            ..ValidateOptions::default()
        },
    ));
    assert!(found.iter().any(|error| error.contains("not approved")), "{found:?}");
    // A control cannot join a trusted component's form.
    refuses(
        &[(block, Some("<button form=\"checkout\" type=\"submit\">Go</button>"))],
        "attribute `form`",
    );
    // A property named by a template literal.
    refuses(&[(block, Some("${block[`constructor`]}"))], "`constructor`");
    // An expression only the assembly makes: the file's `${"` and `"}` around an include.
    refuses(
        &[
            (block, Some("<p>${\"<#include \"leak\">\"}</p>")),
            (
                "snippets/leak.html.fancy-locked",
                Some("\" + block.constructor.constructor(\"return 1\")() + \""),
            ),
        ],
        "with its snippets put in",
    );
    // An SVG asset in another namespace loses it.
    assert_eq!(
        sanitize_svg("<svg xmlns=\"http://www.w3.org/1999/xhtml\"><rect/></svg>"),
        "<svg><rect/></svg>"
    );
}
