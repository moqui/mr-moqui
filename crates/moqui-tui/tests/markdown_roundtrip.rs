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

//! The check of plans/archive/Screen-TUI-Impl.md 3.8: Revue's `RichTextEditor` reads Markdown and writes it back. The samples
//! are the Gallery's Markdown view, the example wiki pages and documents of this repository.

use revue::widget::RichTextEditor;

fn samples() -> Vec<(String, String)> {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = Vec::new();
    let widgets =
        std::fs::read_to_string(repo.join("runtime/base-component/example/screen/Gallery/Widgets.xml")).unwrap();
    let start = widgets.find("<set field=\"markdownText\" value=\"").unwrap() + 33;
    let end = start + widgets[start..].find("\"/>").unwrap();
    let text = widgets[start..end]
        .replace("&#10;", "\n")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&");
    out.push(("Gallery markdown-view".to_string(), text));
    let wiki = std::fs::read_to_string(repo.join("runtime/base-component/example/data/ExampleWikiData.xml")).unwrap();
    for (at, part) in wiki.split("<![CDATA[").skip(1).enumerate() {
        let page = part.split("]]>").next().unwrap_or("");
        out.push((format!("wiki page {at}"), page.to_string()));
    }
    for doc in ["docs/TUI.md", "plans/README.md"] {
        out.push((doc.to_string(), std::fs::read_to_string(repo.join(doc)).unwrap()));
    }
    out
}

/// What the check found on Oct 21, 2026 (Revue 3.10.1): the Gallery sample, the wiki pages and plans/README.md come
/// back as they went in. A line that starts with digits and has `. ` later in it is read as a numbered item and
/// rewritten (`00:53` needs no quotes. An answer is:` comes back as `1. An answer is:`), and inline markup is kept as
/// text but drawn plain. So the TUI keeps the Markdown as the value and styles its source (3.8) rather than edit through
/// RichTextEditor. When an upgrade changes either result, this test says so.
#[test]
fn rich_text_editor_round_trip() {
    let round = |text: &str| RichTextEditor::new().from_markdown(text.to_string()).to_markdown();
    for (name, text) in samples() {
        let back = round(&text);
        if name == "docs/TUI.md" {
            continue;
        }
        assert_eq!(back.trim_end(), text.trim_end(), "{name}");
    }
    let lossy = "00:53 is the time. Then save.";
    assert_ne!(
        round(lossy).trim_end(),
        lossy,
        "a digit line is no longer taken for a numbered item"
    );
}
