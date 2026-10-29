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

//! CommonMark to HTML, the `.md` and `.markdown` template renderers.
//!
//! Java renders these with flexmark, the tables and TOC extensions, and raw HTML passed through. This
//! is CommonMark plus GitHub tables, and raw HTML is passed through as Java does: a markdown file is
//! trusted like any other template file. The `[TOC]` block of the flexmark TOC extension is not
//! rendered; it stays a paragraph.
//!
//! A base link URL (Java's `sri.baseLinkUrl`, which the wiki screens set to the wiki space) resolves the relative
//! link and image destinations of the markdown against it. Links inside raw HTML are left as written.

use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};

/// Render `source` as HTML: CommonMark with tables. Raw HTML blocks and inline HTML are written as is.
pub fn to_html(source: &str) -> String {
    to_html_with_base(source, None)
}

/// [`to_html`] with each relative link and image destination resolved against `base`, a directory (a `/` is
/// added when it does not end with one). `None` or an empty base leaves destinations as written.
pub fn to_html_with_base(source: &str, base: Option<&str>) -> String {
    match base.filter(|base| !base.is_empty()) {
        Some(base) => to_html_mapped(source, |dest| resolve_link(base, dest)),
        None => to_html_mapped(source, |_| None),
    }
}

/// [`to_html`] with each link and image destination passed to `map`, which returns a replacement or `None` to keep it.
pub fn to_html_mapped(source: &str, mut map: impl FnMut(&str) -> Option<String>) -> String {
    let parser = Parser::new_ext(source, Options::ENABLE_TABLES).map(|event| match event {
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let dest_url = map(&dest_url).map_or(dest_url, CowStr::from);
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            })
        }
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let dest_url = map(&dest_url).map_or(dest_url, CowStr::from);
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            })
        }
        event => event,
    });
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

/// `dest` resolved against the directory `base` when it is relative: not empty, not starting with `/`, `#` or `?`
/// (so not `//host` either), and with no scheme (`mailto:`, `https:`, `data:`). The query and fragment of `dest`
/// are kept, and `.` and `..` segments are taken out, never above the root of the base. A base with a scheme and
/// host keeps them. `None` when `dest` is not relative.
pub fn resolve_link(base: &str, dest: &str) -> Option<String> {
    if dest.is_empty() || dest.starts_with(['/', '#', '?']) || has_scheme(dest) {
        return None;
    }
    // The base without its query or fragment, split into origin (`https://host`) and path.
    let base = base.split_once(['?', '#']).map_or(base, |(base, _)| base);
    let (origin, base_path) = match base.split_once("://") {
        Some((_, after)) if has_scheme(base) => {
            let host = after.find('/').unwrap_or(after.len());
            base.split_at(base.len() - after.len() + host)
        }
        _ => ("", base),
    };
    let split = dest.find(['?', '#']).unwrap_or(dest.len());
    let (dest_path, suffix) = dest.split_at(split);
    let joined = if base_path.ends_with('/') || base_path.is_empty() {
        format!("{base_path}{dest_path}")
    } else {
        format!("{base_path}/{dest_path}")
    };
    let rooted = joined.starts_with('/') || !origin.is_empty();
    let mut segments: Vec<&str> = Vec::new();
    let pieces: Vec<&str> = joined.split('/').filter(|piece| !piece.is_empty()).collect();
    let directory = joined.ends_with('/') || matches!(pieces.last(), Some(&".") | Some(&".."));
    for piece in pieces {
        match piece {
            "." => {}
            ".." => {
                segments.pop();
            }
            piece => segments.push(piece),
        }
    }
    let mut out = String::with_capacity(origin.len() + joined.len() + suffix.len());
    out.push_str(origin);
    if rooted {
        out.push('/');
    }
    out.push_str(&segments.join("/"));
    if directory && !segments.is_empty() {
        out.push('/');
    }
    out.push_str(suffix);
    Some(out)
}

/// Whether `url` starts with a scheme: `[A-Za-z][A-Za-z0-9+.-]*:` before any `/`, `?` or `#`.
fn has_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '-'))
}

#[cfg(test)]
mod tests {
    use super::{resolve_link, to_html, to_html_with_base};

    #[test]
    fn heading_and_list_become_html() {
        let html = to_html("# Title\n\n- one\n- two\n");
        assert!(html.contains("<h1>Title</h1>"), "{html}");
        assert!(html.contains("<li>one</li>"), "{html}");
        assert!(html.contains("<li>two</li>"), "{html}");
    }

    #[test]
    fn tables_and_raw_html_pass_through() {
        let html =
            to_html("| a | b |\n|---|---|\n| 1 | 2 |\n\n<div class=\"note\">kept</div>\n\ntext <b>bold</b> & more\n");
        assert!(html.contains("<table>") && html.contains("<td>1</td>"), "{html}");
        assert!(html.contains("<div class=\"note\">kept</div>"), "{html}");
        assert!(html.contains("text <b>bold</b> &amp; more"), "{html}");
    }

    #[test]
    fn relative_links_and_images_resolve_against_the_base() {
        let base = Some("/apps/wiki/SPACE");
        let html = to_html_with_base("[x](Page) [o](../Other/Page) ![i](img.png?x=1#y)\n", base);
        assert!(html.contains("<a href=\"/apps/wiki/SPACE/Page\">x</a>"), "{html}");
        assert!(html.contains("<a href=\"/apps/wiki/Other/Page\">o</a>"), "{html}");
        assert!(
            html.contains("<img src=\"/apps/wiki/SPACE/img.png?x=1#y\" alt=\"i\" />"),
            "{html}"
        );
        // A base ending in `/` is the same directory.
        let html = to_html_with_base("[x](./sub/Page)\n", Some("/apps/wiki/SPACE/"));
        assert!(html.contains("href=\"/apps/wiki/SPACE/sub/Page\""), "{html}");
    }

    #[test]
    fn absolute_links_are_kept() {
        let source = "[a](/root) [b](#top) [c](?q=1) [d](//cdn.example.com/x) [e](mailto:a@b.c) \
                      [f](https://example.com/p) ![g](data:image/png;base64,AA)\n";
        assert_eq!(to_html_with_base(source, Some("/apps/wiki/SPACE")), to_html(source));
        // No base, or an empty one, leaves relative links as written.
        let relative = "[x](Page)\n";
        assert_eq!(to_html_with_base(relative, None), "<p><a href=\"Page\">x</a></p>\n");
        assert_eq!(to_html_with_base(relative, Some("")), to_html(relative));
    }

    #[test]
    fn dot_segments_stop_at_the_root_and_a_full_base_keeps_its_host() {
        assert_eq!(resolve_link("/a/b", "../../../c").as_deref(), Some("/c"));
        assert_eq!(resolve_link("/a/b", "..").as_deref(), Some("/a/"));
        assert_eq!(
            resolve_link("https://host.example/apps/wiki?x=1", "../Page#s").as_deref(),
            Some("https://host.example/apps/Page#s")
        );
        assert_eq!(
            resolve_link("https://host.example", "Page").as_deref(),
            Some("https://host.example/Page")
        );
        // `a/b:c` has a slash before its colon, so it is a relative path.
        assert_eq!(resolve_link("/w", "a/b:c").as_deref(), Some("/w/a/b:c"));
        assert_eq!(resolve_link("/w", "C:x"), None);
    }
}
