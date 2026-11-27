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

//! HTML cleaned to the safe set: what an `allow-html="safe"` service parameter takes (`moqui_core::service`), and what
//! the desktop and mobile client's offline Compose quotes (`moqui_client::offline`), so both clean alike. Markdown's raw
//! HTML cleaned the same way ([`clean_markdown`]), for a wiki page of a space that does not allow any HTML.

use pulldown_cmark::{Event, Options, Parser, Tag};

/// `text` cleaned to the safe set: tags and attributes not on the list removed, script and style content gone with
/// its element, and links without unsafe URLs. Images of any `http` or `https` address stay: public pages, a shop's
/// descriptions, a theme and outgoing mail show them as written. App text read by signed-in users is [`clean_app`].
pub fn clean(text: &str) -> String {
    cleaner().clean(text).to_string()
}

/// `text` cleaned to the safe set for app text read by signed-in users (a note, a comment, a review, a chat-like
/// message): as [`clean`], and an image keeps its `src` only when it is a path of this server ([`own_image_src`]). An
/// outside image would tell its host each reader's address and when they read (plans/archive/Security-Review-Medium.md
/// SEC-CLI-82). What an `allow-html="app"` service parameter takes.
pub fn clean_app(text: &str) -> String {
    app_cleaner().clean(text).to_string()
}

/// How many times [`clean_markdown`] cleans before it writes the raw HTML left as text: removing a tag joins the text
/// around it, which may make a new one (`<scr<!-- -->ipt>`), so it cleans again until nothing changes.
const MARKDOWN_PASSES: usize = 6;

/// Markdown `source` with the raw HTML the safe set would change cleaned to it, as [`clean`] cleans HTML
/// (plans/archive/Security-Review.md SEC-SCR-81). A source whose rendered HTML holds nothing the safe set takes out comes back
/// as it was, so a `<details>` block, a table, a link or `<b>` is kept as written; otherwise each piece of raw HTML
/// that holds something it takes out (a block whole, an inline tag in the context it needs, a table cell's in a
/// table) is cleaned, until the HTML holds nothing more; raw HTML still changing after a few passes is written as
/// text (`&lt;`). The links the markdown itself makes are the renderer's (`markdown::to_html_mapped`). A wiki page is
/// cleaned again when it is rendered (`WikiServices.render#WikiPage`), which is where its viewers are kept safe; this
/// keeps the stored text so too.
pub fn clean_markdown(source: &str) -> String {
    let mut text = source.to_string();
    for _ in 0..MARKDOWN_PASSES {
        if !holds_unsafe(&crate::markdown::to_html(&text)) {
            return text;
        }
        match markdown_edits(&text, clean_raw_html) {
            Some(next) => text = next,
            // what is left is not raw HTML (a markdown image's `data:` source): the renderer's
            None => return text,
        }
    }
    markdown_edits(&text, |raw, _| raw.replace('<', "&lt;")).unwrap_or(text)
}

/// Whether `html` holds something the safe set takes out: it cleans to other HTML than a cleaner that keeps every
/// tag, attribute and URL scheme in it, which writes the same HTML ammonia's parser and serializer make.
pub fn holds_unsafe(html: &str) -> bool {
    clean(html) != keep_all(html)
}

/// `html` through a cleaner that keeps every tag of it, every attribute and every URL scheme it names (also through
/// a character reference), with the safe set's link `rel` and comments taken out, so it differs from [`clean`] only by
/// what the safe set takes out.
fn keep_all(html: &str) -> String {
    let mut tags = tag_names(html);
    // the parser adds these where a table needs them
    tags.extend(["tbody".to_string(), "colgroup".to_string()]);
    let schemes = url_schemes(html);
    const PREFIXES: &str = "!#$%&()*+,-.0123456789:;<?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";
    let prefixes: std::collections::HashSet<&str> = PREFIXES
        .char_indices()
        .map(|(at, ch)| &PREFIXES[at..at + ch.len_utf8()])
        .collect();
    let mut builder = ammonia::Builder::empty();
    builder
        .tags(tags.iter().map(String::as_str).collect())
        .clean_content_tags(std::collections::HashSet::new())
        .generic_attribute_prefixes(prefixes)
        .url_schemes(schemes.iter().map(String::as_str).collect())
        .url_relative(ammonia::UrlRelative::PassThrough)
        .link_rel(Some("noopener noreferrer"))
        .strip_comments(true);
    builder.clean(html).to_string()
}

/// The element names of the tags in `html`, lower case: each `<` or `</` with a letter after it, to the next
/// space, `/` or `>`, as the HTML tokenizer reads a tag name.
fn tag_names(html: &str) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for (at, _) in html.match_indices('<') {
        let rest = &html[at + 1..];
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        if !rest.starts_with(|ch: char| ch.is_ascii_alphabetic()) {
            continue;
        }
        let name: String = rest
            .chars()
            .take_while(|ch| !ch.is_whitespace() && *ch != '/' && *ch != '>')
            .collect::<String>()
            .to_ascii_lowercase();
        names.insert(name);
    }
    names
}

/// Every word before a `:` in `html`, lower case, also with character references and the tabs and line ends a
/// browser takes out of a URL first, so each scheme a URL of it may have is there.
fn url_schemes(html: &str) -> std::collections::HashSet<String> {
    let decoded = decode_references(html).replace(['\t', '\n', '\r'], "");
    let mut schemes = std::collections::HashSet::new();
    for text in [html, decoded.as_str()] {
        let bytes = text.as_bytes();
        for (at, _) in text.match_indices(':') {
            let start = bytes[..at]
                .iter()
                .rposition(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-')))
                .map_or(0, |i| i + 1);
            if let Some(word) = text.get(start..at).filter(|word| !word.is_empty()) {
                schemes.insert(word.to_ascii_lowercase());
            }
        }
    }
    schemes
}

/// `text` with numeric character references (`&#58;`, `&#x3a;`) and `&colon;`, `&Tab;` and `&NewLine;` decoded.
fn decode_references(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest.find(';').filter(|end| *end <= 12);
        let decoded = end.and_then(|end| {
            let name = &rest[1..end];
            match name {
                "colon" => Some(':'),
                "Tab" => Some('\t'),
                "NewLine" => Some('\n'),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                    .and_then(char::from_u32),
            }
        });
        match (decoded, end) {
            (Some(ch), Some(end)) => {
                out.push(ch);
                rest = &rest[end + 1..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `source` with each raw HTML range replaced by `edit(raw, inline)`, or None when no replacement changes anything.
fn markdown_edits(source: &str, edit: impl Fn(&str, bool) -> String) -> Option<String> {
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut block_end = 0;
    for (event, range) in Parser::new_ext(source, Options::ENABLE_TABLES).into_offset_iter() {
        if range.start < block_end {
            continue;
        }
        let inline = match event {
            Event::Start(Tag::HtmlBlock) => false,
            Event::Html(_) | Event::InlineHtml(_) => true,
            _ => continue,
        };
        let Some(raw) = source.get(range.clone()) else {
            continue;
        };
        let mut cleaned = edit(raw, inline);
        if !inline {
            block_end = range.end;
            // the block keeps its line end, so the markdown after it starts its own line
            if raw.ends_with('\n') && !cleaned.ends_with('\n') {
                cleaned.push('\n');
            }
        }
        if cleaned != raw {
            edits.push((range, cleaned));
        }
    }
    if edits.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(source.len());
    let mut at = 0;
    for (range, cleaned) in edits {
        out.push_str(source.get(at..range.start)?);
        out.push_str(&cleaned);
        at = range.end;
    }
    out.push_str(source.get(at..)?);
    Some(out)
}

/// The table elements a tag of that name needs around it to be parsed at all, as opening and closing text.
fn table_context(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "td" | "th" => Some(("<table><tbody><tr>", "</tr></tbody></table>")),
        "tr" => Some(("<table><tbody>", "</tbody></table>")),
        "tbody" | "thead" | "tfoot" | "caption" | "colgroup" | "col" => Some(("<table>", "</table>")),
        _ => None,
    }
}

/// One piece of raw HTML, kept as written when it holds nothing the safe set takes out (in the table context a table
/// part needs), else cleaned: a block as HTML; an inline comment or declaration taken out, a closing tag kept when its
/// element is on the list, an opening tag cleaned without the end tag the cleaner adds.
fn clean_raw_html(raw: &str, inline: bool) -> String {
    let element = |text: &str| -> String {
        text.chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let first = raw
        .strip_prefix('<')
        .map(|rest| element(rest.strip_prefix('/').unwrap_or(rest)))
        .unwrap_or_default();
    let (open, close) = table_context(&first).unwrap_or(("", ""));
    if !holds_unsafe(&format!("{open}{raw}{close}")) {
        return raw.to_string();
    }
    if !inline {
        return clean(raw);
    }
    if raw.starts_with("<!") || raw.starts_with("<?") {
        return String::new();
    }
    // the element cleaned alone (in its table context) keeps its start tag
    let allowed = |name: &str| {
        let (open, close) = table_context(name).unwrap_or(("", ""));
        !name.is_empty() && clean(&format!("{open}<{name}></{name}>{close}")).contains(&format!("</{name}>"))
    };
    if raw.starts_with("</") {
        return if allowed(&first) {
            format!("</{first}>")
        } else {
            String::new()
        };
    }
    let cleaned = clean(&format!("{open}{raw}{close}"));
    let inner = cleaned
        .strip_prefix(open)
        .and_then(|rest| rest.strip_suffix(close))
        .unwrap_or(if open.is_empty() { cleaned.as_str() } else { "" });
    match inner.strip_suffix(&format!("</{first}>")) {
        Some(start) if !first.is_empty() => start.to_string(),
        _ => inner.to_string(),
    }
}

/// The cleaner. ammonia's defaults are a maintained, conservative allow-list, so there is no
/// list of our own to keep up to date, with three additions on a `span`: `data-mention-id` (a person,
/// `<span data-mention-id="john.doe">@John Doe</span>`), and `data-record-entity` with `data-record-id`
/// (a record, `<span data-record-entity="moqui.test.Order" data-record-id="100050">#SO100050</span>`).
/// A record span is not a mention. The values are only ever text: ammonia escapes them. Tables (`table`,
/// `thead`, `tbody`, `tr`, `th`, `td`, with `colspan` and `rowspan`) are in ammonia's defaults. A `class` on `code`,
/// `pre` and `span` keeps only a code block's language and highlighting names (`language-js`, `lang-rust`, `hljs`,
/// `hljs-keyword`, [`code_class`]), what markdown writes for a fenced block, so a wiki page keeps them when it is
/// cleaned; any other class (a layout class of the page's own CSS) is taken out, and `style` and `id` stay out.
fn cleaner() -> &'static ammonia::Builder<'static> {
    static CLEANER: std::sync::OnceLock<ammonia::Builder<'static>> = std::sync::OnceLock::new();
    CLEANER.get_or_init(|| safe_builder(false))
}

fn app_cleaner() -> &'static ammonia::Builder<'static> {
    static CLEANER: std::sync::OnceLock<ammonia::Builder<'static>> = std::sync::OnceLock::new();
    CLEANER.get_or_init(|| safe_builder(true))
}

/// The safe set's builder; with `own_images` an image keeps only a `src` of this server ([`clean_app`]).
fn safe_builder(own_images: bool) -> ammonia::Builder<'static> {
    let mut builder = ammonia::Builder::default();
    builder.add_tag_attributes("span", &["data-mention-id", "data-record-entity", "data-record-id"]);
    for tag in ["code", "pre", "span"] {
        builder.add_tag_attributes(tag, &["class"]);
    }
    builder.attribute_filter(move |element, attribute, value| {
        if own_images && element == "img" && attribute == "src" {
            return own_image_src(value).then(|| value.into());
        }
        if attribute != "class" {
            return Some(value.into());
        }
        let kept: Vec<&str> = value.split_ascii_whitespace().filter(|name| code_class(name)).collect();
        (!kept.is_empty()).then(|| kept.join(" ").into())
    });
    builder
}

/// Whether an image `src` of [`clean_app`] is this server's: a path with no scheme or host (`/apps/...`,
/// `/richTextImage/1`, `a.png`), never `https://`, `//host` or a backslash. A browser's URL parser takes tabs and line
/// ends out anywhere and spaces and C0 controls off both ends before it reads a URL, so `/\t/evil.example/x.png` would be
/// another host's: a `src` with any control or white space character is refused whole (plans/archive/Security-Review-Medium.md
/// SEC-CLI-82). A path of this server is a same-origin GET, which a transition that writes refuses when the browser
/// loads it as an image (`moqui_core::web::loaded_as_subresource`).
fn own_image_src(src: &str) -> bool {
    if src.is_empty() || src.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return false;
    }
    let before_path = src.split(['/', '?', '#']).next().unwrap_or_default();
    !src.starts_with("//") && !src.contains('\\') && !before_path.contains(':')
}

/// Whether `name` is a code block's language or highlighting class: `language-` or `lang-` and a name of letters,
/// digits and `+#_.-`, or `hljs` and its token classes.
fn code_class(name: &str) -> bool {
    let language = name.strip_prefix("language-").or_else(|| name.strip_prefix("lang-"));
    if let Some(language) = language {
        return !language.is_empty()
            && language.len() <= 40
            && language
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+#_.-".contains(&b));
    }
    name == "hljs"
        || name.strip_prefix("hljs-").is_some_and(|token| {
            !token.is_empty()
                && token
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        })
}

/// `text` cleaned for a chat message (SEC-COM-02): the safe set, and an image only when its `src` is this server's own
/// rich-text image route (`/richTextImage/<id>`, what the composer's image upload answers). Any other image loses its
/// `src`, so a reader's browser fetches nothing from a host the sender names, which would tell the sender each reader's
/// address and when they read.
pub fn clean_chat(text: &str) -> String {
    chat_cleaner().clean(text).to_string()
}

/// Whether `src` is an image path of this server that a chat message may show: `/richTextImage/` and an id of letters,
/// digits, `-` and `_`.
pub fn chat_image_src(src: &str) -> bool {
    src.strip_prefix("/richTextImage/")
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}

fn chat_cleaner() -> &'static ammonia::Builder<'static> {
    static CLEANER: std::sync::OnceLock<ammonia::Builder<'static>> = std::sync::OnceLock::new();
    CLEANER.get_or_init(|| {
        let mut builder = ammonia::Builder::default();
        builder.add_tag_attributes("span", &["data-mention-id", "data-record-entity", "data-record-id"]);
        builder.attribute_filter(|element, attribute, value| {
            if element == "img" && attribute == "src" && !chat_image_src(value) {
                None
            } else {
                Some(value.into())
            }
        });
        builder
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chat_message_keeps_only_this_servers_images() {
        let own = r#"<p><img src="/richTextImage/100123" alt="chart"></p>"#;
        assert_eq!(clean_chat(own), own);
        for outside in [
            "https://host.example/x.png?u=1",
            "http://host.example/x.png",
            "//host.example/x.png",
            "/richTextImage/../apps/x",
            "/richTextImage/1?x=2",
            "/apps/report/export",
        ] {
            let cleaned = clean_chat(&format!(r#"<p>hi <img src="{outside}" alt="a"></p>"#));
            assert!(!cleaned.contains("src="), "{outside}: {cleaned}");
            assert!(cleaned.contains("hi"), "{cleaned}");
        }
        // mentions survive
        let mention = r#"<p><span data-mention-id="john.doe">@John</span></p>"#;
        assert_eq!(clean_chat(mention), mention);
    }

    /// SEC-CLI-82: app text keeps an image only from this server: an outside one, which would tell the writer who
    /// read the text and when, loses its `src`, also one a browser would read as another host's once it takes tabs,
    /// line ends and end controls out; a path of this server is kept (a transition that writes refuses it as an image,
    /// `moqui_core::web::loaded_as_subresource`). The public safe set keeps an outside image.
    #[test]
    fn app_text_keeps_only_this_servers_images() {
        for own in [
            "/richTextImage/100123",
            "/apps/wiki/attachment?name=a.png",
            "a.png",
            "Page/_attachments/b.png",
        ] {
            let html = format!(r#"<p><img src="{own}" alt="a"></p>"#);
            assert!(clean_app(&html).contains("src="), "{own}: {}", clean_app(&html));
        }
        for outside in [
            "https://host.example/x.png?u=1",
            "http://host.example/x.png",
            "//host.example/x.png",
            "HTTPS://host.example/x.png",
            "\\\\host.example\\x.png",
            "data:image/png;base64,AAAA",
            "/&#9;/evil.example/x.png",
            "/&#10;/evil.example/x.png",
            "&#1;//evil.example/x.png",
            " //evil.example/x.png",
            "&#0;https://evil.example/x.png",
        ] {
            let cleaned = clean_app(&format!(r#"<p>hi <img src="{outside}" alt="a"></p>"#));
            assert!(!cleaned.contains("src="), "{outside}: {cleaned}");
        }
        // public HTML (a shop's description, a theme, an outgoing mail) keeps an outside image
        assert!(clean(r#"<img src="https://host.example/x.png">"#).contains(r#"src="https://host.example/x.png""#));
    }

    /// A code block keeps its language and highlighting classes; any other class, a `style` and an `id` go.
    #[test]
    fn a_code_block_keeps_its_language_class_only() {
        assert_eq!(
            clean(r#"<pre><code class="language-rust">fn x()</code></pre>"#),
            r#"<pre><code class="language-rust">fn x()</code></pre>"#
        );
        assert_eq!(
            clean(r#"<span class="hljs-keyword fixed-full z-max">fn</span>"#),
            r#"<span class="hljs-keyword">fn</span>"#
        );
        assert_eq!(clean(r#"<span class="fixed-full">x</span>"#), "<span>x</span>");
        assert_eq!(
            clean(r#"<p class="language-js" id="a" style="position:fixed">x</p>"#),
            "<p>x</p>"
        );
        assert_eq!(
            clean(r#"<code class="language-x&quot;onclick">x</code>"#),
            "<code>x</code>"
        );
    }

    /// Markdown's raw HTML is cleaned and the markdown around it kept; one with none comes back as it was.
    #[test]
    fn markdown_raw_html_is_cleaned_and_the_markdown_kept() {
        let plain = "# Title\n\n> a < b && c\n\n```\n<script>kept in code</script>\n```\n\n- `<b>` [x](Page)\n";
        assert_eq!(clean_markdown(plain), plain);
        let source = "# Hi\n\n<script>alert(1)</script>\n\ntext <b onclick=\"x()\">bold</b> <img src=\"a.png\" onerror=\"x()\">\n";
        let cleaned = clean_markdown(source);
        assert!(cleaned.starts_with("# Hi\n\n"), "{cleaned}");
        assert!(
            !cleaned.contains("script") && !cleaned.contains("onclick") && !cleaned.contains("onerror"),
            "{cleaned}"
        );
        assert!(
            cleaned.contains("<b>bold</b>") && cleaned.contains(r#"<img src="a.png">"#),
            "{cleaned}"
        );
        // a tag made by joining the text around one taken out is cleaned too
        let joined = clean_markdown("x <scr<!-- -->ipt>alert(1)</script>\n");
        assert!(!joined.contains("<script"), "{joined}");
        let block = "<div onmouseover=\"x()\">\n<a href=\"javascript:x()\">a</a>\n</div>\n";
        let html = crate::markdown::to_html(&clean_markdown(block));
        assert!(!html.contains("onmouseover") && !html.contains("javascript"), "{html}");
        // a scheme written as a character reference is still a scheme
        let encoded = clean_markdown("x <a href=\"java&#115;cript:alert(1)\">a</a>\n");
        assert!(!encoded.contains("href"), "{encoded}");
    }

    /// The HTML the safe set keeps is kept as written: a block split by markdown, a table, a link (which the cleaner
    /// would give a `rel`), table cells inline, a line break.
    #[test]
    fn markdown_with_safe_html_comes_back_as_it_was() {
        for kept in [
            "<details>\n<summary>More</summary>\n\nSome *markdown* here.\n\n</details>\n",
            "A [link](https://x.example), <a href=\"https://x.example/p\">a raw one</a> and <b>bold</b> <i>text</i>.\n",
            "<table>\n<tr><th>a</th><td>b</td></tr>\n</table>\n",
            "| a | b |\n|---|---|\n| <b>1</b> | <code>2</code> |\n",
            "<table>\n<tr>\n<td>\n\n*cell markdown*\n\n</td>\n</tr>\n</table>\n",
            "one<br>\ntwo<br/>\n<!-- a note -->\n",
        ] {
            assert_eq!(clean_markdown(kept), kept);
        }
    }
}
