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
//! the desktop and mobile client's offline Compose quotes (`moqui_client::offline`), so both clean alike.

/// `text` cleaned to the safe set: tags and attributes not on the list removed, script and style content gone with
/// its element, and links without unsafe URLs.
pub fn clean(text: &str) -> String {
    cleaner().clean(text).to_string()
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
}
