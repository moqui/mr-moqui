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

fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn a_window_s_attachment_or_large_file_is_saved_by_the_host() {
    let attachment = headers(&[("Content-Disposition", "attachment; filename=\"Orders.csv\"")]);
    assert_eq!(
        saved_by_host(true, 200, &attachment, "/qapps/x/export").as_deref(),
        Some("Orders.csv")
    );
    assert_eq!(
        saved_by_host(false, 200, &attachment, "/qapps/x/export"),
        None,
        "a page's fetch takes it"
    );
    assert_eq!(saved_by_host(true, 404, &attachment, "/qapps/x/export"), None);
    let inline = headers(&[
        ("Content-Disposition", "inline; filename=\"a.pdf\""),
        ("Content-Length", "100"),
    ]);
    assert_eq!(saved_by_host(true, 200, &inline, "/a.pdf"), None);

    let large = (STREAM_OVER + 1).to_string();
    let video = headers(&[("Content-Type", "video/mp4"), ("Content-Length", &large)]);
    assert_eq!(
        saved_by_host(true, 200, &video, "/content/Film%201.mp4?x=1").as_deref(),
        Some("Film%201.mp4")
    );
    let page = headers(&[("Content-Type", "text/html"), ("Content-Length", &large)]);
    assert_eq!(
        saved_by_host(true, 200, &page, "/qapps/big"),
        None,
        "a large page is still a page"
    );
    let small = headers(&[("Content-Type", "video/mp4"), ("Content-Length", "1000")]);
    assert_eq!(saved_by_host(true, 200, &small, "/a.mp4"), None);
}

#[test]
fn a_download_is_renamed_when_whole_and_removed_when_not() {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-download-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("report.csv");
    assert_eq!(part_path(&path), dir.join("report.csv.part"));

    let written = write_to(&path, |out| {
        out.write_all(b"a,b\n1,2\n").map_err(|err| err.to_string())?;
        Ok(8)
    });
    assert_eq!(written, Ok(8));
    assert_eq!(std::fs::read(&path).unwrap(), b"a,b\n1,2\n");
    assert!(!part_path(&path).exists());

    let cut = dir.join("cut.bin");
    let failed = write_to(&cut, |out| {
        out.write_all(b"half").map_err(|err| err.to_string())?;
        Err("the server's answer was cut off".to_string())
    });
    assert!(failed.is_err());
    assert!(!cut.exists() && !part_path(&cut).exists());
    let _ = std::fs::remove_dir_all(&dir);
}
