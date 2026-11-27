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

//! The device's map packages (plans/Maps-Offline.md §2.4, OF2): a package downloaded by ranges from a server that
//! answers as the package transition does, resumed after a cut, checked, unpacked with bounds and served by range.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use super::*;

const ACCOUNT: &str = "maps.example.com-0123456789ab";
const PACKAGE: &str = "P1";
const MAX: u64 = 1 << 30;

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-client-packages-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A zip member: its name, bytes and Unix mode.
struct Part {
    name: String,
    bytes: Vec<u8>,
    mode: u32,
}

fn part(name: &str, bytes: &[u8]) -> Part {
    Part {
        name: name.to_string(),
        bytes: bytes.to_vec(),
        mode: 0o100_644,
    }
}

/// A zip of stored members, as `writeZip` writes a package (no CRC: the package's checks are SHA-256).
fn zip(parts: &[Part]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for part in parts {
        let at = u32::try_from(out.len()).unwrap();
        let size = u32::try_from(part.bytes.len()).unwrap();
        let name = part.name.as_bytes();
        let name_len = u16::try_from(name.len()).unwrap();
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&[0; 8]); // flags, method, time, date
        out.extend_from_slice(&0u32.to_le_bytes()); // crc
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&part.bytes);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&((3u16 << 8) | 20).to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&[0; 8]); // flags, method, time, date
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&[0; 8]); // extra, comment, disk, internal
        central.extend_from_slice(&(part.mode << 16).to_le_bytes());
        central.extend_from_slice(&at.to_le_bytes());
        central.extend_from_slice(name);
    }
    let central_at = u32::try_from(out.len()).unwrap();
    let count = u16::try_from(parts.len()).unwrap();
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&u32::try_from(central.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&central_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// The basemap member: past one range, so a download takes two.
fn basemap(revision: i64) -> Vec<u8> {
    (0..RANGE_LEN as usize + 300_000)
        .map(|at| u8::try_from((at * 7 + usize::try_from(revision).unwrap()) % 251).unwrap())
        .collect()
}

const STYLE: &[u8] = br#"{"version":8,"sprite":"sprites/v4/light"}"#;

/// A package's zip: its manifest listing `listed` (path, bytes), and the zip's members `parts` beside the manifest.
fn package_with(revision: i64, listed: &[(&str, &[u8])], parts: Vec<Part>) -> Vec<u8> {
    let members: Vec<serde_json::Value> = listed
        .iter()
        .map(|(path, bytes)| serde_json::json!({"path": path, "size": bytes.len(), "sha256": sha(bytes)}))
        .collect();
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "package": {"packageId": PACKAGE, "name": "Lakes", "revision": revision, "bbox": [1, 2, 3, 4],
            "minZoom": 0, "maxZoom": 3, "builtDate": "2026-11-27T00:00:00Z"},
        "members": members,
    });
    let mut all = vec![part("manifest.json", manifest.to_string().as_bytes())];
    all.extend(parts);
    zip(&all)
}

/// A whole, good package of `revision`.
fn package(revision: i64) -> Vec<u8> {
    let tiles = basemap(revision);
    package_with(
        revision,
        &[("basemap/LIGHT.pmtiles", &tiles), ("style/light.json", STYLE)],
        vec![part("basemap/LIGHT.pmtiles", &tiles), part("style/light.json", STYLE)],
    )
}

/// What the fake server does with the next package request.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Next {
    Serve,
    /// Send this many bytes of the range, then fail.
    CutAfter(usize),
    Status(u16),
}

/// A request the fake server was asked: its path and headers.
type Asked = (String, Vec<(String, String)>);

/// The server as the package transition answers: the list, and the zip of the listed revision by range with
/// `If-Range` on its strong tag.
struct Fake {
    zip: Vec<u8>,
    listed_revision: Mutex<i64>,
    /// The tag the list names (the zip's own unless a test lies).
    tag: String,
    next: Mutex<Vec<Next>>,
    asked: Mutex<Vec<Asked>>,
}

impl Fake {
    fn new(zip: Vec<u8>, revision: i64) -> Self {
        let tag = format!("sha256-{}", sha(&zip));
        Self {
            zip,
            listed_revision: Mutex::new(revision),
            tag,
            next: Mutex::new(Vec::new()),
            asked: Mutex::new(Vec::new()),
        }
    }

    fn ranges(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _)| path.starts_with(&format!("{PACKAGE_PATH}?")))
            .filter_map(|(_, headers)| crate::proxy::header(headers, "range").map(str::to_string))
            .collect()
    }
}

/// A body that fails after its bytes.
struct Cut(Cursor<Vec<u8>>);

impl Read for Cut {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self.0.read(buffer)? {
            0 => Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "cut")),
            read => Ok(read),
        }
    }
}

fn answer(status: u16, headers: Vec<(&str, String)>, body: Vec<u8>) -> ServerAnswer {
    ServerAnswer {
        status,
        headers: headers
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
        body: Box::new(Cursor::new(body)),
    }
}

impl PackageServer for Fake {
    fn get(&self, path: &str, headers: &[(String, String)]) -> Result<ServerAnswer, String> {
        self.asked.lock().unwrap().push((path.to_string(), headers.to_vec()));
        let revision = *self.listed_revision.lock().unwrap();
        if path == LIST_PATH {
            let list = serde_json::json!({"packageList": [{
                "packageId": PACKAGE, "name": "Lakes", "statusId": "MapPkgReady", "revision": revision,
                "size": self.zip.len(), "tag": self.tag,
                "downloadUrl": format!("{PACKAGE_PATH}?packageId={PACKAGE}&revision={revision}"),
            }]});
            return Ok(answer(
                200,
                vec![("Content-Type", "application/json".to_string())],
                list.to_string().into_bytes(),
            ));
        }
        let next = {
            let mut next = self.next.lock().unwrap();
            if next.is_empty() {
                Next::Serve
            } else {
                next.remove(0)
            }
        };
        if let Next::Status(status) = next {
            return Ok(answer(status, Vec::new(), Vec::new()));
        }
        if !path.ends_with(&format!("revision={revision}")) {
            return Ok(answer(409, Vec::new(), Vec::new()));
        }
        let etag = format!("\"{}\"", self.tag);
        let range = crate::proxy::header(headers, "range").unwrap_or("");
        let if_range = crate::proxy::header(headers, "if-range");
        let len = self.zip.len();
        let (start, end) = match range.strip_prefix("bytes=").and_then(|spec| spec.split_once('-')) {
            Some((start, end)) if if_range == Some(etag.as_str()) => {
                let start: usize = start.parse().unwrap();
                let end: usize = end.parse::<usize>().map_or(len - 1, |end| end.min(len - 1));
                (start, end)
            }
            _ => return Ok(answer(200, vec![("ETag", etag)], self.zip.clone())),
        };
        let slice = self.zip[start..=end].to_vec();
        let headers = vec![("ETag", etag), ("Content-Range", format!("bytes {start}-{end}/{len}"))];
        let body: Box<dyn Read + Send> = match next {
            Next::CutAfter(bytes) => Box::new(Cut(Cursor::new(slice[..bytes].to_vec()))),
            _ => Box::new(Cursor::new(slice)),
        };
        Ok(ServerAnswer {
            status: 206,
            headers: headers
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect(),
            body,
        })
    }
}

fn packages(dir: &Path) -> Packages {
    Packages::with_free_space(dir, Box::new(|_| Some(u64::MAX)))
}

fn installed(packages: &Packages) -> Option<i64> {
    packages.installed_dir(ACCOUNT, PACKAGE).map(|(revision, _)| revision)
}

fn serve(packages: &Packages, path: &str, range: Option<&str>) -> Served {
    super::serve(
        packages,
        ACCOUNT,
        "GET",
        &format!("{PATH_PREFIX}{PACKAGE}/{path}"),
        range,
        None,
    )
}

fn header<'a>(served: &'a Served, name: &str) -> Option<&'a str> {
    served
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

#[test]
fn a_package_downloads_by_ranges_with_its_tag_and_installs() {
    let dir = dir("install");
    let packages = packages(&dir);
    let server = Fake::new(package(1), 1);
    let len = server.zip.len() as u64;
    assert_eq!(
        download(&packages, &server, ACCOUNT, PACKAGE, 1, MAX),
        Status::Installed { revision: 1 }
    );
    // two ranges of RANGE_LEN, each with If-Range the strong tag
    assert_eq!(
        server.ranges(),
        [
            format!("bytes=0-{}", RANGE_LEN - 1),
            format!("bytes={RANGE_LEN}-{}", len - 1)
        ]
    );
    let tag = format!("\"{}\"", server.tag);
    assert!(server
        .asked
        .lock()
        .unwrap()
        .iter()
        .filter(|(path, _)| path.starts_with(&format!("{PACKAGE_PATH}?")))
        .all(|(_, headers)| crate::proxy::header(headers, "if-range") == Some(tag.as_str())));
    assert_eq!(installed(&packages), Some(1));
    let revision_dir = dir.join(format!("data/{ACCOUNT}.packages/{PACKAGE}/1"));
    assert_eq!(
        std::fs::read(revision_dir.join("basemap/LIGHT.pmtiles")).unwrap(),
        basemap(1)
    );
    // no part file is left, and the job says installed
    let leftovers: Vec<_> = std::fs::read_dir(revision_dir.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(leftovers, ["1"]);
    let status = packages.status(ACCOUNT);
    assert_eq!(status[0]["packageId"], PACKAGE);
    assert_eq!(status[0]["name"], "Lakes");
    assert_eq!(status[0]["download"]["state"], "installed");
    assert!(
        !status.to_string().contains(&dir.display().to_string()),
        "a path in {status}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir.join(format!("data/{ACCOUNT}.packages"))), 0o700);
        assert_eq!(mode(&revision_dir), 0o700);
        assert_eq!(mode(&revision_dir.join("basemap")), 0o700);
        assert_eq!(mode(&revision_dir.join("basemap/LIGHT.pmtiles")), 0o600);
        assert_eq!(mode(&revision_dir.join("manifest.json")), 0o600);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_range_is_answered_from_the_file() {
    let dir = dir("serve");
    let packages = packages(&dir);
    let server = Fake::new(package(1), 1);
    assert_eq!(
        download(&packages, &server, ACCOUNT, PACKAGE, 1, MAX),
        Status::Installed { revision: 1 }
    );
    let tiles = basemap(1);
    let etag = format!("\"sha256-{}\"", sha(&tiles));

    let served = serve(&packages, "basemap/LIGHT.pmtiles", Some("bytes=100-199"));
    assert_eq!(served.status, 206);
    assert_eq!(served.body, &tiles[100..200]);
    assert_eq!(
        header(&served, "content-range"),
        Some(format!("bytes 100-199/{}", tiles.len()).as_str())
    );
    assert_eq!(header(&served, "etag"), Some(etag.as_str()));
    assert_eq!(header(&served, "content-type"), Some("application/vnd.pmtiles"));
    // open-ended and suffix ranges; an If-Range of another tag takes the whole file
    let served = serve(&packages, "basemap/LIGHT.pmtiles", Some("bytes=-16"));
    assert_eq!(
        (served.status, served.body.as_slice()),
        (206, &tiles[tiles.len() - 16..])
    );
    let served = serve(
        &packages,
        "basemap/LIGHT.pmtiles",
        Some(&format!("bytes={}-", tiles.len() - 5)),
    );
    assert_eq!(served.body, &tiles[tiles.len() - 5..]);
    let whole = super::serve(
        &packages,
        ACCOUNT,
        "GET",
        &format!("{PATH_PREFIX}{PACKAGE}/style/light.json"),
        Some("bytes=0-3"),
        Some("\"sha256-other\""),
    );
    assert_eq!((whole.status, whole.body.as_slice()), (200, STYLE));
    // past the end, several ranges: 416
    for range in [format!("bytes={}-", tiles.len()), "bytes=0-1,5-6".to_string()] {
        let refused = serve(&packages, "basemap/LIGHT.pmtiles", Some(&range));
        assert_eq!(refused.status, 416, "{range}");
        assert_eq!(
            header(&refused, "content-range"),
            Some(format!("bytes */{}", tiles.len()).as_str())
        );
    }
    // HEAD: the headers and no body
    let head = super::serve(
        &packages,
        ACCOUNT,
        "HEAD",
        &format!("{PATH_PREFIX}{PACKAGE}/basemap/LIGHT.pmtiles"),
        None,
        None,
    );
    assert_eq!((head.status, head.body.len()), (200, 0));
    assert_eq!(header(&head, "content-length"), Some(tiles.len().to_string().as_str()));
    // the manifest is served; a path the manifest does not list is 404, a traversal too; no writes
    let manifest = serve(&packages, "manifest.json", None);
    assert_eq!(manifest.status, 200);
    assert!(String::from_utf8_lossy(&manifest.body).contains("\"packageId\":\"P1\""));
    for path in [
        "basemap/OTHER.pmtiles",
        "../1/manifest.json",
        "basemap/../manifest.json",
        "%2e%2e/x",
    ] {
        assert_eq!(serve(&packages, path, None).status, 404, "{path}");
    }
    assert_eq!(
        super::serve(
            &packages,
            "another-account",
            "GET",
            &format!("{PATH_PREFIX}{PACKAGE}/style/light.json"),
            None,
            None
        )
        .status,
        404
    );
    let post = super::serve(
        &packages,
        ACCOUNT,
        "POST",
        &format!("{PATH_PREFIX}{PACKAGE}/style/light.json"),
        None,
        None,
    );
    assert_eq!(post.status, 405);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_download_cut_off_resumes_from_its_part_file() {
    let dir = dir("resume");
    let packages = packages(&dir);
    let server = Fake::new(package(1), 1);
    server.next.lock().unwrap().push(Next::CutAfter(1000));
    let cut = download(&packages, &server, ACCOUNT, PACKAGE, 1, MAX);
    assert!(
        matches!(&cut, Status::Failed { code, retry: true, .. } if code == "OFFLINE" || code == "SERVER"),
        "{cut:?}"
    );
    let part = dir.join(format!("data/{ACCOUNT}.packages/{PACKAGE}/1.zip.part"));
    assert_eq!(std::fs::metadata(&part).unwrap().len(), 1000);
    assert_eq!(installed(&packages), None);
    // the next run asks from where the part ends, and the whole checks
    assert_eq!(
        download(&packages, &server, ACCOUNT, PACKAGE, 1, MAX),
        Status::Installed { revision: 1 }
    );
    assert_eq!(server.ranges()[1], format!("bytes=1000-{}", RANGE_LEN + 999));
    assert!(!part.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_checksum_mismatch_keeps_the_previous_revision() {
    let dir = dir("checksum");
    let packages = packages(&dir);
    let first = Fake::new(package(1), 1);
    assert_eq!(
        download(&packages, &first, ACCOUNT, PACKAGE, 1, MAX),
        Status::Installed { revision: 1 }
    );
    // revision 2, whose bytes are not the tag the server names
    let mut second = Fake::new(package(2), 2);
    second.tag = format!("sha256-{}", sha(b"another file"));
    let ended = download(&packages, &second, ACCOUNT, PACKAGE, 2, MAX);
    assert!(
        matches!(&ended, Status::Failed { code, retry: false, .. } if code == "CHECKSUM"),
        "{ended:?}"
    );
    assert_eq!(installed(&packages), Some(1));
    assert_eq!(
        serve(&packages, "basemap/LIGHT.pmtiles", Some("bytes=0-9")).body,
        &basemap(1)[..10]
    );
    assert!(!dir
        .join(format!("data/{ACCOUNT}.packages/{PACKAGE}/2.zip.part"))
        .exists());
    // a good revision 2 replaces revision 1, which goes
    let good = Fake::new(package(2), 2);
    assert_eq!(
        download(&packages, &good, ACCOUNT, PACKAGE, 2, MAX),
        Status::Installed { revision: 2 }
    );
    assert_eq!(installed(&packages), Some(2));
    assert!(!dir.join(format!("data/{ACCOUNT}.packages/{PACKAGE}/1")).exists());
    assert_eq!(
        serve(&packages, "basemap/LIGHT.pmtiles", Some("bytes=0-9")).body,
        &basemap(2)[..10]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_member_outside_the_manifest_or_not_as_it_says_is_refused() {
    let dir = dir("outside");
    let packages = packages(&dir);
    let tiles = basemap(1);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        // a member the manifest does not list
        (
            "extra",
            package_with(
                1,
                &[("style/light.json", STYLE)],
                vec![part("style/light.json", STYLE), part("evil.js", b"alert(1)")],
            ),
        ),
        // a member the manifest lists that is not there
        (
            "missing",
            package_with(
                1,
                &[("style/light.json", STYLE), ("basemap/LIGHT.pmtiles", &tiles)],
                vec![part("style/light.json", STYLE)],
            ),
        ),
        // a member whose bytes are not its checksum
        (
            "altered",
            package_with(
                1,
                &[("style/light.json", STYLE)],
                vec![part("style/light.json", b"{\"version\":9,\"sprite\":\"x\"}")],
            ),
        ),
        // a symlink
        (
            "symlink",
            package_with(
                1,
                &[("style/light.json", STYLE)],
                vec![Part {
                    name: "style/light.json".to_string(),
                    bytes: STYLE.to_vec(),
                    mode: 0o120_777,
                }],
            ),
        ),
    ];
    for (name, zip) in cases {
        let server = Fake::new(zip, 1);
        let ended = download(&packages, &server, ACCOUNT, PACKAGE, 1, MAX);
        assert!(
            matches!(&ended, Status::Failed { code, .. } if code == "BAD_PACKAGE"),
            "{name}: {ended:?}"
        );
        assert_eq!(installed(&packages), None, "{name}");
        let package_dir = dir.join(format!("data/{ACCOUNT}.packages/{PACKAGE}"));
        let left: Vec<_> = std::fs::read_dir(&package_dir)
            .map(|entries| entries.map(|entry| entry.unwrap().file_name()).collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "{name}: {left:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_traversal_path_is_refused_and_writes_nothing_outside() {
    let dir = dir("traversal");
    let packages = packages(&dir);
    for path in [
        "../escaped.txt",
        "/abs/escaped.txt",
        "style/../../escaped.txt",
        "a\\..\\escaped.txt",
        ".hidden",
    ] {
        // listed in the manifest and in the zip alike
        let zip = package_with(1, &[(path, b"x")], vec![part(path, b"x")]);
        let server = Fake::new(zip, 1);
        let ended = download(&packages, &server, ACCOUNT, PACKAGE, 1, MAX);
        assert!(
            matches!(&ended, Status::Failed { code, .. } if code == "BAD_PACKAGE"),
            "{path}: {ended:?}"
        );
        // in the zip only
        let zip = package_with(1, &[], vec![part(path, b"x")]);
        let ended = download(&packages, &Fake::new(zip, 1), ACCOUNT, PACKAGE, 1, MAX);
        assert!(
            matches!(&ended, Status::Failed { code, .. } if code == "BAD_PACKAGE"),
            "{path}: {ended:?}"
        );
    }
    assert_eq!(installed(&packages), None);
    let found: Vec<PathBuf> = walk(&dir)
        .into_iter()
        .filter(|path| path.ends_with("escaped.txt"))
        .collect();
    assert!(found.is_empty(), "{found:?}");
    assert!(!dir.join("escaped.txt").exists() && !Path::new("/abs/escaped.txt").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        }
        found.push(path);
    }
    found
}

#[test]
fn a_shared_devices_wipe_takes_the_accounts_packages_and_its_downloads() {
    let dir = dir("wipe");
    let packages = packages(&dir);
    let server = Fake::new(package(1), 1);
    assert_eq!(
        download(&packages, &server, ACCOUNT, PACKAGE, 1, MAX),
        Status::Installed { revision: 1 }
    );
    let other = "other.example.com-0123456789ab";
    assert_eq!(
        download(&packages, &server, other, PACKAGE, 1, MAX),
        Status::Installed { revision: 1 }
    );
    // one more download asked and not run yet, and a part file of one cut off
    packages.queue(ACCOUNT, "P2", 1).unwrap();
    let part_dir = dir.join(format!("data/{ACCOUNT}.packages/P3"));
    std::fs::create_dir_all(&part_dir).unwrap();
    std::fs::write(part_dir.join("1.zip.part"), b"PK").unwrap();
    packages.forget(ACCOUNT);
    assert!(!dir.join(format!("data/{ACCOUNT}.packages")).exists());
    assert_eq!(packages.status(ACCOUNT), serde_json::json!([]));
    assert_eq!(serve(&packages, "style/light.json", None).status, 404);
    // the other account's stay
    assert_eq!(
        packages.installed_dir(other, PACKAGE).map(|(revision, _)| revision),
        Some(1)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_package_past_the_cap_or_the_free_space_is_refused_before_it_starts() {
    let dir = dir("limits");
    let packages = packages(&dir);
    let server = Fake::new(package(1), 1);
    let len = server.zip.len() as u64;
    let ended = download(&packages, &server, ACCOUNT, PACKAGE, 1, len - 1);
    assert!(
        matches!(&ended, Status::Failed { code, retry: false, .. } if code == "TOO_LARGE"),
        "{ended:?}"
    );
    assert!(server.ranges().is_empty());
    // the cap counts the account's other packages, not the one replaced
    let other = Fake::new(package(1), 1);
    assert_eq!(
        download(&packages, &other, ACCOUNT, PACKAGE, 1, MAX),
        Status::Installed { revision: 1 }
    );
    assert!(packages.used(ACCOUNT, "P2") > RANGE_LEN);
    assert_eq!(packages.used(ACCOUNT, PACKAGE), 0);
    // a device with too little room: the part and the unpacked copy need twice the file and a margin
    let small = Packages::with_free_space(&dir, Box::new(move |_| Some(len)));
    let _ = std::fs::remove_dir_all(dir.join("data"));
    let tight = Fake::new(package(1), 1);
    let ended = download(&small, &tight, ACCOUNT, PACKAGE, 1, MAX);
    assert!(
        matches!(&ended, Status::Failed { code, retry: false, .. } if code == "NO_SPACE"),
        "{ended:?}"
    );
    assert!(tight.ranges().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stale_revision_answered_409_offers_the_new_one() {
    let dir = dir("stale");
    let packages = packages(&dir);
    let server = Fake::new(package(2), 2);
    // the list names revision 2, then the server builds it again while the download asks
    server.next.lock().unwrap().push(Next::Status(409));
    *server.listed_revision.lock().unwrap() = 2;
    let first = {
        // the first range is 409; the list read again names revision 3
        let ended = {
            let fake = &server;
            struct Rebuilt<'a>(&'a Fake);
            impl PackageServer for Rebuilt<'_> {
                fn get(&self, path: &str, headers: &[(String, String)]) -> Result<ServerAnswer, String> {
                    let answer = self.0.get(path, headers)?;
                    if answer.status == 409 {
                        *self.0.listed_revision.lock().unwrap() = 3;
                    }
                    Ok(answer)
                }
            }
            download(&packages, &Rebuilt(fake), ACCOUNT, PACKAGE, 2, MAX)
        };
        ended
    };
    assert_eq!(first, Status::Stale { revision: Some(3) });
    assert_eq!(
        packages.job_status(ACCOUNT, PACKAGE),
        Some(Status::Stale { revision: Some(3) })
    );
    assert!(!dir
        .join(format!("data/{ACCOUNT}.packages/{PACKAGE}/2.zip.part"))
        .exists());
    // a download of a revision the list no longer names is stale before it asks for a byte
    let asked = server.ranges().len();
    assert_eq!(
        download(&packages, &server, ACCOUNT, PACKAGE, 2, MAX),
        Status::Stale { revision: Some(3) }
    );
    assert_eq!(server.ranges().len(), asked);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cancel_stops_a_download_and_keeps_nothing_of_it() {
    let dir = dir("cancel");
    let packages = packages(&dir);
    assert_eq!(
        packages.queue(ACCOUNT, PACKAGE, 1).unwrap(),
        Status::Queued { revision: 1 }
    );
    assert!(packages.cancel(ACCOUNT, PACKAGE));
    assert_eq!(
        packages.job_status(ACCOUNT, PACKAGE),
        Some(Status::Cancelled { revision: 1 })
    );
    // cancelled between ranges: the server's first range comes, the cancel is seen before the next
    struct CancelAfterFirst<'a>(&'a Fake, &'a Packages);
    impl PackageServer for CancelAfterFirst<'_> {
        fn get(&self, path: &str, headers: &[(String, String)]) -> Result<ServerAnswer, String> {
            let answer = self.0.get(path, headers)?;
            if answer.status == 206 {
                self.1.cancel(ACCOUNT, PACKAGE);
            }
            Ok(answer)
        }
    }
    let server = Fake::new(package(1), 1);
    let ended = download(
        &packages,
        &CancelAfterFirst(&server, &packages),
        ACCOUNT,
        PACKAGE,
        1,
        MAX,
    );
    assert_eq!(ended, Status::Cancelled { revision: 1 });
    assert_eq!(server.ranges().len(), 1);
    assert!(!dir.join(format!("data/{ACCOUNT}.packages/{PACKAGE}")).exists());
    // delete removes an installed one
    assert_eq!(
        download(&packages, &server, ACCOUNT, PACKAGE, 1, MAX),
        Status::Installed { revision: 1 }
    );
    packages.delete(ACCOUNT, PACKAGE).unwrap();
    assert_eq!(installed(&packages), None);
    assert!(packages.delete(ACCOUNT, "../x").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
