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

use std::collections::BTreeMap;

use super::{expected, mismatches, parse_sums, Part, CHECKSUMS_SETTING, DEFAULT_FILE};

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn the_runtime_checksum_is_over_the_index_then_the_blob() {
    use sha2::{Digest, Sha256};
    let (index, blob) = moqui_embedded::packed();
    let mut hasher = Sha256::new();
    hasher.update(index);
    hasher.update(blob);
    let sha256 = Part::Runtime.sha256().unwrap();
    assert_eq!(sha256, moqui_lib::codec::hex_encode(&hasher.finalize()));
    assert_eq!(sha256.len(), 64);
    assert_eq!(
        Part::OpenSearch.sha256().is_some(),
        moqui_embedded::programs::opensearch().is_some()
    );
}

#[test]
fn a_sums_file_names_parts_and_skips_other_files_and_comments() {
    let text = format!(
        "# made by moqui embedded checksum\n{A}  runtime\n{}  *postgres\n\n{B}  moqui-20261114.gz\n",
        B.to_uppercase()
    );
    let sums = parse_sums(&text, "embedded.sha256").unwrap();
    assert_eq!(sums.len(), 2);
    assert_eq!(sums[&Part::Runtime].sha256, A);
    assert_eq!(sums[&Part::Postgres].sha256, B, "upper case hex is read as lower");
    assert_eq!(sums[&Part::Runtime].source, "embedded.sha256");

    let err = parse_sums("abc  runtime\n", "sums").unwrap_err().to_string();
    assert!(err.contains("sums line 1"), "{err}");
}

#[test]
fn the_expected_checksums_come_from_the_named_file_or_the_runtime_and_the_variables_win() {
    let dir = std::env::temp_dir().join(format!("moqui-embedded-sums-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let none = |_: &str| String::new();
    assert!(
        expected(&dir, &none).unwrap().is_empty(),
        "nothing given, nothing checked"
    );

    std::fs::write(dir.join(DEFAULT_FILE), format!("{A}  runtime\n{A}  chromium\n")).unwrap();
    let from_runtime = expected(&dir, &none).unwrap();
    assert_eq!(from_runtime.len(), 2);
    assert!(from_runtime[&Part::Runtime].source.ends_with(DEFAULT_FILE));

    let other = dir.join("other.sha256");
    std::fs::write(&other, format!("{B}  postgres\n")).unwrap();
    let other_name = other.display().to_string();
    let lookup = |name: &str| match name {
        CHECKSUMS_SETTING => other_name.clone(),
        "MOQUI_EMBEDDED_SHA256_RUNTIME" => format!(" {B} "),
        _ => String::new(),
    };
    let both = expected(&dir, &lookup).unwrap();
    assert_eq!(both.len(), 2, "the named file in place of the runtime's: {both:?}");
    assert_eq!(both[&Part::Postgres].sha256, B);
    assert_eq!(both[&Part::Runtime].sha256, B);
    assert_eq!(both[&Part::Runtime].source, "MOQUI_EMBEDDED_SHA256_RUNTIME");

    let missing = |name: &str| match name {
        CHECKSUMS_SETTING => dir.join("missing.sha256").display().to_string(),
        _ => String::new(),
    };
    assert!(expected(&dir, &missing).is_err(), "a file named must be there");
    let bad = |name: &str| match name {
        "MOQUI_EMBEDDED_SHA256_OPENSEARCH" => "not-hex".to_string(),
        _ => String::new(),
    };
    let err = expected(&dir, &bad).unwrap_err().to_string();
    assert!(err.contains("MOQUI_EMBEDDED_SHA256_OPENSEARCH"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_part_that_differs_or_is_not_built_in_does_not_match() {
    let expect = |sha256: &str| super::Expected {
        sha256: sha256.to_string(),
        source: "sums".to_string(),
    };
    let expected = BTreeMap::from([
        (Part::Runtime, expect(A)),
        (Part::Postgres, expect(A)),
        (Part::Chromium, expect(A)),
    ]);
    let actual = |part: Part| match part {
        Part::Runtime => Some(A.to_string()),
        Part::Postgres => Some(B.to_string()),
        _ => None,
    };
    let failed = mismatches(&expected, &actual);
    let found: Vec<(Part, Option<&str>)> = failed.iter().map(|(part, _, have)| (*part, have.as_deref())).collect();
    assert_eq!(found, vec![(Part::Postgres, Some(B)), (Part::Chromium, None)]);
}

#[test]
fn each_part_has_its_variable() {
    assert_eq!(Part::Runtime.env_var(), "MOQUI_EMBEDDED_SHA256_RUNTIME");
    assert_eq!(Part::OpenSearch.env_var(), "MOQUI_EMBEDDED_SHA256_OPENSEARCH");
    assert_eq!(Part::named("chromium"), Some(Part::Chromium));
    assert_eq!(Part::named("other"), None);
}
