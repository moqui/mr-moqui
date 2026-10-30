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

fn body(parts: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (headers, data) in parts {
        out.extend_from_slice(b"--XyZ\r\n");
        out.extend_from_slice(headers.as_bytes());
        out.extend_from_slice(b"\r\n\r\n");
        out.extend_from_slice(data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"--XyZ--\r\n");
    out
}

const TYPE: &str = "multipart/form-data; boundary=XyZ";

#[test]
fn fields_and_files_are_separated_and_keep_their_bytes() {
    let data = body(&[
        (
            "Content-Disposition: form-data; name=\"title\"",
            "Hello \u{e9}".as_bytes(),
        ),
        (
            "Content-Disposition: form-data; name=\"doc\"; filename=\"C:\\tmp\\a b.bin\"\r\nContent-Type: image/png",
            &[0, 1, 2, 13, 10, 45, 45, 255],
        ),
        ("Content-Disposition: form-data; name=\"empty\"", b""),
        ("Content-Disposition: form-data; name=\"title\"", b"again"),
    ]);
    let parsed = parse(TYPE, &data).unwrap();
    assert_eq!(
        parsed.fields,
        vec![
            ("title".to_string(), "Hello \u{e9}".to_string()),
            ("empty".to_string(), String::new()),
            ("title".to_string(), "again".to_string())
        ]
    );
    assert_eq!(parsed.files.len(), 1);
    let file = &parsed.files[0];
    assert_eq!(file.field_name, "doc");
    assert_eq!(file.file_name, "a b.bin");
    assert_eq!(file.content_type, "image/png");
    assert_eq!(file.read_bytes().unwrap(), vec![0, 1, 2, 13, 10, 45, 45, 255]);
}

#[test]
fn an_empty_file_input_posts_nothing_and_quotes_in_names_are_unescaped() {
    let data = body(&[
        (
            "Content-Disposition: form-data; name=\"doc\"; filename=\"\"\r\nContent-Type: application/octet-stream",
            b"",
        ),
        (
            "Content-Disposition: form-data; name=\"a\\\"b\"; filename=\"x\\\\y.txt\"",
            b"t",
        ),
    ]);
    let parsed = parse("multipart/form-data; boundary=\"XyZ\"", &data).unwrap();
    assert!(parsed.fields.is_empty());
    assert_eq!(parsed.files.len(), 1);
    assert_eq!(parsed.files[0].field_name, "a\"b");
    assert_eq!(parsed.files[0].file_name, "y.txt");
    assert_eq!(parsed.files[0].content_type, "application/octet-stream");
}

#[test]
fn a_part_that_looks_like_a_boundary_inside_data_does_not_split() {
    let data = body(&[(
        "Content-Disposition: form-data; name=\"t\"",
        b"line\r\n--Xy not a boundary\r\nend",
    )]);
    let parsed = parse(TYPE, &data).unwrap();
    assert_eq!(parsed.fields[0].1, "line\r\n--Xy not a boundary\r\nend");
}

#[test]
fn bad_bodies_are_errors_not_panics() {
    assert!(parse("multipart/form-data", b"--x\r\n").is_err());
    assert!(parse(TYPE, b"").is_err());
    assert!(parse(
        TYPE,
        b"--XyZ\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nno end"
    )
    .is_err());
    assert!(parse(TYPE, b"--XyZ\r\nContent-Type: text/plain\r\n\r\ndata\r\n--XyZ--").is_err());
    assert!(parse(TYPE, b"--XyZ junk\r\n").is_err());
    assert!(parse(TYPE, b"--XyZ\r\nContent-Disposition: form-data; name=\"a\"").is_err());
    // The closing marker alone is an empty form.
    assert!(parse(TYPE, b"--XyZ--\r\n").unwrap().fields.is_empty());
}

/// The same body fed in pieces of `size` bytes.
fn fed(data: &[u8], size: usize, spool: Option<Spool>) -> Result<Parsed, MultipartError> {
    let mut reader = MultipartReader::new(TYPE, spool)?;
    for piece in data.chunks(size.max(1)) {
        reader.feed(piece)?;
    }
    reader.finish()
}

#[test]
fn a_body_fed_in_pieces_reads_as_one_in_memory() {
    let file: Vec<u8> = (0..5000u32).map(|n| (n % 251) as u8).collect();
    let data = body(&[
        ("Content-Disposition: form-data; name=\"a\"", b"first"),
        ("Content-Disposition: form-data; name=\"f\"; filename=\"f.bin\"", &file),
        ("Content-Disposition: form-data; name=\"empty\"", b""),
        (
            "Content-Disposition: form-data; name=\"t\"",
            b"line\r\n--Xy not a boundary\r\nx--XyZ- no\r\nend",
        ),
    ]);
    let whole = parse(TYPE, &data).unwrap();
    for size in [1, 2, 3, 5, 7, 13, 64, 1000, data.len()] {
        let parsed = fed(&data, size, None).unwrap();
        assert_eq!(parsed.fields, whole.fields, "pieces of {size}");
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].read_bytes().unwrap(), file, "pieces of {size}");
    }
    assert_eq!(whole.fields[2].1, "line\r\n--Xy not a boundary\r\nx--XyZ- no\r\nend");
}

#[test]
fn large_files_are_spooled_with_their_hash_and_removed_when_dropped() {
    let dir = std::env::temp_dir().join(format!("moqui-spool-{}", std::process::id()));
    let spool = Spool {
        dir: dir.clone(),
        memory_limit: 1000,
        field_limit: 100,
    };
    let large: Vec<u8> = (0..50_000u32).map(|n| (n * 31 % 253) as u8).collect();
    let data = body(&[
        (
            "Content-Disposition: form-data; name=\"small\"; filename=\"s.txt\"",
            b"small file",
        ),
        (
            "Content-Disposition: form-data; name=\"large\"; filename=\"l.bin\"",
            &large,
        ),
    ]);
    let parsed = fed(&data, 777, Some(spool.clone())).unwrap();
    let (small, big) = (&parsed.files[0], &parsed.files[1]);
    assert!(small.is_in_memory());
    assert!(!big.is_in_memory());
    assert_eq!(big.size(), large.len() as u64);
    assert_eq!(big.sha256(), crate::resource::blob::blob_id(&large));
    assert_eq!(big.read_bytes().unwrap(), large);
    let source = big.source().unwrap();
    assert_eq!(source.read_at(49_990, 100).unwrap(), large[49_990..]);
    let mut read = Vec::new();
    big.reader().unwrap().read_to_end(&mut read).unwrap();
    assert_eq!(read, large);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    drop(source);
    drop(parsed);
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "the spool file goes with the upload"
    );

    // a field over its limit, and a body cut short in a spooled part, leave nothing
    let field = body(&[("Content-Disposition: form-data; name=\"x\"", &[b'x'; 200])]);
    assert_eq!(
        fed(&field, 50, Some(spool.clone())).unwrap_err(),
        MultipartError::FieldTooLarge("x".to_string())
    );
    let mut reader = MultipartReader::new(TYPE, Some(spool.clone())).unwrap();
    reader.feed(&data[..data.len() / 2]).unwrap();
    assert!(reader.finish().is_err());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_boundary_is_read_with_or_without_quotes() {
    assert_eq!(boundary_of("multipart/form-data; boundary=abc").as_deref(), Some("abc"));
    assert_eq!(
        boundary_of("multipart/form-data; charset=x; boundary=\"a b\"").as_deref(),
        Some("a b")
    );
    assert_eq!(boundary_of("multipart/form-data"), None);
    assert_eq!(boundary_of("multipart/form-data; boundary="), None);
}

#[test]
fn a_disposition_parameter_is_read_quoted_or_bare() {
    let value = "form-data; name=\"a\\\"b\"; filename=\"x\\\\y.txt\"";
    assert_eq!(disposition_param(value, "name").as_deref(), Some("a\"b"));
    assert_eq!(disposition_param(value, "FileName").as_deref(), Some("x\\y.txt"));
    assert_eq!(disposition_param(value, "size"), None);
    // a semicolon or a parameter inside a quoted value is part of it
    let value = "form-data; name=\"a; filename=b\"; filename=c.txt";
    assert_eq!(disposition_param(value, "name").as_deref(), Some("a; filename=b"));
    assert_eq!(disposition_param(value, "filename").as_deref(), Some("c.txt"));
    assert_eq!(
        disposition_param("form-data;name= bare ;filename=f", "name").as_deref(),
        Some("bare")
    );
    assert_eq!(
        disposition_param("form-data; name=\"open", "name").as_deref(),
        Some("open")
    );
    assert_eq!(
        disposition_param("form-data; name=\"\\n\"", "name").as_deref(),
        Some("\\n")
    );
    assert_eq!(
        disposition_param("form-data; name=\"é\"; x=ü", "x").as_deref(),
        Some("ü")
    );
    assert_eq!(disposition_param("form-data", "name"), None);
    assert_eq!(disposition_param("form-data; name", "name"), None);
}

#[test]
fn programs_are_recognised_by_their_first_bytes() {
    assert!(is_executable(b"MZ\x90\x00"));
    assert!(is_executable(&[0x7f, 0x45, 0x4c, 0x46, 2]));
    assert!(is_executable(&[0xca, 0xfe, 0xba, 0xbe]));
    assert!(is_executable(&[0xcf, 0xfa, 0xed, 0xfe]));
    // The other Mach-O magics from SecurityMisconfigTests. A PNG is not a program.
    assert!(is_executable(&[0xfe, 0xed, 0xfa, 0xce]));
    assert!(is_executable(&[0xfe, 0xed, 0xfa, 0xcf]));
    assert!(is_executable(&[0xce, 0xfa, 0xed, 0xfe]));
    assert!(!is_executable(&[0x89, 0x50, 0x4e, 0x47]));
    assert!(is_executable(b"#!/bin/sh\n"));
    assert!(!is_executable(b"PK\x03\x04"));
    assert!(!is_executable(b"plain text"));
    assert!(!is_executable(b""));
}
