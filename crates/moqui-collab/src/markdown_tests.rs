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

use super::*;

fn round(markdown: &str) -> String {
    let doc = Doc::new();
    seed(&doc, markdown);
    render(&doc)
}

/// The structure y-prosemirror reads: element names, attributes and the marks of each text run.
fn outline(doc: &Doc) -> String {
    fn walk<T: ReadTxn>(txn: &T, node: &impl XmlFragment, out: &mut String) {
        for child in node.children(txn) {
            match child {
                XmlOut::Element(element) => {
                    out.push('<');
                    out.push_str(element.tag());
                    let mut attributes: Vec<(String, String)> = element
                        .attributes(txn)
                        .map(|(key, value)| (key.to_string(), value.to_string(txn)))
                        .collect();
                    attributes.sort();
                    for (key, value) in attributes {
                        out.push_str(&format!(" {key}={value}"));
                    }
                    out.push('>');
                    walk(txn, &element, out);
                    out.push_str(&format!("</{}>", element.tag()));
                }
                XmlOut::Text(text) => {
                    for diff in text.diff(txn, |_| ()) {
                        let mut names: Vec<String> = diff
                            .attributes
                            .map(|map| map.keys().map(|key| key.to_string()).collect())
                            .unwrap_or_default();
                        names.sort();
                        out.push_str(&format!("[{}]{}", names.join(","), diff.insert.to_string(txn)));
                    }
                }
                XmlOut::Fragment(_) => {}
            }
        }
    }
    let fragment = doc.get_or_insert_xml_fragment(FRAGMENT);
    let txn = doc.transact();
    let mut out = String::new();
    walk(&txn, &fragment, &mut out);
    out
}

#[test]
fn the_editor_structure_of_a_page() {
    let doc = Doc::new();
    seed(
        &doc,
        "# Title\n\nSome **bold** and [a link](https://example.com).\n\n- one\n- two\n",
    );
    assert_eq!(
        outline(&doc),
        "<heading level=1>[]Title</heading><paragraph>[]Some [bold]bold[] and [link]a link[].</paragraph>\
         <bulletList><listItem><paragraph>[]one</paragraph></listItem><listItem><paragraph>[]two</paragraph></listItem></bulletList>"
    );
}

#[test]
fn normalized_markdown_reads_back_as_it_was() {
    for text in [
        "# Title\n\nA paragraph with **bold**, *italic*, ~~gone~~ and `code`.\n",
        "## Lists\n\n- one\n- two\n  - nested\n\n1. first\n2. second\n",
        "> A quote\n>\n> - in a list\n",
        "```rust\nfn main() {}\n```\n",
        "Line one\\\nline two\n",
        "---\n\n![A cat](/img/cat.png)\n",
        "| A | B |\n| --- | --- |\n| 1 | 2 |\n",
        "A [[Wiki Link]] and [text](Other/Page) and <u>under</u> and snake_case_name.\n",
        "**bold *both* bold**\n",
    ] {
        assert_eq!(round(text), text, "{text}");
    }
}

#[test]
fn markdown_is_normalized_once_then_stable() {
    let source = "Title\n=====\n\n* star bullet\n+ plus bullet\n\n__strong__ and _em_\n\n    indented code\n";
    let once = round(source);
    assert_eq!(
        once,
        "# Title\n\n- star bullet\n\n* plus bullet\n\n**strong** and *em*\n\n```\nindented code\n```\n"
    );
    assert_eq!(round(&once), once);
}

#[test]
fn text_that_looks_like_markdown_is_escaped() {
    let doc = Doc::new();
    let fragment = doc.get_or_insert_xml_fragment(FRAGMENT);
    {
        let mut txn = doc.transact_mut();
        let paragraph = fragment.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        paragraph.push_back(
            &mut txn,
            XmlTextPrelim::new("# not a heading, 2*3*4, [x](y) and 1. not a list"),
        );
    }
    let text = render(&doc);
    assert_eq!(text, "\\# not a heading, 2\\*3\\*4, \\[x](y) and 1. not a list\n");
    assert_eq!(round(&text), text);
}

/// A document of `levels` elements named `tag`, each in the one before, the last holding the text `deep`.
fn nested(tag: &str, levels: usize) -> Doc {
    let doc = Doc::new();
    let fragment = doc.get_or_insert_xml_fragment(FRAGMENT);
    {
        let mut txn = doc.transact_mut();
        let mut node = fragment.push_back(&mut txn, XmlElementPrelim::empty(tag));
        for _ in 1..levels {
            node = node.push_back(&mut txn, XmlElementPrelim::empty(tag));
        }
        node.push_back(&mut txn, XmlTextPrelim::new("deep"));
    }
    doc
}

/// `body` on a thread with a small stack, as a worker's: a recursion once a level would overflow it.
fn on_small_stack<R: Send + 'static>(body: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(body)
        .expect("a thread")
        .join()
        .expect("no panic")
}

#[test]
fn quotes_nested_past_the_cap_are_written_as_text() {
    // one past the cap: the innermost quote is its text, inside the quotes the cap allows
    let doc = nested("blockquote", MAX_DEPTH + 1);
    assert_eq!(render(&doc), format!("{}deep\n", "> ".repeat(MAX_DEPTH)));
    // at the cap every quote is written
    let doc = nested("blockquote", MAX_DEPTH);
    assert_eq!(render(&doc), format!("{}deep\n", "> ".repeat(MAX_DEPTH)));
    // inline elements in a paragraph past the cap: their text, escaped
    let doc = nested("paragraph", MAX_DEPTH + 2);
    assert_eq!(render(&doc), "deep\n");
}

#[test]
fn a_document_nested_far_past_the_cap_renders_in_a_small_stack() {
    // an editor's update may nest elements to any depth; the render does not recurse past the cap
    let text = on_small_stack(|| {
        let quotes = render(&nested("blockquote", 50_000));
        let unknown = render(&nested("panel", 50_000));
        (quotes, unknown)
    });
    assert_eq!(text.0, format!("{}deep\n", "> ".repeat(MAX_DEPTH)));
    assert_eq!(text.1, "deep\n");
}

#[test]
fn a_page_text_nested_past_the_cap_is_read_in_a_small_stack() {
    // one past the cap: the innermost quote is a paragraph of the one around it
    let text = format!("{}deep\n", "> ".repeat(MAX_DEPTH + 1));
    assert_eq!(round(&text), format!("{}deep\n", "> ".repeat(MAX_DEPTH)));
    // lists, each item holding the next list
    let mut lists = String::new();
    for level in 0..=MAX_DEPTH {
        lists.push_str(&format!("{}- level {level}\n", "  ".repeat(level)));
    }
    let out = round(&lists);
    assert!(out.contains("level 32"), "{out}");
    // a list and its item are two levels
    assert_eq!(
        out.lines().filter(|line| line.trim_start().starts_with("- ")).count(),
        MAX_DEPTH / 2,
        "{out}"
    );
    // far past it
    let deep = on_small_stack(|| round(&format!("{}deep\n", ">".repeat(20_000))));
    assert_eq!(deep, format!("{}deep\n", "> ".repeat(MAX_DEPTH)));
}

#[test]
fn an_empty_page_is_one_empty_paragraph() {
    let doc = Doc::new();
    seed(&doc, "");
    assert_eq!(outline(&doc), "<paragraph></paragraph>");
    assert_eq!(render(&doc), "");
}
