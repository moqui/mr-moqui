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
    /// A whole answer (200, the range ignored) of this many bytes of the file.
    WholeShort(usize),
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
        if let Next::WholeShort(bytes) = next {
            let etag = format!("\"{}\"", self.tag);
            return Ok(answer(200, vec![("ETag", etag)], self.zip[..bytes].to_vec()));
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
    // a pause (the host stopped the run) ends it at its next range with the part kept
    struct PauseAfterFirst<'a>(&'a Fake, &'a Packages);
    impl PackageServer for PauseAfterFirst<'_> {
        fn get(&self, path: &str, headers: &[(String, String)]) -> Result<ServerAnswer, String> {
            let answer = self.0.get(path, headers)?;
            if answer.status == 206 {
                self.1.pause(ACCOUNT, PACKAGE, None);
            }
            Ok(answer)
        }
    }
    let paused = download(
        &packages,
        &PauseAfterFirst(&server, &packages),
        ACCOUNT,
        PACKAGE,
        1,
        MAX,
    );
    assert!(
        matches!(&paused, Status::Failed { code, retry: true, .. } if code == "PAUSED"),
        "{paused:?}"
    );
    assert_eq!(std::fs::metadata(&part).unwrap().len(), RANGE_LEN + 1000);
    // the next run asks from where the part ends, and the whole checks
    assert_eq!(
        download(&packages, &server, ACCOUNT, PACKAGE, 1, MAX),
        Status::Installed { revision: 1 }
    );
    assert_eq!(server.ranges()[1], format!("bytes=1000-{}", RANGE_LEN + 999));
    assert_eq!(
        server.ranges()[2],
        format!("bytes={}-{}", RANGE_LEN + 1000, server.zip.len() - 1)
    );
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
    // revision 2, whose bytes are not the tag the server names; asked for as the host asks
    packages.queue(ACCOUNT, PACKAGE, 2).unwrap();
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
    assert!(packages.queue(ACCOUNT, "P2", 1).unwrap().1);
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
        (Status::Queued { revision: 1 }, true)
    );
    // asked again before it ran: started again (its worker replaced, its Wi-Fi choice with it); a revision under 1
    // refused
    assert_eq!(
        packages.queue(ACCOUNT, PACKAGE, 1).unwrap(),
        (Status::Queued { revision: 1 }, true)
    );
    assert!(packages.queue(ACCOUNT, PACKAGE, 0).is_err());
    assert!(packages.cancel(ACCOUNT, PACKAGE));
    assert_eq!(
        packages.job_status(ACCOUNT, PACKAGE),
        Some(Status::Cancelled { revision: 1 })
    );
    // the cancel stands: a run of it (a worker that comes late) does nothing and ends, done
    let late = Fake::new(package(1), 1);
    assert_eq!(
        download(&packages, &late, ACCOUNT, PACKAGE, 1, MAX),
        Status::Cancelled { revision: 1 }
    );
    assert!(late.asked.lock().unwrap().is_empty());
    // a download asked again runs
    assert!(packages.queue(ACCOUNT, PACKAGE, 1).unwrap().1);
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
    packages.queue(ACCOUNT, PACKAGE, 1).unwrap();
    assert_eq!(
        download(&packages, &server, ACCOUNT, PACKAGE, 1, MAX),
        Status::Installed { revision: 1 }
    );
    packages.delete(ACCOUNT, PACKAGE).unwrap();
    assert_eq!(installed(&packages), None);
    assert!(packages.delete(ACCOUNT, "../x").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The review of OF2: a cancel with no run under way (a failed or paused download, or one from before the app started:
/// no job) removes its part file at once, and the folders it leaves empty, named after the server, go too.
#[test]
fn a_cancel_with_no_run_removes_the_part_now() {
    let dir = dir("cancel-part");
    let packages = packages(&dir);
    let account_dir = dir.join(format!("data/{ACCOUNT}.packages"));
    let part = account_dir.join(format!("{PACKAGE}/1.zip.part"));
    // a part kept from before the app started: no job
    std::fs::create_dir_all(part.parent().unwrap()).unwrap();
    std::fs::write(&part, b"PK").unwrap();
    std::fs::write(part.with_extension("tag"), b"sha256-x").unwrap();
    assert!(packages.cancel(ACCOUNT, PACKAGE));
    assert!(!part.exists());
    assert!(!account_dir.exists(), "an empty account folder is left");
    // a paused download's part
    let server = Fake::new(package(1), 1);
    server.next.lock().unwrap().push(Next::CutAfter(1000));
    let cut = download(&packages, &server, ACCOUNT, PACKAGE, 1, MAX);
    assert!(matches!(&cut, Status::Failed { retry: true, .. }), "{cut:?}");
    assert!(part.exists());
    assert!(packages.cancel(ACCOUNT, PACKAGE));
    assert!(!part.exists() && !account_dir.exists());
    assert_eq!(
        packages.job_status(ACCOUNT, PACKAGE),
        Some(Status::Cancelled { revision: 1 })
    );
    // nothing to cancel
    assert!(!packages.cancel(ACCOUNT, PACKAGE));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The review of OF2: a wipe while a download runs (a shared device's sign-out) leaves nothing of it, no empty folder
/// named after the server either, and the run ends cancelled.
#[test]
fn a_wipe_during_a_download_leaves_no_folder() {
    let dir = dir("wipe-run");
    let packages = packages(&dir);
    struct WipeOnLast<'a>(&'a Fake, &'a Packages);
    impl PackageServer for WipeOnLast<'_> {
        fn get(&self, path: &str, headers: &[(String, String)]) -> Result<ServerAnswer, String> {
            let last = crate::proxy::header(headers, "range")
                .is_some_and(|range| range.ends_with(&format!("-{}", self.0.zip.len() - 1)));
            let answer = self.0.get(path, headers)?;
            if last {
                self.1.forget(ACCOUNT);
            }
            Ok(answer)
        }
    }
    let server = Fake::new(package(1), 1);
    let ended = download(&packages, &WipeOnLast(&server, &packages), ACCOUNT, PACKAGE, 1, MAX);
    assert_eq!(ended, Status::Cancelled { revision: 1 });
    assert!(!dir.join(format!("data/{ACCOUNT}.packages")).exists());
    assert_eq!(installed(&packages), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The review of OF2: a run that holds a package (here cancelled and still stopping) is never joined by a second:
/// another download of it is refused until the first has stopped, and a second run asked meanwhile does not start.
#[test]
fn a_package_being_cancelled_is_not_downloaded_twice_at_once() {
    let dir = dir("twice");
    let packages = packages(&dir);
    struct Meanwhile<'a>(&'a Fake, &'a Packages, Mutex<Option<(String, Status)>>);
    impl PackageServer for Meanwhile<'_> {
        fn get(&self, path: &str, headers: &[(String, String)]) -> Result<ServerAnswer, String> {
            let answer = self.0.get(path, headers)?;
            if answer.status == 206 && self.2.lock().unwrap().is_none() {
                self.1.cancel(ACCOUNT, PACKAGE);
                let queued = self.1.queue(ACCOUNT, PACKAGE, 1).err().unwrap_or_default();
                let second = download(self.1, self.0, ACCOUNT, PACKAGE, 1, MAX);
                *self.2.lock().unwrap() = Some((queued, second));
            }
            Ok(answer)
        }
    }
    let server = Fake::new(package(1), 1);
    let meanwhile = Meanwhile(&server, &packages, Mutex::new(None));
    assert_eq!(
        download(&packages, &meanwhile, ACCOUNT, PACKAGE, 1, MAX),
        Status::Cancelled { revision: 1 }
    );
    let (queued, second) = meanwhile.2.lock().unwrap().clone().unwrap();
    assert!(queued.contains("being stopped"), "{queued}");
    assert!(!second.ended(), "{second:?}");
    assert_eq!(server.ranges().len(), 1, "one run asked for bytes");
    // once stopped, it may be asked again
    assert!(packages.queue(ACCOUNT, PACKAGE, 1).unwrap().1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The review of OF2: two downloads of one account under way together cannot pass its cap: each counts what the
/// others have reserved and not yet downloaded.
#[test]
fn two_downloads_of_an_account_together_keep_within_its_cap() {
    let dir = dir("cap-together");
    let packages = packages(&dir);
    let first = packages.take(ACCOUNT, "P1", 1, None).unwrap();
    let second = packages.take(ACCOUNT, "P2", 1, None).unwrap();
    assert!(packages.reserve(ACCOUNT, "P1", &first, 600, 1000));
    assert!(!packages.reserve(ACCOUNT, "P2", &second, 600, 1000));
    assert!(packages.reserve(ACCOUNT, "P2", &second, 400, 1000));
    // another account's do not count
    let other = packages.take("other.example.com-1", "P1", 1, None).unwrap();
    assert!(packages.reserve("other.example.com-1", "P1", &other, 1000, 1000));
    // a run that ended holds nothing
    packages.release(&first);
    let third = packages.take(ACCOUNT, "P3", 1, None).unwrap();
    assert!(packages.reserve(ACCOUNT, "P3", &third, 600, 1000));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The review of OF2: a pause by a runner whose run ended does not pause the next run.
#[test]
fn a_pause_is_the_runners_own() {
    let dir = dir("runner");
    let packages = packages(&dir);
    let job = packages.take(ACCOUNT, PACKAGE, 1, Some("work-2")).unwrap();
    packages.pause(ACCOUNT, PACKAGE, Some("work-1"));
    assert!(!job.paused());
    packages.pause(ACCOUNT, PACKAGE, Some("work-2"));
    assert!(job.paused());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The review of OF2: a whole answer cut short is a server failure to try again, the part kept; the host's giving up
/// makes the failure final.
#[test]
fn a_whole_answer_cut_short_is_tried_again_and_a_give_up_is_final() {
    let dir = dir("whole-short");
    let packages = packages(&dir);
    let server = Fake::new(package(1), 1);
    server.next.lock().unwrap().push(Next::WholeShort(5000));
    let ended = download_by(&packages, &server, ACCOUNT, PACKAGE, 1, MAX, Some("work-1"));
    assert!(
        matches!(&ended, Status::Failed { code, retry: true, .. } if code == "SERVER"),
        "{ended:?}"
    );
    let given = packages.give_up(ACCOUNT, PACKAGE, 1, "work-1", "it failed").unwrap();
    assert!(matches!(&given, Status::Failed { retry: false, .. }), "{given:?}");
    // a revision under 1 is not downloaded
    assert!(matches!(
        download(&packages, &server, ACCOUNT, PACKAGE, 0, MAX),
        Status::Failed { retry: false, .. }
    ));
    assert_eq!(
        download(&packages, &server, ACCOUNT, PACKAGE, 1, MAX),
        Status::Installed { revision: 1 }
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The review of OF2: a tag's temporary file a crash left goes with the part files.
#[test]
fn a_tags_temporary_file_goes_with_the_parts() {
    let dir = dir("tmp");
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["1.zip.part", "1.zip.tag", ".1.zip.tag.4242.tmp", "1"] {
        std::fs::write(dir.join(name), b"x").unwrap();
    }
    assert_eq!(download::remove_parts(&dir, None), 3);
    let left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(left, ["1"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The review of OF2: no member or account file may take a name Windows keeps for a device, whatever its extension.
#[test]
fn a_windows_device_name_is_no_member_path() {
    for path in [
        "CON",
        "nul.txt",
        "basemap/aux.pmtiles",
        "Com1.tar.gz",
        "fonts/LPT9.css",
        "prn",
    ] {
        assert!(!unzip::plain_path(path), "{path}");
    }
    for path in [
        "console.json",
        "com10.json",
        "basemap/LIGHT.pmtiles",
        "fonts/noto-sans/files/a@2x.png",
        "nullable.json",
    ] {
        assert!(unzip::plain_path(path), "{path}");
    }
    // a package id is refused one too; an account id is a plain name everywhere (what an earlier version wrote is
    // found and forgotten), and a name a new file may take but on Windows
    assert!(Packages::new(Path::new("/x")).package_dir(ACCOUNT, "NUL").is_none());
    assert!(crate::files::plain_name("con.example.com-0123456789ab"));
    assert_eq!(
        crate::files::writable_name("con.example.com-0123456789ab"),
        !cfg!(windows)
    );
    assert!(crate::files::writable_name("console.example.com-0123456789ab"));
}

/// The second review of OF2: an account of `aux.corp.net` is an account on Linux, macOS and Android: its store opens,
/// its packages download, and it is forgotten whole; a forget never refuses a name.
#[test]
fn an_account_named_like_a_windows_device_opens_and_is_forgotten() {
    let dir = dir("aux-account");
    let account = "aux.corp.net-0123456789ab";
    let packages = packages(&dir);
    let server = Fake::new(package(1), 1);
    if !cfg!(windows) {
        let store = crate::store::Store::open(&dir, account, 1 << 20).unwrap();
        drop(store);
        assert!(dir.join(format!("data/{account}.db")).exists());
        assert_eq!(
            download(&packages, &server, account, PACKAGE, 1, MAX),
            Status::Installed { revision: 1 }
        );
    } else {
        std::fs::create_dir_all(dir.join(format!("data/{account}.packages/{PACKAGE}/1"))).unwrap();
    }
    packages.forget(account);
    crate::store::forget(&dir, account).unwrap();
    assert!(!dir.join(format!("data/{account}.packages")).exists());
    assert!(!dir.join(format!("data/{account}.db")).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The second review of OF2: a package deleted (or its account forgotten) while its cancelled run still goes keeps a
/// tombstone until that run ends: no download of it is queued or run on the same files meanwhile.
#[test]
fn a_package_deleted_while_its_run_goes_is_not_downloaded_again_meanwhile() {
    for forget in [false, true] {
        let dir = dir(&format!("tombstone-{forget}"));
        let packages = packages(&dir);
        struct DeleteOnFirst<'a>(&'a Fake, &'a Packages, bool, Mutex<Option<(bool, bool, Status)>>);
        impl PackageServer for DeleteOnFirst<'_> {
            fn get(&self, path: &str, headers: &[(String, String)]) -> Result<ServerAnswer, String> {
                let answer = self.0.get(path, headers)?;
                if answer.status == 206 && self.3.lock().unwrap().is_none() {
                    if self.2 {
                        self.1.forget(ACCOUNT);
                    } else {
                        self.1.delete(ACCOUNT, PACKAGE).unwrap();
                    }
                    let refused = self.1.queue(ACCOUNT, PACKAGE, 1).is_err();
                    let listed = self.1.status(ACCOUNT).as_array().is_some_and(|list| list.is_empty());
                    let second = download(self.1, self.0, ACCOUNT, PACKAGE, 1, MAX);
                    *self.3.lock().unwrap() = Some((refused, listed, second));
                }
                Ok(answer)
            }
        }
        let server = Fake::new(package(1), 1);
        let deleting = DeleteOnFirst(&server, &packages, forget, Mutex::new(None));
        assert_eq!(
            download(&packages, &deleting, ACCOUNT, PACKAGE, 1, MAX),
            Status::Cancelled { revision: 1 }
        );
        let (refused, listed, second) = deleting.3.lock().unwrap().clone().unwrap();
        assert!(refused, "a download queued while the deleted run went");
        assert!(listed, "the tombstone is not the shell's");
        assert!(!second.ended(), "{second:?}");
        assert_eq!(server.ranges().len(), 1, "one run asked for bytes");
        assert!(!dir.join(format!("data/{ACCOUNT}.packages")).exists());
        // once the run ended, its tombstone is gone and a download may be asked again
        assert_eq!(packages.job_status(ACCOUNT, PACKAGE), None);
        assert!(packages.queue(ACCOUNT, PACKAGE, 1).unwrap().1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A zip64 package: the end record's fields at their markers and the members' sizes and offset in their zip64 extra
/// field, as a package past 4 GiB is written; read for its entries as a plain one is.
#[test]
fn a_zip64_package_is_read_by_its_zip64_records() {
    let dir = dir("zip64");
    std::fs::create_dir_all(&dir).unwrap();
    let data = b"the tiles";
    let name = b"basemap/LIGHT.pmtiles";
    let mut out = Vec::new();
    // the local header: zip64 sizes in its extra field
    out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    out.extend_from_slice(&45u16.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    out.extend_from_slice(data);
    // the central header: sizes and offset at their markers, in its zip64 extra field
    let central_at = out.len() as u64;
    let mut central = Vec::new();
    central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
    central.extend_from_slice(&((3u16 << 8) | 45).to_le_bytes());
    central.extend_from_slice(&45u16.to_le_bytes());
    central.extend_from_slice(&[0; 8]);
    central.extend_from_slice(&0u32.to_le_bytes());
    central.extend_from_slice(&u32::MAX.to_le_bytes());
    central.extend_from_slice(&u32::MAX.to_le_bytes());
    central.extend_from_slice(&(name.len() as u16).to_le_bytes());
    central.extend_from_slice(&28u16.to_le_bytes());
    central.extend_from_slice(&[0; 6]);
    central.extend_from_slice(&(0o100_644u32 << 16).to_le_bytes());
    central.extend_from_slice(&u32::MAX.to_le_bytes());
    central.extend_from_slice(name);
    central.extend_from_slice(&1u16.to_le_bytes());
    central.extend_from_slice(&24u16.to_le_bytes());
    central.extend_from_slice(&(data.len() as u64).to_le_bytes());
    central.extend_from_slice(&(data.len() as u64).to_le_bytes());
    central.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&central);
    // the zip64 end record and its locator, then the end record with its fields at their markers
    let zip64_at = out.len() as u64;
    out.extend_from_slice(&0x0606_4b50u32.to_le_bytes());
    out.extend_from_slice(&44u64.to_le_bytes());
    out.extend_from_slice(&45u16.to_le_bytes());
    out.extend_from_slice(&45u16.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&1u64.to_le_bytes());
    out.extend_from_slice(&1u64.to_le_bytes());
    out.extend_from_slice(&(central.len() as u64).to_le_bytes());
    out.extend_from_slice(&central_at.to_le_bytes());
    out.extend_from_slice(&0x0706_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&zip64_at.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    let path = dir.join("p.zip");
    std::fs::write(&path, &out).unwrap();
    let mut file = std::fs::File::open(&path).unwrap();
    let entries = unzip::entries(&mut file, out.len() as u64).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (entries[0].path.as_str(), entries[0].size),
        ("basemap/LIGHT.pmtiles", data.len() as u64)
    );
    let mut copied = Vec::new();
    unzip::copy_entry(&mut file, &entries[0], &mut copied, &|| false).unwrap();
    assert_eq!(copied, data);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The third review of OF2: a revision left only aside (`.N.old`) by a run whose renames both failed is put back
/// before the next swap, not removed; a swap that fails keeps what was there.
#[test]
fn a_revision_left_aside_is_put_back_not_removed() {
    let dir = dir("aside");
    let (target, aside, unpacked) = (dir.join("1"), dir.join(".1.old"), dir.join(".1.unpack"));
    std::fs::create_dir_all(&aside).unwrap();
    std::fs::write(aside.join("manifest.json"), b"old").unwrap();
    // nothing unpacked to put in place: the rename fails, and the copy aside is back as the revision
    assert!(download::put_in_place(&unpacked, &target, &aside).is_err());
    assert_eq!(std::fs::read(target.join("manifest.json")).unwrap(), b"old");
    assert!(!aside.exists());
    // a new one in place: the old one goes
    std::fs::create_dir_all(&unpacked).unwrap();
    std::fs::write(unpacked.join("manifest.json"), b"new").unwrap();
    download::put_in_place(&unpacked, &target, &aside).unwrap();
    assert_eq!(std::fs::read(target.join("manifest.json")).unwrap(), b"new");
    assert!(!aside.exists() && !unpacked.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The third review of OF2: a job installed already gives no run of its own revision (a worker replaced before it
/// took its job), so the package is not downloaded a second time; a download asked again is.
#[test]
fn an_installed_job_is_not_downloaded_again_by_a_late_worker() {
    let dir = dir("installed-late");
    let packages = packages(&dir);
    let server = Fake::new(package(1), 1);
    assert_eq!(
        download_by(&packages, &server, ACCOUNT, PACKAGE, 1, MAX, Some("work-1")),
        Status::Installed { revision: 1 }
    );
    let asked = server.asked.lock().unwrap().len();
    assert!(packages.take(ACCOUNT, PACKAGE, 1, Some("work-2")).is_none());
    assert_eq!(
        download_by(&packages, &server, ACCOUNT, PACKAGE, 1, MAX, Some("work-2")),
        Status::Installed { revision: 1 }
    );
    assert_eq!(server.asked.lock().unwrap().len(), asked);
    assert!(packages.queue(ACCOUNT, PACKAGE, 1).unwrap().1);
    assert!(packages.take(ACCOUNT, PACKAGE, 1, Some("work-3")).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The third review of OF2: only the runner that last took or tried a job, of its revision, gives it up: a stale run
/// leaves a job a new tap queued as it is; a run that never took it (no network) gives it up with its own reason.
#[test]
fn a_give_up_is_the_last_runners_of_its_revision() {
    let dir = dir("give-up-runner");
    let packages = packages(&dir);
    let server = Fake::new(package(1), 1);
    server.next.lock().unwrap().push(Next::CutAfter(1000));
    let cut = download_by(&packages, &server, ACCOUNT, PACKAGE, 1, MAX, Some("work-1"));
    assert!(matches!(&cut, Status::Failed { retry: true, .. }), "{cut:?}");
    // another runner, another revision: nothing changes
    for (revision, runner) in [(1, "work-2"), (2, "work-1")] {
        let kept = packages.give_up(ACCOUNT, PACKAGE, revision, runner, "x").unwrap();
        assert!(
            matches!(&kept, Status::Failed { retry: true, .. }),
            "{revision} {runner}: {kept:?}"
        );
    }
    // a new tap queues a new job: the stale run cannot end it
    assert!(packages.queue(ACCOUNT, PACKAGE, 1).unwrap().1);
    assert_eq!(
        packages.give_up(ACCOUNT, PACKAGE, 1, "work-1", "x"),
        Some(Status::Queued { revision: 1 })
    );
    // its own worker, which tried with no network, gives it up with what stopped it
    packages.tried_by(ACCOUNT, PACKAGE, 1, "work-3");
    let given = packages
        .give_up(ACCOUNT, PACKAGE, 1, "work-3", "there was no network")
        .unwrap();
    assert!(
        matches!(&given, Status::Failed { code, reason, retry: false, .. }
            if code == "GAVE_UP" && reason.contains("no network")),
        "{given:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
