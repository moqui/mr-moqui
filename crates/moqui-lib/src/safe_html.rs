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
