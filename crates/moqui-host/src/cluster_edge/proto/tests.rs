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

use std::path::PathBuf;

/// The generated file, as tonic-prost-build names it for the proto package.
const GENERATED: &str = "moqui.cluster.v1.rs";

#[test]
fn the_checked_in_code_is_what_the_proto_generates() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let proto_dir = manifest.join("proto");
    let checked_in = manifest.join("src/cluster_edge/proto").join(GENERATED);
    let out = std::env::temp_dir().join(format!("moqui-cluster-proto-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let descriptors = protox::compile([proto_dir.join("cluster.proto")], [&proto_dir]).unwrap();
    tonic_prost_build::configure()
        .build_transport(false)
        .out_dir(&out)
        .compile_fds(descriptors)
        .unwrap();
    let generated = std::fs::read_to_string(out.join(GENERATED)).unwrap();
    let _ = std::fs::remove_dir_all(&out);
    if std::env::var_os("MOQUI_REGENERATE_PROTO").is_some() {
        std::fs::write(&checked_in, &generated).unwrap();
        return;
    }
    let current = std::fs::read_to_string(&checked_in).unwrap_or_default();
    assert!(
        current == generated,
        "{} is not what proto/cluster.proto generates; rerun with MOQUI_REGENERATE_PROTO=1 and read the diff",
        checked_in.display()
    );
}
