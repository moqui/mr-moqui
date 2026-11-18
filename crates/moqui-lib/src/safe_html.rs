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
/// its element, and links without unsafe URLs.
pub fn clean(text: &str) -> String {
    cleaner().clean(text).to_string()
}

/// How many times [`clean_markdown`] cleans before it writes the raw HTML left as text: removing a tag joins the text
/// around it, which may make a new one (`<scr<!-- -->ipt>`), so it cleans again until nothing changes.
const MARKDOWN_PASSES: usize = 6;

/// Markdown `source` with its raw HTML cleaned to the safe set, as [`clean`] cleans HTML (plans/Security-Review.md
/// SEC-SCR-81): an HTML block cleaned whole, an inline tag kept when it is on the list (its attributes cleaned), else
/// taken out. The markdown around it is not changed, so a source with no raw HTML comes back as it was; the links the
/// markdown itself makes are cleaned by the renderer (`markdown::to_html_mapped`). Raw HTML that cleaning still
/// changes after a few passes is written as text (`&lt;`).
pub fn clean_markdown(source: &str) -> String {
    let mut text = source.to_string();
    for _ in 0..MARKDOWN_PASSES {
        match markdown_edits(&text, clean_raw_html) {
            Some(next) => text = next,
            None => return text,
        }
    }
    markdown_edits(&text, |raw, _| raw.replace('<', "&lt;")).unwrap_or(text)
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

/// One piece of raw HTML cleaned: a block as HTML; an inline comment or declaration taken out, a closing tag kept when
/// its element is on the list, an opening tag cleaned without the end tag the cleaner adds.
fn clean_raw_html(raw: &str, inline: bool) -> String {
    if !inline {
        return clean(raw);
    }
    if raw.starts_with("<!") || raw.starts_with("<?") {
        return String::new();
    }
    let element = |text: &str| -> String {
        text.chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let allowed = |name: &str| {
        let pair = format!("<{name}></{name}>");
        !name.is_empty() && clean(&pair) == pair
    };
    if let Some(rest) = raw.strip_prefix("</") {
        let name = element(rest);
        return if allowed(&name) {
            format!("</{name}>")
        } else {
            String::new()
        };
    }
    let name = raw.strip_prefix('<').map(element).unwrap_or_default();
    let cleaned = clean(raw);
    match cleaned.strip_suffix(&format!("</{name}>")) {
        Some(open) if !name.is_empty() => open.to_string(),
        _ => cleaned,
    }
}

/// The cleaner. ammonia's defaults are a maintained, conservative allow-list, so there is no
/// list of our own to keep up to date, with three additions on a `span`: `data-mention-id` (a person,
/// `<span data-mention-id="john.doe">@John Doe</span>`), and `data-record-entity` with `data-record-id`
/// (a record, `<span data-record-entity="moqui.test.Order" data-record-id="100050">#SO100050</span>`).
/// A record span is not a mention. The values are only ever text: ammonia escapes them. Tables (`table`,
/// `thead`, `tbody`, `tr`, `th`, `td`, with `colspan` and `rowspan`) are in ammonia's defaults.
fn cleaner() -> &'static ammonia::Builder<'static> {
    static CLEANER: std::sync::OnceLock<ammonia::Builder<'static>> = std::sync::OnceLock::new();
    CLEANER.get_or_init(|| {
        let mut builder = ammonia::Builder::default();
        builder.add_tag_attributes("span", &["data-mention-id", "data-record-entity", "data-record-id"]);
        builder
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
        // the safe set keeps an outside image, as any allow-html="safe" text does
        assert!(clean(r#"<img src="https://host.example/x.png">"#).contains("src="));
        // mentions survive
        let mention = r#"<p><span data-mention-id="john.doe">@John</span></p>"#;
        assert_eq!(clean_chat(mention), mention);
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
    }
}
