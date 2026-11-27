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
use crate::context::ExecutionContextFactory;

/// Where a whole-database archive goes is its download (plans/archive/Security-Review-Medium.md SEC-TLS-83): a caller without
/// ADMIN_BACKUP_DOWNLOAD who names a directory or a mirror other than a backup job's is refused before anything is
/// dumped; the job's own destination, the default one and a call with authorization disabled pass.
#[test]
fn a_destination_other_than_a_jobs_needs_the_download_permission() {
    let facade = crate::testing::open();
    facade.set_disable_fk_create(true);
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let text = |value: &str| Value::string(value);
    {
        let _authz = ctx.disable_authz();
        let mut job = facade.make_value("moqui.service.job.ServiceJob").unwrap();
        job.set("jobName", text("spec_backup"));
        job.set(
            "serviceName",
            text("org.moqui.impl.ServerServices.backup#EmbeddedDatabase"),
        );
        job.create(&facade).unwrap();
        let mut parameter = facade.make_value("moqui.service.job.ServiceJobParameter").unwrap();
        parameter.set("jobName", text("spec_backup"));
        parameter.set("parameterName", text("systemMessageRemoteId"));
        parameter.set("parameterValue", text("JobMirror"));
        parameter.create(&facade).unwrap();
        let mut account = facade.make_value("moqui.security.UserAccount").unwrap();
        account.set("userId", text("BKOPERATOR"));
        account.set("username", text("bkoperator"));
        account.create(&facade).unwrap();
    }
    crate::user::internal_login_user("bkoperator").unwrap();
    let call = |pairs: &[(&str, &str)]| {
        ctx.message().clear();
        let parameters: BTreeMap<String, Value> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), text(value)))
            .collect();
        super::backup_embedded_database(&parameters, &facade).unwrap();
        ctx.message().errors().join("\n")
    };
    let refused = |errors: String| errors.contains("ADMIN_BACKUP_DOWNLOAD");
    assert!(refused(call(&[("systemMessageRemoteId", "TakerMirror")])));
    assert!(refused(call(&[("backupDir", "/srv/www/static")])));
    // the job's own mirror, but sent plain to another recipient's key or with another transport: not the job's
    assert!(refused(call(&[
        ("systemMessageRemoteId", "JobMirror"),
        ("mirrorTransport", "s3")
    ])));
    assert!(!refused(call(&[("systemMessageRemoteId", "JobMirror")])));
    assert!(!refused(call(&[])));
    {
        let _authz = ctx.disable_authz();
        assert!(!refused(call(&[("systemMessageRemoteId", "TakerMirror")])));
    }
    crate::user::logout().unwrap();
}

#[test]
fn a_destination_reads_the_flags_and_the_transport_as_the_service_does() {
    let params = |pairs: &[(&str, Value)]| -> BTreeMap<String, Value> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect()
    };
    assert_eq!(
        super::destination(&params(&[("mirrorEncrypt", Value::Bool(false))])),
        super::destination(&params(&[]))
    );
    assert_eq!(
        super::destination(&params(&[
            ("mirrorTransport", Value::string(" S3 ")),
            ("mirrorEncrypt", Value::string("true"))
        ])),
        super::destination(&params(&[
            ("mirrorTransport", Value::string("s3")),
            ("mirrorEncrypt", Value::Bool(true))
        ]))
    );
}

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

/// SEC-TLS-96: the mirror reads its remote as the other senders do. A disabled remote receives nothing, and one whose
/// `allowPrivateNetwork` is N is connected to at no internal address (the SFTP sync and a git push mirror take the
/// same rule through `SftpClient::for_remote`).
#[test]
fn a_mirror_refuses_a_disabled_remote_and_keeps_a_public_one_off_internal_addresses() {
    let facade = crate::testing::open();
    facade.set_disable_fk_create(true);
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let connector = crate::sftp::tests::memory(ctx.factory());
    let _authz = ctx.disable_authz();
    let mut message_type = facade.make_value("moqui.service.message.SystemMessageType").unwrap();
    message_type.set("systemMessageTypeId", Value::string("BackupMirror"));
    message_type.set("sendPath", Value::string("/srv/bk"));
    message_type.create(&facade).unwrap();
    let remote = |id: &str, disabled: &str| {
        let mut row = facade.make_value("moqui.service.message.SystemMessageRemote").unwrap();
        for (name, value) in [
            ("systemMessageRemoteId", id),
            ("systemMessageTypeId", "BackupMirror"),
            ("sendUrl", "backup.example.com"),
            ("username", "joe"),
            ("password", "pw"),
            ("allowPrivateNetwork", "N"),
            ("disabled", disabled),
        ] {
            row.set(name, Value::string(value));
        }
        row.create(&facade).unwrap();
    };
    remote("Off", "Y");
    remote("Public", "N");
    let options = MirrorOptions {
        transport: None,
        recipient: None,
    };
    let outcome = crate::backup::BackupOutcome {
        skipped: false,
        backup_dir: PathBuf::new(),
        archive_path: PathBuf::new(),
        backup_kind: "full".into(),
        bytes_written: 0,
        promoted: Vec::new(),
        removed: Vec::new(),
        keep: Vec::new(),
        renames: Vec::new(),
        rewritten: Vec::new(),
        errors: Vec::new(),
    };
    let refused = super::mirror(&facade, "Off", &options, &outcome).unwrap_err();
    assert!(refused.message().contains("disabled"), "{refused}");
    assert!(connector.seen.lock().unwrap().is_empty());
    super::mirror(&facade, "Public", &options, &outcome).unwrap();
    let seen = connector.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].deny_internal);
}
