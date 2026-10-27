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

//! The local origin of an offline client spec (`spec({ server: false })`, plans/archive/NodeDependency.md).
//!
//! Pages open at [`ORIGIN`]. Nothing listens: the browser's Fetch domain asks here, and the answer is the file or a
//! 404. `/js`, `/css` and `/libs` are the webroot. `/files/<repo-relative>` is the repo, read-only. A `..` segment is a
//! 404. `/favicon.ico` is an empty 204, because the browser asks for one on its own.

use std::path::{Path, PathBuf};

/// The origin offline pages load from. `.test` is reserved for this kind of name.
pub const ORIGIN: &str = "https://moqui.test";

/// Where an offline page's URLs are read from.
#[derive(Clone, Debug)]
pub struct Roots {
    /// `runtime/base-component/webroot/screen/webroot`.
    pub webroot: PathBuf,
    /// The repo root, parent of `runtime` when `crates/` is there.
    pub repo: PathBuf,
}

/// One fulfilled response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

impl Roots {
    /// The webroot under `runtime`, and the repo that contains it.
    pub fn from_runtime(runtime: &Path) -> Self {
        let runtime = std::fs::canonicalize(runtime).unwrap_or_else(|_| runtime.to_path_buf());
        let webroot = runtime.join("base-component/webroot/screen/webroot");
        let repo = runtime
            .parent()
            .filter(|parent| parent.join("crates").is_dir() || parent.join("Cargo.toml").is_file())
            .unwrap_or(&runtime)
            .to_path_buf();
        let canon = |path: PathBuf| std::fs::canonicalize(&path).unwrap_or(path);
        Self {
            webroot: canon(webroot),
            repo: canon(repo),
        }
    }
}

/// `https://moqui.test` plus `path` (`/js/moqui-html.js`, or `js/moqui-html.js`).
pub fn url(path: &str) -> String {
    if path.is_empty() || path == "/" {
        format!("{ORIGIN}/")
    } else if let Some(rest) = path.strip_prefix('/') {
        format!("{ORIGIN}/{rest}")
    } else {
        format!("{ORIGIN}/{path}")
    }
}

/// The response for `url`. `content` is the document `setContent` stored for [`ORIGIN`]/`.
pub fn answer(roots: &Roots, url: &str, content: Option<&[u8]>) -> Answer {
    let Some(path) = request_path(url) else {
        return not_found();
    };
    if path == "/" {
        return match content {
            Some(body) => Answer {
                status: 200,
                content_type: "text/html; charset=utf-8".to_string(),
                body: body.to_vec(),
            },
            None => not_found(),
        };
    }
    if path == "/favicon.ico" {
        return Answer {
            status: 204,
            content_type: "image/x-icon".to_string(),
            body: Vec::new(),
        };
    }
    let Some(file) = mapped(roots, &path) else {
        return not_found();
    };
    let root = if path.starts_with("/files/") {
        &roots.repo
    } else {
        &roots.webroot
    };
    let (Ok(root), Ok(file)) = (std::fs::canonicalize(root), std::fs::canonicalize(&file)) else {
        return not_found();
    };
    if !file.starts_with(&root) || !file.is_file() {
        return not_found();
    }
    match std::fs::read(&file) {
        Ok(body) => Answer {
            status: 200,
            content_type: content_type(&file).to_string(),
            body,
        },
        _ => not_found(),
    }
}

fn not_found() -> Answer {
    Answer {
        status: 404,
        content_type: "text/plain; charset=utf-8".to_string(),
        body: b"not found".to_vec(),
    }
}

/// The path of a `https://moqui.test` URL, or none for any other URL. The query and fragment are dropped.
fn request_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix(ORIGIN)?;
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    if rest.is_empty() {
        return Some("/".to_string());
    }
    if !rest.starts_with('/') || rest.contains('\\') || rest.contains('\0') {
        return None;
    }
    Some(rest.to_string())
}

/// The file for a path on the origin, or none when it leaves the two trees.
fn mapped(roots: &Roots, path: &str) -> Option<PathBuf> {
    let rel = path.trim_start_matches('/');
    if let Some(rest) = rel.strip_prefix("files/") {
        under(&roots.repo, rest)
    } else if rel.starts_with("js/") || rel.starts_with("css/") || rel.starts_with("libs/") {
        under(&roots.webroot, rel)
    } else {
        None
    }
}

/// `root`/`rel` when `rel` has no empty, `.` or `..` segment and the result stays under `root`.
fn under(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() {
        return None;
    }
    let parts: Vec<&str> = rel.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return None;
    }
    let mut full = root.to_path_buf();
    for part in parts {
        full.push(part);
    }
    full.starts_with(root).then_some(full)
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> (TempDir, Roots) {
        let dir = std::env::temp_dir().join(format!("moqui-offline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let webroot = dir.join("webroot");
        let repo = dir.join("repo");
        std::fs::create_dir_all(webroot.join("css")).unwrap();
        std::fs::create_dir_all(repo.join("crates/moqui-lib/testdata")).unwrap();
        std::fs::write(webroot.join("css/moqui-html.css"), b"body{color:red}").unwrap();
        std::fs::write(repo.join("crates/moqui-lib/testdata/format-vectors.json"), b"[1]").unwrap();
        std::fs::write(repo.join("secret.txt"), b"no").unwrap();
        (TempDir { dir }, Roots { webroot, repo })
    }

    struct TempDir {
        dir: PathBuf,
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn the_webroot_and_the_repo_are_served_and_a_parent_segment_is_not() {
        let (_keep, roots) = roots();
        let css = answer(&roots, "https://moqui.test/css/moqui-html.css", None);
        assert_eq!(css.status, 200);
        assert_eq!(css.content_type, "text/css; charset=utf-8");
        assert_eq!(css.body, b"body{color:red}");
        let json = answer(
            &roots,
            "https://moqui.test/files/crates/moqui-lib/testdata/format-vectors.json?x=1",
            None,
        );
        assert_eq!(json.status, 200);
        assert_eq!(json.body, b"[1]");
        assert_eq!(
            answer(&roots, "https://moqui.test/files/../secret.txt", None).status,
            404
        );
        assert_eq!(
            answer(&roots, "https://moqui.test/css/../../secret.txt", None).status,
            404
        );
        assert_eq!(answer(&roots, "https://moqui.test/secret.txt", None).status, 404);
        assert_eq!(answer(&roots, "https://evil.test/css/moqui-html.css", None).status, 404);
        let page = answer(&roots, "https://moqui.test/", Some(b"<p>hi</p>"));
        assert_eq!(page.status, 200);
        assert_eq!(page.body, b"<p>hi</p>");
        assert_eq!(answer(&roots, "https://moqui.test/", None).status, 404);
        assert_eq!(answer(&roots, "https://moqui.test/favicon.ico", None).status, 204);
    }
}
