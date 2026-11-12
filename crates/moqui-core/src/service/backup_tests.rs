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
use std::path::PathBuf;

use moqui_lib::object::Value;

use super::{backup_dir, mirror_options, paused_for, MirrorOptions, Transport};

#[test]
fn the_call_names_the_directory_before_the_property() {
    assert_eq!(backup_dir("/call", "/property"), Some(PathBuf::from("/call")));
    assert_eq!(backup_dir("  ", " /property "), Some(PathBuf::from("/property")));
    assert_eq!(backup_dir("", ""), None);
}

#[test]
fn the_schedule_property_runs_or_pauses_the_job_and_otherwise_leaves_it() {
    assert_eq!(paused_for("true"), Some("N"));
    assert_eq!(paused_for(" TRUE "), Some("N"));
    assert_eq!(paused_for("false"), Some("Y"));
    assert_eq!(paused_for(""), None);
    assert_eq!(paused_for("maybe"), None);
}

#[test]
fn the_mirror_flags_pick_the_transport_and_the_encryption() {
    let params = |pairs: &[(&str, Value)]| -> BTreeMap<String, Value> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect()
    };
    assert_eq!(
        mirror_options(&params(&[])).unwrap(),
        MirrorOptions {
            transport: None,
            recipient: None
        }
    );
    let on = mirror_options(&params(&[
        ("mirrorTransport", Value::string("S3")),
        ("mirrorEncrypt", Value::Bool(true)),
        ("mirrorRecipient", Value::string(" age1x ")),
    ]))
    .unwrap();
    assert_eq!(
        on,
        MirrorOptions {
            transport: Some(Transport::S3),
            recipient: Some("age1x".into())
        }
    );
    // a recipient without the flag does not encrypt
    let off = mirror_options(&params(&[
        ("mirrorTransport", Value::string("sftp")),
        ("mirrorRecipient", Value::string("age1x")),
    ]))
    .unwrap();
    assert_eq!(off.transport, Some(Transport::Sftp));
    assert_eq!(off.recipient, None);
    assert!(mirror_options(&params(&[("mirrorEncrypt", Value::Bool(true))])).is_err());
    assert!(mirror_options(&params(&[("mirrorTransport", Value::string("ftp"))])).is_err());
}
