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

use std::io::Write;

use super::*;

#[test]
fn metadata_as_stored_or_gzipped() {
    let json = br#"{"name":"spec","vector_layers":[{"id":"earth"}]}"#;
    assert_eq!(metadata_text(json, Compression::None).unwrap(), json.to_vec());
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(json).unwrap();
    let gzipped = encoder.finish().unwrap();
    assert_eq!(metadata_text(&gzipped, Compression::Gzip).unwrap(), json.to_vec());
    assert!(metadata_text(b"not gzip", Compression::Gzip)
        .unwrap_err()
        .contains("does not decompress"));
    assert!(metadata_text(json, Compression::Zstd).unwrap_err().contains("zstd"));
}
