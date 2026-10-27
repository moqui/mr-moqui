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

//! The `html-print` mode and its older name `xsl-fo`, the fancy renderers in screens, the email
//! screens, the PDF stub, and `dynamic-options` through a test render.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use moqui_lib::object::Value;

use crate::context::ExecutionContextFactory;
use crate::screen::{make_render, PdfRenderer, RenderOutput, ScreenTest, PDF_UNAVAILABLE};

fn render_root() -> String {
    format!("{}/src/screen/testdata/Render/Render.xml", env!("CARGO_MANIFEST_DIR"))
}

fn framework_screen(name: &str) -> String {
    format!(
        "{}/../../runtime/base-component/framework/screen/{name}.xml",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn strings(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
        .collect()
}

fn render(factory: &std::sync::Arc<ExecutionContextFactory>, path: &str) -> RenderOutput {
    make_render(factory)
        .root_screen(render_root())
        .screen_path(path)
        .render()
        .unwrap_or_else(|err| panic!("{path}: {err}"))
}

/// What the print document holds between its `<body>` and `</body>`.
fn print_body(output: &RenderOutput) -> String {
    let text = output.body_text();
    let start = text.find("<body").expect("a body");
    let after = &text[start..];
    let content = &after[after.find('>').unwrap() + 1..];
    content
        .trim_end_matches("</body></html>\n")
        .trim_start_matches('\n')
        .to_string()
}

#[test]
fn the_print_document_has_a_page_for_each_paper_and_the_chosen_layout() {
    let context = ExecutionContextFactory::test_context();
    let output = render(context.factory(), "/Labels.html-print");
    assert_eq!(output.content_type(), Some("text/html;charset=UTF-8"));
    let text = output.body_text();
    assert!(
        text.starts_with("<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\">"),
        "{text}"
    );
    for (name, size) in [
        ("letter-portrait", "8.5in 11in"),
        ("letter-landscape", "11in 8.5in"),
        ("legal-portrait", "8.5in 14in"),
        ("legal-landscape", "14in 8.5in"),
        ("tabloid-portrait", "11in 17in"),
        ("tabloid-landscape", "17in 11in"),
        ("a4-portrait", "210mm 297mm"),
        ("a4-landscape", "297mm 210mm"),
        ("a3-portrait", "297mm 420mm"),
        ("a3-landscape", "420mm 297mm"),
    ] {
        assert!(text.contains(&format!("@page {name} {{ size: {size};")), "{name}");
    }
    // The default page is letter portrait with a page number in the footer.
    assert!(text
        .contains("@page { size: 8.5in 11in; margin: 0.25in; @bottom-center { content: \"- \" counter(page) \" -\""));
    assert!(text.contains("<body class=\"letter-portrait\">"));
    assert!(text.contains("table.form-list thead { display: table-header-group;"));
    // layoutMaster and documentTitle come from the parameters, which are in the context.
    let landscape = make_render(context.factory())
        .root_screen(render_root())
        .screen_path("/Labels.html-print")
        .parameters(strings(&[
            ("layoutMaster", "a4-landscape"),
            ("documentTitle", "Quarterly \"report\""),
        ]))
        .render()
        .unwrap()
        .body_text();
    assert!(
        landscape.contains("<title>Quarterly &quot;report&quot;</title>"),
        "{landscape}"
    );
    assert!(
        landscape.contains("@page { size: 297mm 210mm; margin: 15mm; @top-center { content: \"Quarterly  report \"")
    );
    assert!(landscape.contains("<body class=\"a4-landscape\">"));
    // Only one document is written even though the render goes through several screens and sections.
    assert_eq!(text.matches("<!DOCTYPE").count(), 1);
    assert_eq!(text.matches("</html>").count(), 1);
}

#[test]
fn xsl_fo_is_the_same_body_as_html_print_everywhere_the_mode_is_named() {
    let context = ExecutionContextFactory::test_context();
    let print = render(context.factory(), "/Forms.html-print");
    assert_eq!(render(context.factory(), "/Forms.xsl-fo").body, print.body);
    for parameter in ["html-print", "xsl-fo"] {
        let by_parameter = make_render(context.factory())
            .root_screen(render_root())
            .screen_path("/Forms")
            .parameters(strings(&[("renderMode", parameter)]))
            .render()
            .unwrap();
        assert_eq!(by_parameter.body, print.body, "renderMode={parameter}");
        assert_eq!(by_parameter.content_type(), Some("text/html;charset=UTF-8"));
    }
    let by_builder = make_render(context.factory())
        .root_screen(render_root())
        .screen_path("/Forms")
        .render_mode("xsl-fo")
        .render()
        .unwrap();
    assert_eq!(by_builder.body, print.body);
    let facade = crate::screen::ScreenFacade::of(context.factory());
    assert!(facade.is_render_mode_valid("xsl-fo") && facade.is_render_mode_valid("html-print"));
    assert_eq!(facade.mime_type("xsl-fo"), facade.mime_type("html-print"));
    assert_eq!(crate::screen::canonical_render_mode("xsl-fo"), "html-print");
    assert_eq!(crate::screen::canonical_render_mode("csv"), "csv");
}

#[test]
fn text_types_name_html_print_or_its_older_name() {
    let context = ExecutionContextFactory::test_context();
    let dir = std::env::temp_dir().join(format!("moqui-print-types-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Types.xml");
    std::fs::write(
        &path,
        r#"<screen require-authentication="false"><widgets><render-mode>
            <text type="html">only html;</text>
            <text type="xsl-fo">old name;</text>
        </render-mode>
        <render-mode><text type="html-print">new name;</text></render-mode>
        <render-mode><text type="csv,xsl-fo">in a list;</text></render-mode>
        <render-mode><text type="text">only text;</text></render-mode>
        <render-mode><text type="html-print">first;</text><text type="csv, xsl-fo">last typed;</text>
            <text>fallback;</text><text type="any">last fallback;</text></render-mode>
        <render-mode><text>first fallback;</text><text type="any">fallback only;</text>
            <text type="text">unmatched;</text></render-mode></widgets></screen>"#,
    )
    .unwrap();
    let location = path.to_string_lossy().into_owned();
    let print = |mode: &str| {
        let output = make_render(context.factory())
            .root_screen(location.clone())
            .render_mode(mode)
            .render()
            .unwrap();
        print_body(&output)
    };
    // The last typed match wins even when fallbacks follow it; print aliases match in type lists too.
    let expected = "old name;new name;in a list;last typed;fallback only;";
    assert_eq!(print("html-print"), expected);
    assert_eq!(print("xsl-fo"), expected);
    let csv = make_render(context.factory())
        .root_screen(location)
        .render_mode("csv")
        .render()
        .unwrap();
    assert_eq!(csv.body_text(), "in a list;last typed;fallback only;");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn labels_containers_and_sections_in_html_print() {
    let context = ExecutionContextFactory::test_context();
    assert_eq!(
        print_body(&render(context.factory(), "/Labels.html-print")),
        "<div>Hello Ada</div>\n<div><div>in container</div>\n</div>\n\
         <div class=\"box-header\"><div>Header</div>\n</div>\n<div>Body</div>\n<div>in section</div>\n\
         <div>a-1-0</div>\n<div>b-2-1</div>\n"
    );
}

#[test]
fn a_form_single_and_a_form_list_in_html_print() {
    let context = ExecutionContextFactory::test_context();
    let body = print_body(&render(context.factory(), "/Forms.html-print"));
    let field = |title: &str, value: &str| {
        format!("<table class=\"field\"><tr><td class=\"field-title\">{title}</td><td class=\"field-value\">{value}</td></tr></table>\n")
    };
    let single = [
        field("Item ID", "I1"),
        field("Item Name", "Widget, &quot;large&quot;"),
        field("Status", "Active"),
        field("Flag", "Y"),
        field("Amount", "$1,234.50"),
        field("When Stamp", "2024-03-05 14:07"),
        // the hidden field and the submit write nothing
        field("Mode", "flag is Y"),
    ]
    .concat();
    assert!(body.starts_with(&single), "{body}");
    let list = &body[single.len()..];
    // Columns share the table by their character widths, and the submit column has a header and no cell.
    assert!(
        list.starts_with("<table class=\"form-list\">\n<colgroup><col style=\"width:20.65%\">"),
        "{list}"
    );
    assert!(list.contains(
        "<thead><tr><th><div style=\"text-align:left\">Item ID</div></th><th><div style=\"text-align:left\">Name</div></th>\
         <th><div style=\"text-align:right\">Price</div></th>"
    ));
    assert!(list.contains(
        "<tr><td><div style=\"text-align:left\">I1</div></td><td><div style=\"text-align:left\">Widget, &quot;large&quot;</div></td>\
         <td><div style=\"text-align:right\">12.50</div></td><td><div style=\"text-align:left\">first line</div></td>"
    ));
    assert!(list.contains("<td><div style=\"text-align:right\">3.00</div></td>"));
    assert!(list.ends_with("</tbody>\n</table>\n"), "{list}");
    assert_eq!(list.matches("<tr><td>").count(), 2);
}

#[test]
fn layout_box_panel_break_label_and_link_in_html_print() {
    let context = ExecutionContextFactory::test_context();
    let output = make_render(context.factory())
        .root_screen(render_root())
        .screen_path("/Print.html-print")
        .render()
        .unwrap();
    let body = print_body(&output);
    let expected = [
        "<div class=\"page-break-before\"><div>First page Ada</div>\n</div>\n",
        "<div class=\"box-header\"><div>Boxed</div>\n</div>\n<div>inside</div>\n",
        "<div><div>head</div>\n</div>\n",
        "<table class=\"panel\"><tr><td><div>left</div>\n</td><td><div>center</div>\n</td></tr></table>\n",
        "<div><div>foot</div>\n</div>\n",
        // a field-group has a bold title, a field-row is a table of cells, and a big row has a title cell
        "<div class=\"group-title\">Group</div>\n",
        "<table class=\"field-row\"><tr><td><table class=\"field\"><tr><td class=\"field-title\">Item</td><td class=\"field-value\">I9</td></tr></table>\n</td>",
        "<td><table class=\"field\"><tr><td class=\"field-title\">Name</td><td class=\"field-value\">Nine</td></tr></table>\n</td></tr></table>\n",
        "<table class=\"field-row\"><tr><td class=\"field-title\">Big</td><td><table class=\"field\"><tr><td class=\"field-title\">Price</td><td class=\"field-value\">5</td></tr></table>\n</td></tr></table>\n",
        // fields-not-referenced writes the field the layout left out; text is encoded
        "<table class=\"field\"><tr><td class=\"field-title\">Note</td><td class=\"field-value\">n&lt;&amp;&gt;</td></tr></table>\n",
        // a label of white space is not written, and text is encoded
        "<div>a &lt; b</div>\n",
        "<div><a href=\"go\">Go &amp; see</a></div>\n",
        "<div><div>tail</div>\n</div>\n",
    ]
    .concat();
    assert_eq!(body, expected);
}

struct FakePdf;

impl crate::screen::PdfEngine for FakePdf {
    fn render(&self, html: &str) -> Result<Vec<u8>, crate::context::Error> {
        Ok(format!("%PDF-fake {}", html.len()).into_bytes())
    }
}

/// One test owns the process-wide engine slot: none, a reason, then an engine.
#[test]
fn the_pdf_renderer_uses_the_installed_engine_or_reports_that_it_is_not_available() {
    let _pdf = crate::screen::pdf_test_lock();
    crate::screen::clear_pdf_engine();
    let err = PdfRenderer::render("<html></html>").unwrap_err();
    assert_eq!(PDF_UNAVAILABLE, err.message());
    assert!(!PdfRenderer::available());
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let thrown = crate::script::eval_condition("(() => { try { ec.resource.xslFoTransform('<html></html>', null, null, 'application/pdf'); return false } catch (e) { return String(e).includes('PDF is not available') } })()")
        .unwrap();
    assert!(thrown);

    crate::screen::install_pdf_unavailable("built without the pdf feature");
    assert_eq!(
        PdfRenderer::render("x").unwrap_err().message(),
        "built without the pdf feature"
    );

    crate::screen::install_pdf_engine(std::sync::Arc::new(FakePdf));
    assert!(PdfRenderer::available());
    assert_eq!(PdfRenderer::render("<p>").unwrap(), b"%PDF-fake 3");
    let bytes = crate::script::eval_condition(
        "(() => { const b = ec.resource.xslFoTransform('<p>', null, null, 'application/pdf'); return b instanceof Uint8Array && b[0] == 37 && b.length == 11 })()",
    )
    .unwrap();
    assert!(bytes);
    let refused = crate::script::eval_condition(
        "(() => { try { ec.resource.xslFoTransform('<p>', null, null, 'application/postscript'); return false } catch (e) { return String(e).includes('application/pdf') } })()",
    )
    .unwrap();
    assert!(refused);
    crate::screen::clear_pdf_engine();
}

/// `ec.resource.code128Svg` as the print templates call it: an inline SVG; no barcode, not an error, for empty text or
/// text outside ASCII; an error for an option it does not know.
#[test]
fn code128_svg_draws_the_barcode_a_template_asks_for() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let drawn = crate::script::eval_condition(
        "(() => { const svg = ec.resource.code128Svg('55500', {height: '0.4in', moduleWidth: '0.3mm', fontSize: '12pt', humanReadable: true}); \
         return svg.startsWith('<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"29.7mm\" height=\"10.16mm\"') && svg.includes('>55500</text>') })()",
    )
    .unwrap();
    assert!(drawn);
    let plain = crate::script::eval_condition(
        "(() => { const svg = ec.resource.code128Svg(100045, {humanReadable: false, quietZone: false}); \
         return svg.startsWith('<svg') && !svg.includes('<text') && svg.includes('<rect x=\"0\"') })()",
    )
    .unwrap();
    assert!(plain);
    let nothing = crate::script::eval_condition(
        "ec.resource.code128Svg(null) === '' && ec.resource.code128Svg('') === '' && ec.resource.code128Svg('caf\\u00e9') === ''",
    )
    .unwrap();
    assert!(nothing);
    let refused = crate::script::eval_condition(
        "(() => { try { ec.resource.code128Svg('1', {barWidth: 2}); return false } catch (e) { return String(e).includes('unknown option barWidth') } })()",
    )
    .unwrap();
    assert!(refused);
}

/// Keeps the last document it was given.
#[derive(Default)]
struct CapturePdf(std::sync::Mutex<String>);

impl crate::screen::PdfEngine for CapturePdf {
    fn render(&self, html: &str) -> Result<Vec<u8>, crate::context::Error> {
        *moqui_lib::sync::lock(&self.0) = html.to_string();
        Ok(b"%PDF-capture".to_vec())
    }
}

#[test]
fn resource_images_reach_the_pdf_engine_as_data_urls() {
    use base64::Engine;

    let _pdf = crate::screen::pdf_test_lock();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let dir = std::env::temp_dir().join(format!("moqui-pdf-images-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A 1x1 PNG.
    let png = base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==")
        .unwrap();
    std::fs::write(dir.join("a&b.png"), &png).unwrap();
    let svg = "<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"/>";
    std::fs::write(dir.join("mark.svg"), svg).unwrap();
    std::fs::write(dir.join("notes.txt"), "not an image").unwrap();
    let file = |name: &str| format!("file://{}", dir.join(name).display());
    let engine = std::sync::Arc::new(CapturePdf::default());
    crate::screen::install_pdf_engine(engine.clone());

    // kept as written: a data URL, an https URL, a file that is not an image, a missing file, a path with no scheme,
    // and an element that is not an img
    let kept = format!(
        "<img src=\"data:image/gif;base64,R0lG\"><img src=\"https://example.com/x.png\"><img src=\"{}\">\
         <img src=\"{}\"><img src=\"/ssstatic/x.png\"><imgx src=\"{}\">",
        file("notes.txt"),
        file("missing.png"),
        file("mark.svg"),
    );
    let html = format!(
        "<p><IMG class=\"logo\" src=\"{}\" alt=\"logo\"></p><img alt='' src='{}'/>{kept}",
        file("a&amp;b.png"),
        file("mark.svg"),
    );
    assert_eq!(PdfRenderer::render(&html).unwrap(), b"%PDF-capture");
    let expected = format!(
        "<p><IMG class=\"logo\" src=\"data:image/png;base64,{}\" alt=\"logo\"></p><img alt='' src='data:image/svg+xml;base64,{}'/>{kept}",
        base64::engine::general_purpose::STANDARD.encode(&png),
        base64::engine::general_purpose::STANDARD.encode(svg),
    );
    assert_eq!(*moqui_lib::sync::lock(&engine.0), expected);
    crate::screen::clear_pdf_engine();
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- the fancy renderers in a screen ----

#[test]
fn a_text_body_is_a_trusted_fancy_template_in_every_mode() {
    let context = ExecutionContextFactory::test_context();
    let dir = std::env::temp_dir().join(format!("moqui-fancy-screen-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Fancy.xml");
    std::fs::write(
        &path,
        r#"<screen require-authentication="false"><actions><set field="names" from="['a', 'b&lt;']"/></actions><widgets>
        <text><![CDATA[<#list names as n>${n}<#if n_has_next>,</#if></#list>|<#assign x = names.length * 2>${x}|${ec.user.username ?? 'anon'}]]></text>
        <text template="false"><![CDATA[<#list names as n>${n}</#list>]]></text>
        </widgets></screen>"#,
    )
    .unwrap();
    let location = path.to_string_lossy().into_owned();
    for mode in ["text", "csv", "xml", "html-print"] {
        let output = make_render(context.factory())
            .root_screen(location.clone())
            .render_mode(mode)
            .render()
            .unwrap();
        let body = if mode == "html-print" {
            print_body(&output)
        } else {
            output.body_text()
        };
        // The first text is rendered, the second (template false) is written as it is.
        assert!(body.contains("a,b<|4|anon"), "{mode}: {body}");
        assert!(body.contains("<#list names as n>${n}</#list>"), "{mode}: {body}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_text_location_uses_the_renderer_of_its_extension_and_ftl_is_refused() {
    let context = ExecutionContextFactory::test_context();
    let dir = std::env::temp_dir().join(format!("moqui-fancy-location-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Part.txt.fancy"), "<#if who>Hi ${who}</#if>").unwrap();
    std::fs::write(dir.join("Old.ftl"), "<#if who>Hi ${who}</#if>").unwrap();
    std::fs::write(dir.join("Other.txt"), "plain ${1 + 1}").unwrap();
    let screen = |name: &str, location: &str, template: &str| {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!(
                r#"<screen require-authentication="false"><actions><set field="who" value="Ada"/></actions><widgets>
                <text location="{}"{template}/></widgets></screen>"#,
                dir.join(location).display()
            ),
        )
        .unwrap();
        path.to_string_lossy().into_owned()
    };
    let text_as = |name: &str, location: &str, template: &str| {
        make_render(context.factory())
            .root_screen(screen(name, location, template))
            .render_mode("text")
            .render()
    };
    let text = |name: &str, location: &str| text_as(name, location, "");
    assert_eq!(text("A.xml", "Part.txt.fancy").unwrap().body_text(), "Hi Ada");
    // No renderer for the extension: the file's text, as Java writes it, so a .js file's `${` is not run.
    assert_eq!(text("B.xml", "Other.txt").unwrap().body_text(), "plain ${1 + 1}");
    // A `template` that names an extension picks that renderer, Java's default extension.
    assert_eq!(
        text_as("B2.xml", "Other.txt", r#" template="fancy""#)
            .unwrap()
            .body_text(),
        "plain 2"
    );
    let err = text("C.xml", "Old.ftl").unwrap_err();
    assert!(
        err.message().contains("FreeMarker template") && err.message().contains("fancy"),
        "{err}"
    );
    // ec.resource.template: the renderer by extension, the locked-down one with only its data.
    let _entered = context.enter();
    std::fs::write(dir.join("Locked.txt.fancy-locked"), "${typeof ec} ${name}").unwrap();
    crate::script::set_context_name("tdir", Value::string(dir.to_string_lossy())).unwrap();
    crate::script::set_context_name("who", Value::string("Ada")).unwrap();
    let rendered = crate::script::eval_value(
        "ec.resource.template(tdir + '/Part.txt.fancy') + '|' + ec.resource.template(tdir + '/Locked.txt.fancy-locked', {name: 'Zed'})",
    )
    .unwrap();
    assert_eq!(moqui_lib::object::to_plain_string(&rendered), "Hi Ada|object Zed");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- the email screens ----

/// A context for a user that exists, who the email screens render as.
fn logged_in(world: &super::World) -> crate::context::ExecutionContext {
    world.user("emailer", None);
    world.factory.new_context()
}

fn email_parameters() -> BTreeMap<String, Value> {
    let mut body = IndexMap::new();
    body.insert("zeta".to_string(), Value::string("<b>bold</b> & more"));
    body.insert("alpha".to_string(), Value::string("first"));
    body.insert("empty".to_string(), Value::string(""));
    body.insert("moquiSessionToken".to_string(), Value::string("secret"));
    let mut parameters = strings(&[
        ("title", "Daily Report"),
        ("webHostName", "example.com"),
        ("screenPath", "apps/Reports/Daily"),
    ]);
    parameters.insert("bodyParameters".to_string(), Value::Map(body));
    parameters
}

#[test]
fn screen_render_email_renders_in_text_and_html_print() {
    let world = super::World::open("");
    let context = logged_in(&world);
    let _entered = context.enter();
    let _authz = context.disable_authz();
    crate::user::internal_login_user("emailer").unwrap();
    let render_mode = |mode: &str| {
        make_render(&world.factory)
            .root_screen(framework_screen("ScreenRenderEmail"))
            .parameters(email_parameters())
            .render_mode(mode)
            .render()
            .unwrap_or_else(|err| panic!("{mode}: {err}"))
    };
    // The text body: the title, the link with the parameters sorted and the excluded one left out, then a
    // line for each parameter that has a value. The path /apps/ is changed to /qapps/ by the screen's script.
    let text = render_mode("text").body_text();
    assert_eq!(
        text,
        "Daily Report\n\nhttps://example.com/qapps/Reports/Daily?alpha=first&empty=&zeta=%3Cb%3Ebold%3C%2Fb%3E%20%26%20more\n\n\
         alpha: first\nzeta: <b>bold</b> & more\n"
    );
    // The html body is the html-print body too, and escapes the values.
    let print = render_mode("html-print");
    let body = print_body(&print);
    assert!(body.contains("<h2>Daily Report</h2>"), "{body}");
    assert!(
        body.contains("<a href=\"https://example.com/qapps/Reports/Daily?alpha=first&amp;empty=&amp;zeta=%3Cb%3Ebold%3C%2Fb%3E%20%26%20more\">"),
        "{body}"
    );
    assert!(body.contains("<div><strong>alpha</strong>: first</div>"), "{body}");
    assert!(
        body.contains("<div><strong>zeta</strong>: &lt;b&gt;bold&lt;/b&gt; &amp; more</div>"),
        "{body}"
    );
    assert!(
        !body.contains("<strong>empty</strong>") && !body.contains("secret"),
        "{body}"
    );
    assert_eq!(render_mode("xsl-fo").body, print.body);
}

#[test]
fn notification_email_renders_text_and_html_print() {
    let world = super::World::open("");
    let context = logged_in(&world);
    let _entered = context.enter();
    let _authz = context.disable_authz();
    crate::user::internal_login_user("emailer").unwrap();
    let mut parameters = strings(&[
        ("title", "Disk <full>"),
        ("topic", "SYSTEM"),
        ("topicDescription", "System"),
        ("type", "danger"),
        ("link", "/apps/System"),
        ("webHostName", "example.com"),
        ("notificationMessageId", "N1"),
    ]);
    parameters.insert("sentDate".to_string(), Value::string("2024-03-05 10:20:30.0"));
    let render_mode = |mode: &str| {
        make_render(&world.factory)
            .root_screen(framework_screen("NotificationEmail"))
            .parameters(parameters.clone())
            .render_mode(mode)
            .render()
            .unwrap_or_else(|err| panic!("{mode}: {err}"))
    };
    let text = render_mode("text").body_text();
    assert!(
        text.contains("System Notification (danger)\n\nDisk <full>\n\nhttps://example.com/apps/System\n"),
        "{text}"
    );
    assert!(text.contains("Notification Message ID N1 Sent "), "{text}");
    assert!(text.contains("to topic 'SYSTEM'"), "{text}");
    // The html body is for the html mode, which comes in the next slice, so the print mode has none.
    let print = print_body(&render_mode("html-print"));
    assert_eq!(print, "");
}

// ---- dynamic-options and the test render ----

#[test]
fn dynamic_options_come_from_a_transition_called_as_a_test_render() {
    let world = super::World::open("");
    let context = world.factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    // The drop-down shows the label of the current value. The value is not set, so the
    // no-current-selected-key picks R1-Acme out of the options the transition returned.
    let output = make_render(&world.factory)
        .root_screen(render_root())
        .screen_path("/Options.text")
        .parameters(strings(&[("company", "Acme"), ("plain", "y"), ("searched", "x")]))
        .render()
        .unwrap();
    let text = output.body_text();
    assert!(text.contains("Role one of Acme"), "{text}");
    // value-field and label-field name the keys of the entries, and server-search leaves the options to the browser.
    assert!(text.contains("Plain: Why"), "{text}");
    assert!(text.contains("Searched: x"), "{text}");
    // Without the field named in depends-on, there are no options and the key shows as it is.
    let without = make_render(&world.factory)
        .root_screen(render_root())
        .screen_path("/Options.text")
        .render()
        .unwrap()
        .body_text();
    assert!(!without.contains("Role one"), "{without}");
    assert!(without.contains("R1-Acme"), "{without}");
}

#[test]
fn a_screen_test_render_collects_errors_under_a_base_path_and_counts() {
    let world = super::World::open("");
    let context = world.factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let test = ScreenTest::new(&world.factory)
        .root_screen(render_root())
        .base_screen_path("/Options/");
    let rendered = test.test_render("roleOptions", strings(&[("company", "Initech")]), Some("post"));
    assert_eq!(rendered.status(), Some(200));
    assert!(rendered.error_messages.is_empty(), "{:?}", rendered.error_messages);
    assert!(rendered.contains("R1-Initech"), "{}", rendered.text());
    // The caller's own errors stay on the context; a render's are taken off it. (An error already on the context
    // stops a transition's actions, as in Java, so it is added after the transition render.)
    context.message().add_error("an error from before the render");
    // A failed render is an error message, not an error.
    let missing = test.test_render("nothing", BTreeMap::new(), None);
    assert!(missing.output.is_none());
    assert_eq!(missing.error_messages.len(), 1, "{:?}", missing.error_messages);
    assert!(
        missing.error_messages[0].starts_with("Exception in render of nothing: "),
        "{:?}",
        missing.error_messages
    );
    let stats = test.stats();
    assert_eq!((stats.render_count, stats.error_count), (2, 1));
    assert_eq!(stats.total_chars, rendered.text().chars().count() as u64);
    // The errors the caller had before are still there; the render's own are not.
    assert_eq!(context.message().errors(), ["an error from before the render"]);
}

#[test]
fn a_screen_test_renders_a_transition_with_parameters_and_reads_its_json() {
    let world = super::World::open("");
    let context = world.factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let test = ScreenTest::new(&world.factory).root_screen(render_root());
    let rendered = test
        .render("/Options/roleOptions", strings(&[("company", "Initech")]))
        .unwrap();
    assert_eq!(rendered.status(), 200);
    let json = rendered.json().unwrap();
    assert_eq!(json["options"][0]["value"], "R1-Initech");
    assert_eq!(json["options"][1]["label"], "Role two");
    let list = test
        .render("/Options/plainList", BTreeMap::new())
        .unwrap()
        .json_value()
        .unwrap();
    assert!(matches!(&list, Value::List(items) if items.len() == 2), "{list:?}");
    // A screen path renders as text, and a path with a query adds parameters.
    let screen = ScreenTest::new(&world.factory)
        .root_screen(render_root())
        .render_mode("text")
        .render("/Options/roleOptions?company=Q", BTreeMap::new())
        .unwrap();
    assert!(screen.json().unwrap()["options"][0]["value"] == "R1-Q");
    assert!(test.render("/Options/nothing", BTreeMap::new()).is_err());
    // The test render inside a render leaves the outer render's output alone: the options above were
    // fetched while Options.text was being written.
    let nested = make_render(&world.factory)
        .root_screen(render_root())
        .screen_path("/Options.text")
        .parameters(strings(&[("company", "Acme")]))
        .render()
        .unwrap();
    assert_eq!(nested.status, 200);
    assert!(
        nested.body_text().starts_with("      Role: ") || nested.body_text().contains("Role"),
        "{}",
        nested.body_text()
    );
}

#[test]
fn make_render_from_a_service_script_renders_text_for_a_scheduled_screen() {
    let world = super::World::open("");
    let context = logged_in(&world);
    let _entered = context.enter();
    let _authz = context.disable_authz();
    crate::user::internal_login_user("emailer").unwrap();
    let location = framework_screen("SingleUseCode");
    crate::script::set_context_name("code", Value::string("123456")).unwrap();
    crate::script::set_context_name("location", Value::string(location)).unwrap();
    let rendered = crate::script::eval_value(
        "ec.screen.makeRender().rootScreen(location).renderMode('xsl-fo').baseLinkUrl('https://example.com').render()",
    )
    .unwrap();
    let rendered = moqui_lib::object::to_plain_string(&rendered);
    assert!(rendered.starts_with("<!DOCTYPE html>"), "{rendered}");
    assert!(
        rendered.contains("<div><div>Your single use code is: 123456</div>\n</div>"),
        "{rendered}"
    );
    let mime =
        crate::script::eval_value("ec.screen.getMimeTypeByMode('xsl-fo') + ec.screen.isRenderModeText('xsl-fo')")
            .unwrap();
    assert_eq!(moqui_lib::object::to_plain_string(&mime), "text/htmltrue");
}

#[test]
fn the_render_modes_list_and_a_link_parameter_take_either_name() {
    let context = ExecutionContextFactory::test_context();
    let dir = std::env::temp_dir().join(format!("moqui-print-modes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, attrs: &str, body: &str| {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!(r#"<screen require-authentication="false" {attrs}><widgets>{body}</widgets></screen>"#),
        )
        .unwrap();
        path.to_string_lossy().into_owned()
    };
    let old = write("Old.xml", r#"render-modes="xsl-fo""#, r#"<label text="old list"/>"#);
    let new = write("New.xml", r#"render-modes="html-print""#, r#"<label text="new list"/>"#);
    let render = |location: &str, mode: &str| {
        make_render(context.factory())
            .root_screen(location)
            .render_mode(mode)
            .render()
    };
    for location in [&old, &new] {
        for mode in ["xsl-fo", "html-print"] {
            let output = render(location, mode).unwrap_or_else(|err| panic!("{location} {mode}: {err}"));
            assert!(print_body(&output).contains("list</div>"), "{location} {mode}");
        }
        let err = render(location, "csv").unwrap_err();
        assert!(err.message().contains("render mode csv is not supported"), "{err}");
    }
    // A link carries renderMode=xsl-fo as a parameter, and following it gives the print document.
    let linked = write(
        "Linked.xml",
        "",
        r#"<link url="." text="Print" url-type="plain" ><parameter name="renderMode" value="xsl-fo"/><parameter name="pageNoLimit" value="true"/></link>"#,
    );
    let page = print_body(&render(&linked, "html-print").unwrap());
    assert!(page.contains("renderMode=xsl-fo"), "{page}");
    let followed = make_render(context.factory())
        .root_screen(old.clone())
        .parameters(strings(&[("renderMode", "xsl-fo"), ("pageNoLimit", "true")]))
        .render()
        .unwrap();
    assert_eq!(print_body(&followed), print_body(&render(&old, "html-print").unwrap()));
    // A test render's path query names the mode, as Java's ScreenTest puts the query in the request parameters.
    let root = write(
        "Root.xml",
        "",
        &format!(
            r#"<subscreens-active/></widgets><subscreens><subscreens-item name="Old" location="{old}"/></subscreens><widgets>"#
        ),
    );
    let tested = ScreenTest::new(context.factory()).root_screen(root).test_render(
        "Old?renderMode=xsl-fo",
        BTreeMap::new(),
        None,
    );
    let output = tested.output.expect("a print render");
    assert!(print_body(&output).contains("old list</div>"), "{}", output.body_text());
    let _ = std::fs::remove_dir_all(&dir);
}
