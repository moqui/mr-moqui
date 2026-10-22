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

//! The sync engine against the scripted local servers of [`super::fake_servers`] (no network): the first sync with
//! special-use folders, files and threads; new UIDs, flags and expunges with and without CONDSTORE and QRESYNC; a
//! UIDVALIDITY reset; moves by another client; local changes onto the server (next sync and at once); a malformed
//! message; a refused sign-in; POP3 by UIDL; APPEND of sent mail; the job and the lock; rules, block and spam;
//! autodiscover; import; and an IDLE worker.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use moqui_lib::object::{basic_convert, to_plain_string, Value};

use super::fake_servers::{fake_dns, fake_http, FakeImap, FakePop};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};

const SERVICES: &str = "org.moqui.impl.EmailClientServices";
const MESSAGE: &str = "moqui.basic.email.EmailMessage";
const FOLDER: &str = "moqui.basic.email.EmailFolder";

pub(super) fn setup() -> (ExecutionContext, EntityFacade) {
    setup_with("")
}

/// [`setup`] with the conf text `conf`.
pub(super) fn setup_with(conf: &str) -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context_in(
        conf,
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    // the attachments are DbResource files; the in-memory database has every table
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

pub(super) fn install() {
    let framework = crate::context::framework_dir().join("service/org/moqui");
    let paths = vec![
        framework.join("EmailServices.xml"),
        framework.join("impl/EmailServices.xml"),
        framework.join("impl/EmailClientServices.xml"),
        framework.join("impl/ShellServices.xml"),
    ];
    let failures = crate::service::lint(&paths).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

pub(super) fn text(value: &str) -> Value {
    Value::string(value)
}

pub(super) fn map(pairs: &[(&str, &str)]) -> Value {
    Value::Map(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
            .collect::<IndexMap<_, _>>(),
    )
}

pub(super) fn stamp(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
}

pub(super) fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

pub(super) fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, Value)]) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value(entity).unwrap();
    for (field, value) in fields {
        row.set(*field, value.clone());
    }
    row.create(facade).unwrap();
}

pub(super) fn update(facade: &EntityFacade, entity: &str, key: (&str, &str), fields: &[(&str, Value)]) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value(entity).unwrap();
    row.set(key.0, text(key.1));
    for (field, value) in fields {
        row.set(*field, value.clone());
    }
    row.update(facade).unwrap();
}

pub(super) fn user(facade: &EntityFacade, user_id: &str) {
    row(
        facade,
        "moqui.security.UserAccount",
        &[
            ("userId", text(user_id)),
            ("username", text(user_id)),
            ("emailAddress", text(&format!("{user_id}@example.com"))),
        ],
    );
}

pub(super) fn login(user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    crate::user::internal_login_user(user_id).unwrap();
}

pub(super) fn call(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    crate::service::sync(&format!("{SERVICES}.{name}"), &parameters).unwrap_or_else(|err| panic!("{name}: {err}"))
}

pub(super) fn errors() -> String {
    ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default()
}

pub(super) fn ok(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let out = call(name, parameters);
    let errors = errors();
    assert!(errors.is_empty(), "{name}: {errors}");
    out
}

pub(super) fn out_text(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

pub(super) fn out_number(out: &BTreeMap<String, Value>, key: &str) -> i64 {
    out.get(key)
        .map(to_plain_string)
        .and_then(|text| text.parse().ok())
        .unwrap_or(-1)
}

pub(super) fn field(row: &EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

pub(super) fn rows(facade: &EntityFacade, entity: &str, conditions: &[(&str, &str)]) -> Vec<EntityValue> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut find = facade.find(entity).unwrap();
    for (name, value) in conditions {
        find = find.eq(name, text(value));
    }
    find.list().unwrap().values().to_vec()
}

pub(super) fn one(facade: &EntityFacade, entity: &str, key: &str, id: &str) -> EntityValue {
    rows(facade, entity, &[(key, id)])
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no {entity} {id}"))
}

pub(super) fn folder_id(facade: &EntityFacade, account: &str, kind: &str) -> String {
    let folders = rows(
        facade,
        FOLDER,
        &[("emailAccountId", account), ("folderKindEnumId", kind)],
    );
    field(&folders[0], "emailFolderId")
}

pub(super) fn folder_by_name(facade: &EntityFacade, account: &str, name: &str) -> Option<EntityValue> {
    rows(facade, FOLDER, &[("emailAccountId", account)])
        .into_iter()
        .find(|folder| field(folder, "folderName") == name)
}

/// The message of an account with this Message-ID.
pub(super) fn by_message_id(facade: &EntityFacade, account: &str, message_id: &str) -> Option<EntityValue> {
    rows(
        facade,
        MESSAGE,
        &[("emailAccountId", account), ("messageId", message_id)],
    )
    .into_iter()
    .next()
}

fn label_names(facade: &EntityFacade, email_message_id: &str) -> Vec<String> {
    let mut names: Vec<String> = rows(
        facade,
        "moqui.basic.email.EmailMessageLabel",
        &[("emailMessageId", email_message_id)],
    )
    .iter()
    .map(|link| {
        field(
            &one(
                facade,
                "moqui.basic.email.EmailLabel",
                "emailLabelId",
                &field(link, "emailLabelId"),
            ),
            "labelName",
        )
    })
    .collect();
    names.sort();
    names
}

pub(super) fn mail(message_id: &str, from: &str, to: &str, subject: &str, headers: &str, body: &str) -> String {
    format!(
        "Message-ID: {message_id}\nFrom: {from}\nTo: {to}\nSubject: {subject}\nDate: Mon, 19 Oct 2026 10:00:00 +0000\n{headers}MIME-Version: 1.0\nContent-Type: text/plain; charset=utf-8\n\n{body}\n"
    )
}

const FILES_MAIL: &str = "Message-ID: <files@example.com>
From: Carol <carol@example.com>
To: alice@example.com
Subject: Files
Date: Mon, 19 Oct 2026 11:00:00 +0000
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary=\"mix\"

--mix
Content-Type: multipart/related; boundary=\"rel\"

--rel
Content-Type: text/html; charset=utf-8

<p>See <img src=\"cid:logo@x\"> and the report</p>
--rel
Content-Type: image/png
Content-ID: <logo@x>
Content-Disposition: inline; filename=\"logo.png\"
Content-Transfer-Encoding: base64

iVBORw0KGgo=
--rel--
--mix
Content-Type: application/pdf; name=\"report.pdf\"
Content-Disposition: attachment; filename=\"report.pdf\"
Content-Transfer-Encoding: base64

JVBERi0xLjQK
--mix--
";

/// An account of alice's on the fake IMAP server.
pub(super) fn imap_account(port: u16, password: &str) -> String {
    out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("emailAddress", text("alice@example.com")),
                ("accountName", text("Alice")),
                ("smtpHost", text("127.0.0.1")),
                ("smtpPort", text("2525")),
                ("smtpStartTls", text("N")),
                ("storeHost", text("127.0.0.1")),
                ("storePort", text(&port.to_string())),
                ("storeProtocol", text("imap")),
                ("mailUsername", text("alice")),
                ("mailPassword", text(password)),
            ],
        ),
        "emailAccountId",
    )
}

pub(super) fn sync(account: &str) -> BTreeMap<String, Value> {
    let out = ok("sync#EmailAccount", &[("emailAccountId", text(account))]);
    assert_eq!(out_text(&out, "outcome"), "synced", "{out:?}");
    out
}

/// A session with alice signed in.
pub(super) fn alice(facade: &EntityFacade) {
    user(facade, "alice");
    login("alice");
}

fn changes(facade: &EntityFacade, account: &str) -> usize {
    rows(
        facade,
        "moqui.basic.email.EmailRemoteChange",
        &[("emailAccountId", account)],
    )
    .len()
}

/// The first sync: the server's folders by special use and name, the messages with their status, flags, files
/// (DbResource) and threads (a reply stored before the message it answers joins its thread), then a sync with
/// nothing new that reads no folder.
#[test]
fn first_sync_maps_folders_and_stores_messages_files_and_threads() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS", "MOVE", "SPECIAL-USE"]);
    imap.add_folder("Projects", "\\HasChildren", false);
    imap.add_folder("Projects/Alpha", "\\HasNoChildren", true);
    imap.add_folder("&ZeVnLIqe-", "\\HasNoChildren", true);
    imap.add(
        "Sent Mail",
        &mail(
            "<s1@example.com>",
            "alice@example.com",
            "bob@example.com",
            "Plan",
            "",
            "The plan",
        ),
        &["\\Seen"],
    );
    imap.add(
        "INBOX",
        &mail(
            "<r1@example.com>",
            "Bob <bob@example.com>",
            "alice@example.com",
            "Re: Plan",
            "In-Reply-To: <s1@example.com>\nReferences: <s1@example.com>\n",
            "Sounds good",
        ),
        &[],
    );
    imap.add("INBOX", FILES_MAIL, &["\\Seen", "\\Flagged"]);
    let account = imap_account(imap.port, "app-password");
    let out = sync(&account);
    assert_eq!(out_number(&out, "newCount"), 3, "{out:?}");

    // the folders: the account's own by special use, the others made, a \Noselect one not
    let sent = one(
        &facade,
        FOLDER,
        "emailFolderId",
        &folder_id(&facade, &account, "EfkSent"),
    );
    assert_eq!(field(&sent, "folderPath"), "Sent Mail");
    assert_eq!(
        field(
            &one(
                &facade,
                FOLDER,
                "emailFolderId",
                &folder_id(&facade, &account, "EfkTrash")
            ),
            "folderPath"
        ),
        "Trash"
    );
    let alpha = folder_by_name(&facade, &account, "Alpha").expect("Alpha");
    assert_eq!(field(&alpha, "folderPath"), "Projects/Alpha");
    assert_eq!(field(&alpha, "folderKindEnumId"), "EfkOther");
    assert!(folder_by_name(&facade, &account, "日本語").is_some());
    assert!(folder_by_name(&facade, &account, "Projects").is_none());

    // the messages
    let reply = by_message_id(&facade, &account, "<r1@example.com>").expect("reply");
    let plan = by_message_id(&facade, &account, "<s1@example.com>").expect("sent");
    let files = by_message_id(&facade, &account, "<files@example.com>").expect("files");
    assert_eq!(field(&reply, "statusId"), "ES_RECEIVED");
    assert_eq!(field(&reply, "isRead"), "N");
    assert_eq!(field(&reply, "fromAddress"), "bob@example.com");
    assert_eq!(field(&reply, "fromName"), "Bob");
    assert_eq!(field(&reply, "snippet"), "Sounds good");
    assert_eq!(field(&reply, "remoteUid"), "1");
    assert_eq!(field(&plan, "statusId"), "ES_SENT");
    assert_eq!(field(&plan, "emailFolderId"), field(&sent, "emailFolderId"));
    // the reply came first and joined the thread of the message it answers when that came
    assert_eq!(field(&reply, "parentEmailMessageId"), field(&plan, "emailMessageId"));
    assert_eq!(field(&reply, "rootEmailMessageId"), field(&plan, "emailMessageId"));
    assert_eq!(field(&files, "isRead"), "Y");
    assert_eq!(field(&files, "isFlagged"), "Y");
    assert_eq!(field(&files, "hasAttachment"), "Y");
    assert!(
        field(&files, "body").contains("cid:logo@x"),
        "{}",
        field(&files, "body")
    );
    let attachments = rows(
        &facade,
        "moqui.basic.email.EmailMessageAttachment",
        &[("emailMessageId", &field(&files, "emailMessageId"))],
    );
    assert_eq!(attachments.len(), 2);
    let logo = attachments
        .iter()
        .find(|row| field(row, "fileName") == "logo.png")
        .expect("logo");
    assert_eq!(field(logo, "isInline"), "Y");
    assert_eq!(field(logo, "contentId"), "logo@x");
    let report = attachments
        .iter()
        .find(|row| field(row, "fileName") == "report.pdf")
        .expect("report");
    assert_eq!(field(report, "isInline"), "N");
    let location = field(report, "attachmentLocation");
    assert!(
        location.starts_with(&format!("dbresource://email/{account}/")),
        "{location}"
    );
    let bytes = ctx.factory().resource().get(&location).unwrap().read_bytes().unwrap();
    assert_eq!(bytes, b"%PDF-1.4\n");

    // the folder's state and the account's
    let inbox = one(
        &facade,
        FOLDER,
        "emailFolderId",
        &folder_id(&facade, &account, "EfkInbox"),
    );
    assert_eq!(field(&inbox, "remoteUidValidity"), "1000");
    assert_eq!(field(&inbox, "remoteUidNext"), "3");
    assert_eq!(field(&inbox, "totalCount"), "2");
    let row = one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &account);
    assert_eq!(field(&row, "statusId"), "EasActive");
    assert!(!field(&row, "lastSyncDate").is_empty());
    assert!(field(&row, "syncLockedUntil").is_empty());

    // nothing new: STATUS says so, and no folder is read
    imap.clear_log();
    let again = sync(&account);
    assert_eq!(out_number(&again, "newCount"), 0);
    let commands = imap.commands();
    assert!(
        commands.iter().any(|command| command.starts_with("STATUS")),
        "{commands:?}"
    );
    assert!(
        !commands.iter().any(|command| command.starts_with("SELECT")),
        "{commands:?}"
    );
}

/// Without CONDSTORE: new UIDs, the flag scan when due, and an expunge found by comparing UIDs, with and without a scan.
#[test]
fn incremental_sync_flag_scan_and_expunge_without_condstore() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS"]);
    let one_uid = imap.add(
        "INBOX",
        &mail("<a1@example.com>", "x@example.com", "alice@example.com", "One", "", "1"),
        &[],
    );
    let two_uid = imap.add(
        "INBOX",
        &mail("<a2@example.com>", "x@example.com", "alice@example.com", "Two", "", "2"),
        &[],
    );
    let account = imap_account(imap.port, "app-password");
    sync(&account);
    let inbox = folder_id(&facade, &account, "EfkInbox");

    imap.add(
        "INBOX",
        &mail(
            "<a3@example.com>",
            "x@example.com",
            "alice@example.com",
            "Three",
            "",
            "3",
        ),
        &[],
    );
    imap.set_flags("INBOX", one_uid, &["\\Seen", "\\Answered"]);
    imap.expunge("INBOX", two_uid);
    // the flag scan is due
    update(
        &facade,
        FOLDER,
        ("emailFolderId", &inbox),
        &[("lastFlagScanDate", stamp(now() - 3_600_000))],
    );
    imap.clear_log();
    let out = sync(&account);
    assert_eq!(out_number(&out, "newCount"), 1, "{out:?}");
    assert_eq!(out_number(&out, "removedCount"), 1, "{out:?}");
    assert!(
        imap.commands()
            .iter()
            .any(|command| command == "UID FETCH 1:* (UID FLAGS)"),
        "{:?}",
        imap.commands()
    );
    let first = by_message_id(&facade, &account, "<a1@example.com>").unwrap();
    assert_eq!(field(&first, "isRead"), "Y");
    assert_eq!(field(&first, "isAnswered"), "Y");
    assert!(by_message_id(&facade, &account, "<a2@example.com>").is_none());
    assert!(by_message_id(&facade, &account, "<a3@example.com>").is_some());

    // an expunge with no scan due: the message count shows it, and the UIDs say which
    let third = by_message_id(&facade, &account, "<a3@example.com>").unwrap();
    imap.expunge("INBOX", field(&third, "remoteUid").parse().unwrap());
    imap.add(
        "INBOX",
        &mail(
            "<a4@example.com>",
            "x@example.com",
            "alice@example.com",
            "Four",
            "",
            "4",
        ),
        &[],
    );
    imap.add(
        "INBOX",
        &mail(
            "<a5@example.com>",
            "x@example.com",
            "alice@example.com",
            "Five",
            "",
            "5",
        ),
        &[],
    );
    imap.clear_log();
    let out = sync(&account);
    assert_eq!(out_number(&out, "newCount"), 2, "{out:?}");
    assert_eq!(out_number(&out, "removedCount"), 1, "{out:?}");
    assert!(
        imap.commands()
            .iter()
            .any(|command| command.starts_with("UID SEARCH UID 1:*")),
        "{:?}",
        imap.commands()
    );
    assert!(by_message_id(&facade, &account, "<a3@example.com>").is_none());
}

/// CONDSTORE and QRESYNC: the flags that changed from CHANGEDSINCE, the expunges from VANISHED; with CONDSTORE alone the
/// expunge is found by the UIDs.
#[test]
fn condstore_and_qresync_bring_flag_changes_and_expunges() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    for qresync in [true, false] {
        let caps: &[&str] = if qresync {
            &["CONDSTORE", "QRESYNC", "ENABLE", "UIDPLUS", "MOVE"]
        } else {
            &["CONDSTORE", "ENABLE", "UIDPLUS"]
        };
        let imap = FakeImap::start(caps);
        let tag = if qresync { "q" } else { "c" };
        let ids: Vec<String> = (1..=3).map(|n| format!("<{tag}{n}@example.com>")).collect();
        let uids: Vec<u32> = ids
            .iter()
            .map(|id| {
                imap.add(
                    "INBOX",
                    &mail(id, "x@example.com", "alice@example.com", id, "", "body"),
                    &[],
                )
            })
            .collect();
        let account = imap_account(imap.port, "app-password");
        sync(&account);
        let inbox = one(
            &facade,
            FOLDER,
            "emailFolderId",
            &folder_id(&facade, &account, "EfkInbox"),
        );
        assert!(!field(&inbox, "remoteHighestModSeq").is_empty(), "{qresync}");
        imap.set_flags("INBOX", uids[0], &["\\Flagged"]);
        imap.expunge("INBOX", uids[1]);
        imap.clear_log();
        let out = sync(&account);
        assert_eq!(out_number(&out, "removedCount"), 1, "{qresync} {out:?}");
        assert_eq!(
            field(&by_message_id(&facade, &account, &ids[0]).unwrap(), "isFlagged"),
            "Y",
            "{qresync}"
        );
        assert!(by_message_id(&facade, &account, &ids[1]).is_none(), "{qresync}");
        let commands = imap.commands();
        assert!(
            commands.iter().any(|command| command.contains("CHANGEDSINCE")),
            "{commands:?}"
        );
        if qresync {
            assert!(
                commands.iter().any(|command| command.contains("VANISHED")),
                "{commands:?}"
            );
            assert!(
                commands.iter().any(|command| command == "ENABLE QRESYNC"),
                "{commands:?}"
            );
        } else {
            assert!(
                commands.iter().any(|command| command.starts_with("UID SEARCH UID")),
                "{commands:?}"
            );
        }
    }
}

/// A new UIDVALIDITY: the folder is read again and each row is kept, with its new UID, by its Message-ID.
#[test]
fn uidvalidity_reset_reads_the_folder_again_and_keeps_the_rows() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS"]);
    imap.add(
        "INBOX",
        &mail("<v1@example.com>", "x@example.com", "alice@example.com", "One", "", "1"),
        &[],
    );
    imap.add(
        "INBOX",
        &mail("<v2@example.com>", "x@example.com", "alice@example.com", "Two", "", "2"),
        &["\\Seen"],
    );
    let account = imap_account(imap.port, "app-password");
    sync(&account);
    let before = field(
        &by_message_id(&facade, &account, "<v1@example.com>").unwrap(),
        "emailMessageId",
    );
    imap.reset_validity("INBOX");
    let out = sync(&account);
    assert_eq!(out_number(&out, "newCount"), 0, "{out:?}");
    assert_eq!(out_number(&out, "removedCount"), 0, "{out:?}");
    let after = by_message_id(&facade, &account, "<v1@example.com>").unwrap();
    assert_eq!(field(&after, "emailMessageId"), before);
    assert_eq!(field(&after, "remoteUid"), "100");
    assert_eq!(
        field(
            &by_message_id(&facade, &account, "<v2@example.com>").unwrap(),
            "remoteUid"
        ),
        "101"
    );
    let inbox = one(
        &facade,
        FOLDER,
        "emailFolderId",
        &folder_id(&facade, &account, "EfkInbox"),
    );
    assert_ne!(field(&inbox, "remoteUidValidity"), "1000");
    assert_eq!(rows(&facade, MESSAGE, &[("emailAccountId", &account)]).len(), 2);
}

/// A message another client moved keeps its row, whichever folder is read first.
#[test]
fn a_message_moved_by_another_client_keeps_its_row() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS", "MOVE"]);
    let uid = imap.add(
        "INBOX",
        &mail(
            "<mv@example.com>",
            "x@example.com",
            "alice@example.com",
            "Move",
            "",
            "m",
        ),
        &[],
    );
    let account = imap_account(imap.port, "app-password");
    sync(&account);
    let id = field(
        &by_message_id(&facade, &account, "<mv@example.com>").unwrap(),
        "emailMessageId",
    );
    let archive = folder_id(&facade, &account, "EfkArchive");
    let inbox = folder_id(&facade, &account, "EfkInbox");

    // out of the Inbox (read first) into Archive (read later)
    let archived_uid = imap.move_message("INBOX", uid, "Archive");
    let out = sync(&account);
    assert_eq!(out_number(&out, "removedCount"), 0, "{out:?}");
    assert_eq!(out_number(&out, "newCount"), 0, "{out:?}");
    let row = one(&facade, MESSAGE, "emailMessageId", &id);
    assert_eq!(field(&row, "emailFolderId"), archive);
    assert_eq!(field(&row, "remoteFolderId"), archive);
    assert_eq!(field(&row, "remoteUid"), archived_uid.to_string());
    assert!(!field(&row, "archivedDate").is_empty());

    // and back: the Inbox, read first, finds a Message-ID still kept in Archive, which it is decided at the end
    let back_uid = imap.move_message("Archive", archived_uid, "INBOX");
    let out = sync(&account);
    assert_eq!(out_number(&out, "removedCount"), 0, "{out:?}");
    assert_eq!(out_number(&out, "newCount"), 0, "{out:?}");
    let row = one(&facade, MESSAGE, "emailMessageId", &id);
    assert_eq!(field(&row, "emailFolderId"), inbox);
    assert_eq!(field(&row, "remoteUid"), back_uid.to_string());
    assert_eq!(rows(&facade, MESSAGE, &[("emailAccountId", &account)]).len(), 1);
}

/// Local changes reach the server at the next sync: flags, a move (MOVE, and COPY with STORE and EXPUNGE without it), a
/// delete, a folder made and renamed; nothing is left queued.
#[test]
fn local_changes_reach_the_server_at_the_next_sync() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    for with_move in [true, false] {
        let caps: &[&str] = if with_move { &["UIDPLUS", "MOVE"] } else { &["UIDPLUS"] };
        let imap = FakeImap::start(caps);
        let tag = if with_move { "m" } else { "c" };
        for n in 1..=3 {
            let id = format!("<{tag}{n}@example.com>");
            imap.add(
                "INBOX",
                &mail(&id, "x@example.com", "alice@example.com", &id, "", "body"),
                &[],
            );
        }
        let account = imap_account(imap.port, "app-password");
        sync(&account);
        let id = |n: u32| {
            field(
                &by_message_id(&facade, &account, &format!("<{tag}{n}@example.com>")).unwrap(),
                "emailMessageId",
            )
        };
        ok(
            "set#MessageFlags",
            &[
                ("emailMessageId", text(&id(1))),
                ("read", Value::Bool(true)),
                ("flagged", Value::Bool(true)),
            ],
        );
        let archive = folder_id(&facade, &account, "EfkArchive");
        ok(
            "move#Messages",
            &[("emailMessageId", text(&id(2))), ("emailFolderId", text(&archive))],
        );
        ok("trash#Messages", &[("emailMessageId", text(&id(3)))]);
        ok(
            "trash#Messages",
            &[("emailMessageId", text(&id(3))), ("deleteForever", Value::Bool(true))],
        );
        let clients = out_text(
            &ok(
                "create#EmailFolder",
                &[("emailAccountId", text(&account)), ("folderName", text("Clients"))],
            ),
            "emailFolderId",
        );
        ok(
            "rename#EmailFolder",
            &[("emailFolderId", text(&clients)), ("folderName", text("Customers"))],
        );
        assert!(changes(&facade, &account) >= 5, "{}", changes(&facade, &account));
        let moved = id(2);
        let out = sync(&account);
        assert!(out_number(&out, "pushedCount") >= 5, "{out:?}");
        assert_eq!(changes(&facade, &account), 0);
        let inbox = imap.messages("INBOX");
        let first = inbox
            .iter()
            .find(|(_, _, mid)| mid == &format!("<{tag}1@example.com>"))
            .expect("first");
        assert!(
            first.1.contains(&"\\Seen".to_string()) && first.1.contains(&"\\Flagged".to_string()),
            "{first:?}"
        );
        assert_eq!(inbox.len(), 1, "{inbox:?}");
        let archived = imap.messages("Archive");
        assert_eq!(archived.len(), 1, "{archived:?}");
        assert!(imap.messages("Trash").is_empty(), "{:?}", imap.messages("Trash"));
        let row = one(&facade, MESSAGE, "emailMessageId", &moved);
        assert_eq!(field(&row, "remoteFolderId"), archive);
        assert_eq!(field(&row, "remoteUid"), archived[0].0.to_string());
        assert!(
            imap.folder_names().contains(&"Customers".to_string()),
            "{:?}",
            imap.folder_names()
        );
        assert_eq!(
            field(&one(&facade, FOLDER, "emailFolderId", &clients), "folderPath"),
            "Customers"
        );
        let commands = imap.commands();
        if with_move {
            assert!(
                commands.iter().any(|command| command.starts_with("UID MOVE")),
                "{commands:?}"
            );
        } else {
            assert!(
                commands.iter().any(|command| command.starts_with("UID COPY")),
                "{commands:?}"
            );
            assert!(
                commands.iter().any(|command| command.starts_with("UID EXPUNGE")),
                "{commands:?}"
            );
        }
    }
}

/// Turns the push right after a commit on for one test.
struct PushNow;

impl PushNow {
    fn on() -> Self {
        super::remote::PUSH_NOW_IN_TESTS.store(true, std::sync::atomic::Ordering::SeqCst);
        Self
    }
}

impl Drop for PushNow {
    fn drop(&mut self) {
        super::remote::PUSH_NOW_IN_TESTS.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// With the push on, a change is on the server when the call returns; with the server gone it waits, queued, and goes
/// at the next sync once the server is back.
#[test]
fn a_change_goes_at_once_and_waits_while_the_server_is_away() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS", "MOVE"]);
    imap.add(
        "INBOX",
        &mail(
            "<now@example.com>",
            "x@example.com",
            "alice@example.com",
            "Now",
            "",
            "n",
        ),
        &[],
    );
    let account = imap_account(imap.port, "app-password");
    sync(&account);
    let id = field(
        &by_message_id(&facade, &account, "<now@example.com>").unwrap(),
        "emailMessageId",
    );
    {
        let _push = PushNow::on();
        ok(
            "set#MessageFlags",
            &[("emailMessageId", text(&id)), ("flagged", Value::Bool(true))],
        );
    }
    assert_eq!(imap.messages("INBOX")[0].1, vec!["\\Flagged".to_string()]);
    assert_eq!(changes(&facade, &account), 0);

    // the server away: the change is kept
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let server = field(
        &one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &account),
        "emailServerId",
    );
    update(
        &facade,
        "moqui.basic.email.EmailServer",
        ("emailServerId", &server),
        &[("storePort", text(&closed.to_string()))],
    );
    {
        let _push = PushNow::on();
        ok(
            "set#MessageFlags",
            &[("emailMessageId", text(&id)), ("read", Value::Bool(true))],
        );
    }
    assert_eq!(changes(&facade, &account), 1);
    let failed = call("sync#EmailAccount", &[("emailAccountId", text(&account))]);
    assert_eq!(out_text(&failed, "outcome"), "failed");
    assert!(errors().contains("could not connect"));
    assert_eq!(changes(&facade, &account), 1);
    assert_eq!(
        field(
            &one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &account),
            "statusId"
        ),
        "EasError"
    );
    // back again
    update(
        &facade,
        "moqui.basic.email.EmailServer",
        ("emailServerId", &server),
        &[("storePort", text(&imap.port.to_string()))],
    );
    sync(&account);
    assert_eq!(changes(&facade, &account), 0);
    assert!(imap.messages("INBOX")[0].1.contains(&"\\Seen".to_string()));
    assert_eq!(
        field(
            &one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &account),
            "statusId"
        ),
        "EasActive"
    );
}

/// A message that is not mail is logged and skipped, the rest are stored and the folder moves past it.
#[test]
fn a_malformed_message_is_skipped_and_the_rest_are_stored() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS"]);
    imap.add("INBOX", "\u{0}\u{1}\u{2} this is not mail at all\n", &[]);
    imap.add(
        "INBOX",
        &mail(
            "<good@example.com>",
            "x@example.com",
            "alice@example.com",
            "Good",
            "",
            "g",
        ),
        &[],
    );
    let account = imap_account(imap.port, "app-password");
    let out = sync(&account);
    assert_eq!(out_number(&out, "newCount"), 1, "{out:?}");
    assert_eq!(out_number(&out, "skippedCount"), 1, "{out:?}");
    assert!(by_message_id(&facade, &account, "<good@example.com>").is_some());
    let inbox = one(
        &facade,
        FOLDER,
        "emailFolderId",
        &folder_id(&facade, &account, "EfkInbox"),
    );
    assert_eq!(field(&inbox, "remoteUidNext"), "3");
    // not read again
    let again = sync(&account);
    assert_eq!(out_number(&again, "skippedCount"), 0, "{again:?}");
}

/// A sign-in the server refuses marks the account EasAuthFailed with the reason; the job leaves it; new settings make
/// it Active again.
#[test]
fn a_refused_sign_in_marks_the_account() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS"]);
    imap.add(
        "INBOX",
        &mail(
            "<auth@example.com>",
            "x@example.com",
            "alice@example.com",
            "Auth",
            "",
            "a",
        ),
        &[],
    );
    let account = imap_account(imap.port, "wrong-password");
    let out = call("sync#EmailAccount", &[("emailAccountId", text(&account))]);
    assert_eq!(out_text(&out, "outcome"), "authFailed");
    assert!(errors().contains("refused the sign-in"));
    let row = one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &account);
    assert_eq!(field(&row, "statusId"), "EasAuthFailed");
    assert!(
        field(&row, "lastSyncError").contains("AUTHENTICATIONFAILED"),
        "{}",
        field(&row, "lastSyncError")
    );
    {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let job = ok("sync#DueEmailAccounts", &[]);
        assert_eq!(out_number(&job, "syncedCount"), 0);
    }
    ok(
        "update#EmailAccount",
        &[
            ("emailAccountId", text(&account)),
            ("mailPassword", text("app-password")),
        ],
    );
    assert_eq!(
        field(
            &one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &account),
            "statusId"
        ),
        "EasActive"
    );
    sync(&account);
    assert!(by_message_id(&facade, &account, "<auth@example.com>").is_some());
}

/// POP3: fetch-only into the Inbox by UIDL, each message once; DELE only when the server is set to delete.
#[test]
fn pop3_fetches_by_uidl_and_deletes_only_when_told() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let pop = FakePop::start();
    pop.add(
        "u1",
        &mail("<p1@example.com>", "x@example.com", "alice@example.com", "One", "", "1"),
    );
    pop.add(
        "u2",
        &mail(
            "<p2@example.com>",
            "x@example.com",
            "alice@example.com",
            "Two",
            "",
            ".leading dot\nend",
        ),
    );
    let account = out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("emailAddress", text("alice@example.com")),
                ("storeHost", text("127.0.0.1")),
                ("storePort", text(&pop.port.to_string())),
                ("storeProtocol", text("pop3")),
                ("mailUsername", text("alice")),
                ("mailPassword", text("pop-password")),
            ],
        ),
        "emailAccountId",
    );
    let out = sync(&account);
    assert_eq!(out_number(&out, "newCount"), 2, "{out:?}");
    let first = by_message_id(&facade, &account, "<p1@example.com>").unwrap();
    assert_eq!(field(&first, "remoteUidl"), "u1");
    assert_eq!(field(&first, "emailFolderId"), folder_id(&facade, &account, "EfkInbox"));
    assert_eq!(field(&first, "statusId"), "ES_RECEIVED");
    assert!(field(
        &by_message_id(&facade, &account, "<p2@example.com>").unwrap(),
        "bodyText"
    )
    .contains(".leading dot"));
    assert_eq!(out_number(&sync(&account), "newCount"), 0);
    assert_eq!(pop.uidls(), vec!["u1".to_string(), "u2".to_string()]);
    // set to delete: DELE after the store, gone at QUIT
    let server = field(
        &one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &account),
        "emailServerId",
    );
    update(
        &facade,
        "moqui.basic.email.EmailServer",
        ("emailServerId", &server),
        &[("storeDelete", text("Y"))],
    );
    pop.add(
        "u3",
        &mail(
            "<p3@example.com>",
            "x@example.com",
            "alice@example.com",
            "Three",
            "",
            "3",
        ),
    );
    assert_eq!(out_number(&sync(&account), "newCount"), 1);
    assert!(pop_log_has(&pop, "DELE 3"));
    assert_eq!(pop.uidls(), vec!["u1".to_string(), "u2".to_string()]);
}

pub(super) fn pop_log_has(pop: &FakePop, line: &str) -> bool {
    moqui_lib::sync::lock(&pop.log).iter().any(|known| known == line)
}

/// Sent mail is appended to the server's Sent folder after send#Draft, and a copy the server kept itself is found by
/// Message-ID instead of appended again.
#[test]
fn sent_mail_is_appended_to_sent_unless_the_server_kept_a_copy() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS", "MOVE"]);
    let account = imap_account(imap.port, "app-password");
    sync(&account);
    let _sink = super::SinkGuard::install();
    let send = |subject: &str| -> String {
        let draft = out_text(
            &ok(
                "save#Draft",
                &[
                    ("emailAccountId", text(&account)),
                    ("toAddresses", text("bob@example.com")),
                    ("subject", text(subject)),
                    ("body", text("<p>Hello</p>")),
                ],
            ),
            "emailMessageId",
        );
        let sent = ok("send#Draft", &[("emailMessageId", text(&draft))]);
        assert_eq!(sent.get("sent"), Some(&Value::Bool(true)), "{sent:?}");
        draft
    };
    let first = send("Appended");
    let message_id = field(&one(&facade, MESSAGE, "emailMessageId", &first), "messageId");
    assert_eq!(changes(&facade, &account), 1);
    sync(&account);
    let copies = imap.messages("Sent Mail");
    let copy = copies
        .iter()
        .find(|(_, _, mid)| *mid == message_id)
        .expect("appended copy");
    assert!(copy.1.contains(&"\\Seen".to_string()), "{copy:?}");
    let row = one(&facade, MESSAGE, "emailMessageId", &first);
    assert_eq!(field(&row, "remoteFolderId"), folder_id(&facade, &account, "EfkSent"));
    assert_eq!(field(&row, "remoteUid"), copy.0.to_string());

    // the server kept its own copy: it is found, not appended
    let second = send("Kept");
    let kept_id = field(&one(&facade, MESSAGE, "emailMessageId", &second), "messageId");
    let kept_uid = imap.add(
        "Sent Mail",
        &mail(&kept_id, "alice@example.com", "bob@example.com", "Kept", "", "Hello"),
        &["\\Seen"],
    );
    imap.clear_log();
    sync(&account);
    assert!(
        !imap.commands().iter().any(|command| command.starts_with("APPEND")),
        "{:?}",
        imap.commands()
    );
    assert_eq!(
        field(&one(&facade, MESSAGE, "emailMessageId", &second), "remoteUid"),
        kept_uid.to_string()
    );
    assert_eq!(imap.messages("Sent Mail").len(), 2);
}

/// A sync that stops at email-client sync-batch with mail left is due again at the job's next run, before its interval, until
/// the backlog is in; then the interval holds again.
#[test]
fn the_job_goes_on_with_a_backlog_at_its_next_run() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup_with(r#"<moqui-conf><email-client sync-batch="1"/></moqui-conf>"#);
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS"]);
    for (id, subject) in [("<b1@example.com>", "One"), ("<b2@example.com>", "Two")] {
        imap.add(
            "INBOX",
            &mail(id, "x@example.com", "alice@example.com", subject, "", "b"),
            &[],
        );
    }
    let account = imap_account(imap.port, "app-password");
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let first = ok("sync#DueEmailAccounts", &[]);
    assert_eq!(out_number(&first, "syncedCount"), 1, "{first:?}");
    let in_first = [
        by_message_id(&facade, &account, "<b1@example.com>").is_some(),
        by_message_id(&facade, &account, "<b2@example.com>").is_some(),
    ];
    assert_eq!(
        in_first.iter().filter(|found| **found).count(),
        1,
        "one message a session"
    );
    // the interval has not passed, but the account has a backlog
    let second = ok("sync#DueEmailAccounts", &[]);
    assert_eq!(out_number(&second, "syncedCount"), 1, "{second:?}");
    assert!(by_message_id(&facade, &account, "<b1@example.com>").is_some());
    assert!(by_message_id(&facade, &account, "<b2@example.com>").is_some());
    // all in: a sync that fetched everything leaves no backlog, so the interval holds (a capped session is the one
    // that found more than the batch; the second found one, so it is not)
    let third = ok("sync#DueEmailAccounts", &[]);
    assert_eq!(out_number(&third, "syncedCount"), 0, "{third:?}");
}

/// The job syncs what is due and leaves a paused account; a sync holding the lock makes another one busy.
#[test]
fn the_job_syncs_due_accounts_and_the_lock_keeps_one_at_a_time() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS"]);
    imap.add(
        "INBOX",
        &mail(
            "<job@example.com>",
            "x@example.com",
            "alice@example.com",
            "Job",
            "",
            "j",
        ),
        &[],
    );
    let due = imap_account(imap.port, "app-password");
    let paused = imap_account(imap.port, "app-password");
    ok(
        "update#EmailAccount",
        &[("emailAccountId", text(&paused)), ("statusId", text("EasPaused"))],
    );
    {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let job = ok("sync#DueEmailAccounts", &[]);
        assert_eq!(out_number(&job, "syncedCount"), 1, "{job:?}");
        let again = ok("sync#DueEmailAccounts", &[]);
        assert_eq!(out_number(&again, "syncedCount"), 0, "{again:?}");
    }
    assert!(by_message_id(&facade, &due, "<job@example.com>").is_some());
    assert!(field(
        &one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &paused),
        "lastSyncDate"
    )
    .is_empty());
    let paused_out = call("sync#EmailAccount", &[("emailAccountId", text(&paused))]);
    assert_eq!(out_text(&paused_out, "outcome"), "paused");
    let _ = errors();
    // another server holds the lock
    update(
        &facade,
        "moqui.basic.email.EmailAccount",
        ("emailAccountId", &due),
        &[
            ("syncLockedUntil", stamp(now() + 600_000)),
            ("syncLockedBy", text("other-server")),
        ],
    );
    let busy = call("sync#EmailAccount", &[("emailAccountId", text(&due))]);
    assert_eq!(out_text(&busy, "outcome"), "busy");
    let _ = errors();
    // a lock left by a server that stopped ends
    update(
        &facade,
        "moqui.basic.email.EmailAccount",
        ("emailAccountId", &due),
        &[("syncLockedUntil", stamp(now() - 1000))],
    );
    sync(&due);
    // a stranger reaches nothing
    user(&facade, "eve");
    login("eve");
    let _ = call("sync#EmailAccount", &[("emailAccountId", text(&due))]);
    assert!(errors().contains("not found"));
}

fn rule(name: &str, conditions: Value, actions: Value, more: &[(&str, Value)]) -> String {
    let mut parameters = vec![
        ("ruleName", text(name)),
        ("conditionList", conditions),
        ("actionList", actions),
    ];
    parameters.extend(more.iter().cloned());
    out_text(&ok("create#EmailRule", &parameters), "emailRuleId")
}

fn conditions(items: &[&[(&str, &str)]]) -> Value {
    Value::List(items.iter().map(|pairs| map(pairs)).collect())
}

/// Rules on the new Inbox mail: each condition kind and operator, all and any, each action kind, stop processing,
/// matchedCount; their server side goes at the end of the session. On demand over a folder, a rule that cannot be read
/// is reported and the others still run. Block sender and report spam.
#[test]
fn rules_conditions_actions_block_and_spam() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS", "MOVE"]);
    let account = imap_account(imap.port, "app-password");
    ok(
        "create#EmailFolder",
        &[("emailAccountId", text(&account)), ("folderName", text("Newsletters"))],
    );
    sync(&account);
    let news = rule(
        "News",
        conditions(&[&[("field", "from"), ("operator", "contains"), ("value", "NEWS@")]]),
        Value::List(vec![
            map(&[("action", "moveToFolder"), ("value", "Newsletters")]),
            map(&[("action", "addLabel"), ("value", "News")]),
            map(&[("action", "markRead")]),
        ]),
        &[],
    );
    rule(
        "Alerts",
        conditions(&[&[("field", "subject"), ("operator", "matches"), ("value", "^\\[ALERT\\]")]]),
        Value::List(vec![map(&[("action", "flag")])]),
        &[],
    );
    rule(
        "Urgent",
        conditions(&[&[("field", "X-Priority"), ("operator", "is"), ("value", "1")]]),
        Value::List(vec![map(&[("action", "addLabel"), ("value", "Urgent")])]),
        &[],
    );
    rule(
        "Files",
        conditions(&[&[("field", "hasAttachment"), ("value", "true")]]),
        Value::List(vec![map(&[("action", "addLabel"), ("value", "Files")])]),
        &[],
    );
    rule(
        "Big",
        conditions(&[&[("field", "size"), ("operator", "greater"), ("value", "1K")]]),
        Value::List(vec![map(&[("action", "addLabel"), ("value", "Big")])]),
        &[],
    );
    rule(
        "Lists",
        conditions(&[&[
            ("field", "listId"),
            ("operator", "contains"),
            ("value", "list.example.com"),
        ]]),
        Value::List(vec![map(&[("action", "markSpam")])]),
        &[],
    );
    rule(
        "Team",
        conditions(&[&[("field", "to"), ("operator", "startsWith"), ("value", "team")]]),
        Value::List(vec![map(&[("action", "delete")])]),
        &[("stopProcessing", text("Y"))],
    );
    rule(
        "Never",
        conditions(&[&[("field", "to"), ("operator", "contains"), ("value", "team")]]),
        Value::List(vec![map(&[("action", "addLabel"), ("value", "Never")])]),
        &[],
    );
    rule(
        "Dave",
        conditions(&[&[("field", "from"), ("operator", "is"), ("value", "dave@example.com")]]),
        Value::List(vec![map(&[("action", "forwardTo"), ("value", "keep@example.org")])]),
        &[],
    );
    rule(
        "Any",
        conditions(&[
            &[("field", "subject"), ("operator", "contains"), ("value", "zzz")],
            &[("field", "cc"), ("operator", "contains"), ("value", "carol@")],
            &[
                ("field", "body"),
                ("operator", "contains"),
                ("value", "never in a body"),
            ],
        ]),
        Value::List(vec![map(&[("action", "addLabel"), ("value", "AnyHit")])]),
        &[("matchAllEnumId", text("ErmAny"))],
    );
    let refused_rule = call(
        "create#EmailRule",
        &[
            ("ruleName", text("Bad")),
            (
                "conditionList",
                conditions(&[&[("field", "subject"), ("operator", "matches"), ("value", "([")]]),
            ),
            ("actionList", Value::List(vec![map(&[("action", "flag")])])),
        ],
    );
    assert!(refused_rule.get("emailRuleId").is_none_or(Value::is_null));
    assert!(errors().contains("not a regular expression"));

    imap.add(
        "INBOX",
        &mail(
            "<n1@example.com>",
            "Shop <news@shop.example>",
            "alice@example.com",
            "Sale",
            "",
            "sale",
        ),
        &[],
    );
    imap.add(
        "INBOX",
        &mail(
            "<n2@example.com>",
            "ops@example.com",
            "alice@example.com",
            "[ALERT] disk",
            "X-Priority: 1\n",
            "full",
        ),
        &[],
    );
    imap.add(
        "INBOX",
        &FILES_MAIL
            .replace("<files@example.com>", "<n3@example.com>")
            .replace("and the report", &"x".repeat(1500)),
        &[],
    );
    imap.add(
        "INBOX",
        &mail(
            "<n4@example.com>",
            "list@example.com",
            "alice@example.com",
            "Weekly",
            "List-Id: Weekly <weekly.list.example.com>\n",
            "w",
        ),
        &[],
    );
    imap.add(
        "INBOX",
        &mail(
            "<n5@example.com>",
            "x@example.com",
            "Team <team@example.com>",
            "All hands",
            "",
            "t",
        ),
        &[],
    );
    imap.add(
        "INBOX",
        &mail(
            "<n6@example.com>",
            "dave@example.com",
            "alice@example.com",
            "Notes",
            "",
            "keep this",
        ),
        &[],
    );
    imap.add(
        "INBOX",
        &mail(
            "<n7@example.com>",
            "x@example.com",
            "alice@example.com",
            "Hi",
            "Cc: carol@example.com\n",
            "hi",
        ),
        &[],
    );
    let sink = super::SinkGuard::install();
    let out = sync(&account);
    assert_eq!(out_number(&out, "newCount"), 7, "{out:?}");
    let message = |n: u32| by_message_id(&facade, &account, &format!("<n{n}@example.com>")).unwrap();
    let id = |n: u32| field(&message(n), "emailMessageId");
    let newsletters = field(
        &folder_by_name(&facade, &account, "Newsletters").unwrap(),
        "emailFolderId",
    );
    assert_eq!(field(&message(1), "emailFolderId"), newsletters);
    assert_eq!(field(&message(1), "isRead"), "Y");
    assert_eq!(label_names(&facade, &id(1)), vec!["News".to_string()]);
    assert_eq!(field(&message(2), "isFlagged"), "Y");
    assert_eq!(label_names(&facade, &id(2)), vec!["Urgent".to_string()]);
    assert_eq!(
        label_names(&facade, &id(3)),
        vec!["Big".to_string(), "Files".to_string()]
    );
    assert_eq!(
        field(&message(4), "emailFolderId"),
        folder_id(&facade, &account, "EfkJunk")
    );
    assert_eq!(
        field(&message(5), "emailFolderId"),
        folder_id(&facade, &account, "EfkTrash")
    );
    assert!(label_names(&facade, &id(5)).is_empty());
    assert_eq!(label_names(&facade, &id(7)), vec!["AnyHit".to_string()]);
    let forwarded = sink.sent();
    assert_eq!(forwarded.len(), 1);
    assert_eq!(forwarded[0].to, vec!["keep@example.org".to_string()]);
    assert_eq!(forwarded[0].subject, "Fwd: Notes");
    assert!(forwarded[0].body_text.contains("keep this"));
    assert_eq!(
        field(
            &one(&facade, "moqui.basic.email.EmailRule", "emailRuleId", &news),
            "matchedCount"
        ),
        "1"
    );
    // their server side went at the end of the session
    assert_eq!(changes(&facade, &account), 0);
    assert!(
        imap.messages("Newsletters")
            .iter()
            .any(|(_, _, mid)| mid == "<n1@example.com>"),
        "{:?}",
        imap.folder_names()
    );
    let junk = imap.messages("Junk");
    let spam = junk
        .iter()
        .find(|(_, _, mid)| mid == "<n4@example.com>")
        .expect("in Junk");
    assert!(spam.1.contains(&"$Junk".to_string()), "{spam:?}");
    assert!(imap
        .messages("Trash")
        .iter()
        .any(|(_, _, mid)| mid == "<n5@example.com>"));
    let news_copy = imap.messages("Newsletters");
    assert!(
        news_copy[0].1.contains(&"\\Seen".to_string()) && news_copy[0].1.contains(&"News".to_string()),
        "{news_copy:?}"
    );

    // on demand over a folder, with one rule that cannot be read
    row(
        &facade,
        "moqui.basic.email.EmailRule",
        &[
            ("emailRuleId", text("BROKEN")),
            ("ownerUserId", text("alice")),
            ("emailAccountId", text(&account)),
            ("ruleName", text("Broken")),
            ("sequenceNum", Value::I64(1)),
            ("enabled", text("Y")),
            ("conditionsJson", text("not json")),
            ("actionsJson", text("[]")),
        ],
    );
    let inbox = folder_id(&facade, &account, "EfkInbox");
    let ran = ok("run#EmailRules", &[("emailFolderId", text(&inbox))]);
    assert!(out_text(&ran, "ruleErrorList").contains("Broken"), "{ran:?}");
    assert!(out_number(&ran, "matchedCount") >= 2, "{ran:?}");

    // block the sender of a message: a rule, and the message to Junk
    let blocked = ok("block#Sender", &[("emailMessageId", text(&id(7)))]);
    assert_eq!(out_number(&blocked, "movedCount"), 1);
    assert_eq!(
        field(&message(7), "emailFolderId"),
        folder_id(&facade, &account, "EfkJunk")
    );
    let rule_row = one(
        &facade,
        "moqui.basic.email.EmailRule",
        "emailRuleId",
        &out_text(&blocked, "emailRuleId"),
    );
    assert!(field(&rule_row, "conditionsJson").contains("x@example.com"));
    // not spam after all
    ok(
        "report#Spam",
        &[("emailMessageId", text(&id(4))), ("notSpam", Value::Bool(true))],
    );
    assert_eq!(field(&message(4), "emailFolderId"), inbox);
    sync(&account);
    let back = imap.messages("INBOX");
    let unspam = back
        .iter()
        .find(|(_, _, mid)| mid == "<n4@example.com>")
        .expect("back in INBOX");
    assert!(
        unspam.1.contains(&"$NotJunk".to_string()) && !unspam.1.contains(&"$Junk".to_string()),
        "{unspam:?}"
    );

    // the rule services: list, reorder, update, delete; another user may not change them
    let listed = ok("find#EmailRules", &[]);
    let ids: Vec<String> = match listed.get("ruleList") {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::Map(map) => map.get("emailRuleId").map(to_plain_string),
                _ => None,
            })
            .collect(),
        other => panic!("{other:?}"),
    };
    assert!(ids.len() >= 11, "{ids:?}");
    let mut reversed = ids.clone();
    reversed.reverse();
    ok(
        "reorder#EmailRules",
        &[(
            "emailRuleIdList",
            Value::List(reversed.iter().map(Value::string).collect()),
        )],
    );
    assert_eq!(
        field(
            &one(&facade, "moqui.basic.email.EmailRule", "emailRuleId", &reversed[0]),
            "sequenceNum"
        ),
        "10"
    );
    ok(
        "update#EmailRule",
        &[("emailRuleId", text(&news)), ("enabled", text("N"))],
    );
    user(&facade, "eve");
    login("eve");
    let _ = call("delete#EmailRule", &[("emailRuleId", text(&news))]);
    assert!(errors().contains("may not delete"));
    login("alice");
    ok("delete#EmailRule", &[("emailRuleId", text(&news))]);
    assert!(rows(&facade, "moqui.basic.email.EmailRule", &[("emailRuleId", &news)]).is_empty());
}

/// Discovery from a local autoconfig file, then from SRV records of a local DNS server, then the guesses.
#[test]
fn discover_from_autoconfig_then_srv_then_guesses() {
    let _lock = crate::user::session::cookie_property_lock();
    let config = r#"<?xml version="1.0"?><clientConfig version="1.1"><emailProvider id="example.test">
        <domain>example.test</domain><displayName>Example Test Mail</displayName>
        <incomingServer type="imap"><hostname>imap.example.test</hostname><port>993</port><socketType>SSL</socketType>
            <username>%EMAILADDRESS%</username><authentication>password-cleartext</authentication></incomingServer>
        <outgoingServer type="smtp"><hostname>smtp.example.test</hostname><port>587</port><socketType>STARTTLS</socketType>
            <username>%EMAILADDRESS%</username><authentication>password-cleartext</authentication></outgoingServer>
        </emailProvider></clientConfig>"#;
    let mut pages = HashMap::new();
    pages.insert("/example.test/config-v1.1.xml".to_string(), config.to_string());
    let http = fake_http(pages);
    let mut records = HashMap::new();
    records.insert(
        "_imaps._tcp.srv.test".to_string(),
        (10, 5, 993, "mail.srv.test".to_string()),
    );
    records.insert(
        "_submission._tcp.srv.test".to_string(),
        (10, 5, 587, "smtp.srv.test".to_string()),
    );
    let dns = fake_dns(records);
    let (ctx, facade) = setup_with(&format!(
        r#"<moqui-conf><email-client autoconfig-urls="http://127.0.0.1:{http}/{{domain}}/config-v1.1.xml"
            dns-server="127.0.0.1:{dns}"/></moqui-conf>"#
    ));
    let _entered = ctx.enter();
    install();
    alice(&facade);

    let found = ok("discover#EmailServer", &[("emailAddress", text("ann@example.test"))]);
    assert_eq!(out_text(&found, "source"), "autoconfig");
    assert_eq!(out_text(&found, "displayName"), "Example Test Mail");
    assert_eq!(out_text(&found, "storeHost"), "imap.example.test");
    assert_eq!(out_text(&found, "storePort"), "993");
    assert_eq!(out_text(&found, "storeProtocol"), "imaps");
    assert_eq!(out_text(&found, "smtpHost"), "smtp.example.test");
    assert_eq!(out_text(&found, "smtpStartTls"), "Y");
    assert_eq!(out_text(&found, "mailUsername"), "ann@example.test");

    let started = Instant::now();
    let srv = ok("discover#EmailServer", &[("emailAddress", text("bo@srv.test"))]);
    assert_eq!(out_text(&srv, "source"), "srv", "{srv:?}");
    assert_eq!(out_text(&srv, "storeHost"), "mail.srv.test");
    assert_eq!(out_text(&srv, "storeProtocol"), "imaps");
    assert_eq!(out_text(&srv, "smtpHost"), "smtp.srv.test");
    assert_eq!(out_text(&srv, "smtpPort"), "587");

    let guessed = ok("discover#EmailServer", &[("emailAddress", text("cy@none.test"))]);
    assert_eq!(out_text(&guessed, "source"), "guess", "{guessed:?}");
    assert_eq!(out_text(&guessed, "storeHost"), "imap.none.test");
    assert_eq!(out_text(&guessed, "smtpHost"), "smtp.none.test");
    assert!(started.elapsed() < Duration::from_secs(12), "{:?}", started.elapsed());
    let _ = call("discover#EmailServer", &[("emailAddress", text("not-an-address"))]);
    assert!(errors().contains("not an email address"));
}

const MBOX: &str = "From news@example.com Mon Oct 19 10:00:00 2026
Message-ID: <i1@example.com>
From: news@example.com
To: alice@example.com
Subject: First
Date: Mon, 19 Oct 2026 10:00:00 +0000
Status: RO

First body
>From the archive
From two@example.com Mon Oct 19 11:00:00 2026
Message-ID: <i2@example.com>
From: two@example.com
To: alice@example.com
Subject: Second
Date: Mon, 19 Oct 2026 11:00:00 +0000
X-Mozilla-Status: 0000

Second body
From broken@example.com Mon Oct 19 12:00:00 2026
this has no headers at all and is not mail
From again@example.com Mon Oct 19 13:00:00 2026
Message-ID: <i1@example.com>
From: news@example.com
Subject: First again

Duplicate
";

/// An mbox and an .eml into a folder: each message once, a duplicate and a broken one counted, the read state from the
/// mbox's headers; a user who may only read the account may not import.
#[test]
fn import_mbox_and_eml_into_a_folder() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let account = out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("emailAddress", text("alice@example.com")),
                ("accountName", text("Local")),
            ],
        ),
        "emailAccountId",
    );
    let archive = folder_id(&facade, &account, "EfkArchive");
    let out = ok(
        "import#EmailMessages",
        &[("emailFolderId", text(&archive)), ("messageText", text(MBOX))],
    );
    assert_eq!(out_number(&out, "importedCount"), 2, "{out:?}");
    assert_eq!(out_number(&out, "duplicateCount"), 1, "{out:?}");
    assert_eq!(out_number(&out, "failedCount"), 1, "{out:?}");
    let first = by_message_id(&facade, &account, "<i1@example.com>").unwrap();
    assert_eq!(field(&first, "emailFolderId"), archive);
    assert_eq!(field(&first, "isRead"), "Y");
    assert!(
        field(&first, "bodyText").contains("From the archive"),
        "{}",
        field(&first, "bodyText")
    );
    assert!(!field(&first, "archivedDate").is_empty());
    assert!(field(&first, "remoteFolderId").is_empty());
    assert_eq!(
        field(&by_message_id(&facade, &account, "<i2@example.com>").unwrap(), "isRead"),
        "N"
    );
    // an .eml at a resource location
    let location = "dbresource://test/import/one.eml";
    ctx.factory()
        .resource()
        .write_bytes(
            location,
            mail(
                "<i3@example.com>",
                "x@example.com",
                "alice@example.com",
                "Eml",
                "",
                "eml",
            )
            .as_bytes(),
        )
        .unwrap();
    let eml = ok(
        "import#EmailMessages",
        &[("emailFolderId", text(&archive)), ("location", text(location))],
    );
    assert_eq!(out_number(&eml, "importedCount"), 1, "{eml:?}");
    // read permission only
    user(&facade, "bob");
    ok(
        "add#EmailAccountUser",
        &[
            ("emailAccountId", text(&account)),
            ("userId", text("bob")),
            ("permissionEnumId", text("EapRead")),
        ],
    );
    login("bob");
    let _ = call(
        "import#EmailMessages",
        &[("emailFolderId", text(&archive)), ("messageText", text(MBOX))],
    );
    assert!(errors().contains("may not change"));
}

/// An IDLE worker: new mail in the Inbox is stored without a sync being asked for, and the worker stops on demand.
#[test]
fn an_idle_worker_syncs_new_inbox_mail_and_stops() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["IDLE", "UIDPLUS"]);
    let account = imap_account(imap.port, "app-password");
    sync(&account);
    super::idle::start_worker_now(ctx.factory(), &account);
    let wait_for = |what: &str, check: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !check() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    wait_for("the IDLE", &|| imap.commands().iter().any(|command| command == "IDLE"));
    imap.add(
        "INBOX",
        &mail(
            "<idle@example.com>",
            "x@example.com",
            "alice@example.com",
            "Idle",
            "",
            "now",
        ),
        &[],
    );
    wait_for("the message", &|| {
        by_message_id(&facade, &account, "<idle@example.com>").is_some()
    });
    assert_eq!(super::idle::worker_accounts(ctx.factory()), vec![account.clone()]);
    let stopping = Instant::now();
    super::idle::stop_all(ctx.factory());
    assert!(stopping.elapsed() < Duration::from_secs(5), "{:?}", stopping.elapsed());
    assert!(super::idle::worker_accounts(ctx.factory()).is_empty());
}

/// The parse of a message: Message-ID, threading headers, addresses, the bodies and the snippet.
#[test]
fn parse_message_reads_headers_bodies_and_files() {
    let parsed = super::store::parse_message(FILES_MAIL.replace('\n', "\r\n").as_bytes()).unwrap();
    assert_eq!(parsed.message_id, "<files@example.com>");
    assert_eq!(parsed.from_address, "carol@example.com");
    assert_eq!(parsed.from_name, "Carol");
    assert_eq!(parsed.to, "alice@example.com");
    assert!(parsed.has_attachment);
    assert_eq!(parsed.files.len(), 2);
    assert!(parsed.headers.starts_with("Message-ID: <files@example.com>"));
    let reply = super::store::parse_message(
        mail(
            "<r@x>",
            "a@x",
            "b@x",
            "Re",
            "In-Reply-To: <p@x>\nReferences: <o@x> <p@x>\nList-Id: <l.x>\n",
            "Hello   there\n\nfriend",
        )
        .as_bytes(),
    )
    .unwrap();
    assert_eq!(reply.in_reply_to, "<p@x>");
    assert_eq!(reply.references, "<o@x> <p@x>");
    assert_eq!(reply.snippet, "Hello there friend");
    assert_eq!(reply.header("list-id"), "<l.x>");
    assert!(super::store::parse_message(b"\x00\x01 not mail").is_err());
    assert!(super::store::parse_message(b"   \r\n").is_err());
}

/// A draft saved here goes to the server's Drafts at the next sync (`\Draft`, with a Message-ID the sync then knows it
/// by), a save after replaces that copy, and the send takes it out of Drafts: the copy in Sent has the same Message-ID.
/// A draft discarded before its change went leaves nothing there.
#[test]
fn drafts_go_to_the_servers_drafts_are_replaced_and_leave_when_sent() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS", "MOVE"]);
    let account = imap_account(imap.port, "app-password");
    sync(&account);
    let save = |more: &[(&str, Value)]| -> String {
        let mut parameters = vec![
            ("emailAccountId", text(&account)),
            ("toAddresses", text("bob@example.com")),
        ];
        parameters.extend_from_slice(more);
        out_text(&ok("save#Draft", &parameters), "emailMessageId")
    };
    let draft = save(&[("subject", text("Plan")), ("body", text("<p>First</p>"))]);
    // autosaves before the push make one change
    save(&[("emailMessageId", text(&draft)), ("body", text("<p>First, again</p>"))]);
    assert_eq!(changes(&facade, &account), 1);
    sync(&account);
    let drafts = imap.messages("Drafts");
    assert_eq!(drafts.len(), 1, "{drafts:?}");
    assert!(drafts[0].1.contains(&"\\Draft".to_string()), "{drafts:?}");
    assert!(drafts[0].1.contains(&"\\Seen".to_string()), "{drafts:?}");
    let row = one(&facade, MESSAGE, "emailMessageId", &draft);
    let message_id = field(&row, "messageId");
    assert!(!message_id.is_empty());
    assert_eq!(drafts[0].2, message_id);
    assert_eq!(field(&row, "remoteFolderId"), folder_id(&facade, &account, "EfkDrafts"));
    assert_eq!(field(&row, "remoteUid"), drafts[0].0.to_string());
    assert!(imap.raw_messages("Drafts")[0].1.contains("First, again"));

    // a save after replaces the copy; the sync keeps one row
    save(&[("emailMessageId", text(&draft)), ("subject", text("Plan v2"))]);
    sync(&account);
    let drafts = imap.raw_messages("Drafts");
    assert_eq!(drafts.len(), 1, "{drafts:?}");
    assert!(drafts[0].1.contains("Subject: Plan v2"), "{}", drafts[0].1);
    assert_eq!(
        rows(
            &facade,
            MESSAGE,
            &[("emailAccountId", &account), ("messageId", &message_id)]
        )
        .len(),
        1
    );
    sync(&account);
    assert_eq!(
        rows(
            &facade,
            MESSAGE,
            &[("emailAccountId", &account), ("messageId", &message_id)]
        )
        .len(),
        1
    );

    // sent: out of Drafts there, and Sent has it under the same Message-ID
    let _sink = super::SinkGuard::install();
    let sent = ok("send#Draft", &[("emailMessageId", text(&draft))]);
    assert_eq!(sent.get("sent"), Some(&Value::Bool(true)), "{sent:?}");
    assert_eq!(out_text(&sent, "messageId"), message_id);
    sync(&account);
    assert!(imap.messages("Drafts").is_empty(), "{:?}", imap.messages("Drafts"));
    assert!(
        imap.messages("Sent Mail").iter().any(|(_, _, mid)| *mid == message_id),
        "{:?}",
        imap.messages("Sent Mail")
    );

    // discarded before its change went: nothing on the server
    let gone = save(&[("subject", text("Never mind"))]);
    ok("trash#Messages", &[("emailMessageId", text(&gone))]);
    imap.clear_log();
    sync(&account);
    assert!(imap.messages("Drafts").is_empty());
    assert!(
        !imap.commands().iter().any(|command| command.starts_with("APPEND")),
        "{:?}",
        imap.commands()
    );
}

/// New Inbox mail at a sync: a reply in a snoozed thread brings it back, and the account's automatic reply goes to
/// its sender once (Auto-Submitted, appended to Sent), not again for the reply.
#[test]
fn new_mail_at_a_sync_wakes_its_thread_and_gets_the_automatic_reply() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS", "MOVE"]);
    let account = imap_account(imap.port, "app-password");
    sync(&account);
    ok(
        "store#EmailOutOfOffice",
        &[
            ("emailAccountId", text(&account)),
            ("fromDate", stamp(now() - 86_400_000)),
            ("bodyHtml", text("<p>Back on Monday</p>")),
        ],
    );
    let sink = super::SinkGuard::install();
    imap.add(
        "INBOX",
        &mail(
            "<root@x.example.org>",
            "Ada <ada@x.example.org>",
            "alice@example.com",
            "Plan",
            "",
            "First",
        ),
        &[],
    );
    sync(&account);
    let sent = sink.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].to, ["ada@x.example.org"]);
    assert_eq!(sent[0].subject, "Automatic reply: Plan");
    assert!(
        sent[0].mime.contains("Auto-Submitted: auto-replied"),
        "{}",
        sent[0].mime
    );
    let copies = imap.raw_messages("Sent Mail");
    assert_eq!(copies.len(), 1, "the reply is appended to Sent");
    assert!(copies[0].1.contains("Auto-Submitted: auto-replied"), "{}", copies[0].1);

    let root = by_message_id(&facade, &account, "<root@x.example.org>").expect("the root");
    let root_id = field(&root, "emailMessageId");
    ok(
        "snooze#Messages",
        &[
            ("rootEmailMessageId", text(&root_id)),
            ("snoozedThruDate", stamp(now() + 86_400_000)),
        ],
    );
    assert!(!field(&one(&facade, MESSAGE, "emailMessageId", &root_id), "snoozedThruDate").is_empty());
    imap.add(
        "INBOX",
        &mail(
            "<reply@x.example.org>",
            "Ada <ada@x.example.org>",
            "alice@example.com",
            "Re: Plan",
            "In-Reply-To: <root@x.example.org>\n",
            "Second",
        ),
        &[],
    );
    sync(&account);
    assert_eq!(
        field(&one(&facade, MESSAGE, "emailMessageId", &root_id), "snoozedThruDate"),
        ""
    );
    let reply = by_message_id(&facade, &account, "<reply@x.example.org>").expect("the reply");
    assert_eq!(field(&reply, "rootEmailMessageId"), root_id);
    assert_eq!(sink.sent().len(), 1, "once per sender");
}

/// The email-ECA rules (`*.emecas.xml`, as Mantle's List-Unsubscribe mailto rule) run once on each new message a sync
/// brings into the Inbox, IMAP and POP3, with `emailAccountId` and `emailMessageId` of its row: not on the first sync
/// (the mailbox's history), not on mail that arrived long before the last sync (a backlog), not again at the next
/// sync, and not on a copy of a message the account has; a rule that fails is logged on its own and the others run.
#[test]
fn email_eca_rules_run_once_on_each_new_message_a_sync_brings_in() {
    const DAY: i64 = 86_400_000;
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    crate::service::eca_rules::install(
        ctx.factory(),
        crate::service::eca_rules::from_xml(
            "",
            r#"<emecas>
            <emeca rule-name="Fails"><condition><expression>true</expression></condition>
                <actions><script>throw new Error('rule boom')</script></actions></emeca>
            <emeca rule-name="ListUnsubscribeMailto">
                <condition><expression>/^unsubscribe /i.test(String((fields &amp;&amp; fields.subject) || ''))</expression></condition>
                <actions><script>ec.message.addMessage('eca ' + fields.subject + ' ' + emailMessageId + ' ' + emailAccountId + ' ' + headers['message-id'] + ' ' + emailServerId)</script></actions>
            </emeca>
        </emecas>"#,
        ),
    );
    let ran = || -> Vec<String> {
        ExecutionContext::with_current(|ec| ec.message().messages().to_vec())
            .unwrap_or_default()
            .into_iter()
            .filter(|text| text.starts_with("eca "))
            .collect()
    };
    let dated = |message_id: &str, subject: &str, millis: i64| {
        let date = jiff::Timestamp::from_millisecond(millis)
            .unwrap()
            .to_zoned(jiff::tz::TimeZone::UTC)
            .strftime("%a, %d %b %Y %H:%M:%S +0000")
            .to_string();
        format!(
            "Message-ID: {message_id}\nFrom: x@example.com\nTo: alice@example.com\nSubject: {subject}\nDate: {date}\n\nbody\n"
        )
    };

    // IMAP: the first sync is history
    let imap = FakeImap::start(&[]);
    imap.add_folder("Lists", "\\HasNoChildren", true);
    imap.add("INBOX", &dated("<old@example.com>", "unsubscribe OLD", now()), &[]);
    let account = imap_account(imap.port, "app-password");
    let server = field(
        &one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &account),
        "emailServerId",
    );
    sync(&account);
    assert!(ran().is_empty(), "{:?}", ran());
    // new mail: once on each new Inbox message, with its row; a failing rule does not stop the other or the sync
    imap.add("INBOX", &dated("<new1@example.com>", "unsubscribe TOKEN1", now()), &[]);
    imap.add("INBOX", &dated("<plain@example.com>", "Hello", now()), &[]);
    imap.add_dated(
        "INBOX",
        &dated("<late@example.com>", "unsubscribe LATE", now() - 10 * DAY),
        &[],
        now() - 10 * DAY,
    );
    imap.add(
        "Lists",
        &dated("<listed@example.com>", "unsubscribe LISTED", now()),
        &[],
    );
    let out = sync(&account);
    assert_eq!(out_number(&out, "newCount"), 4, "{out:?}");
    let new1 = by_message_id(&facade, &account, "<new1@example.com>").unwrap();
    assert_eq!(
        ran(),
        [format!(
            "eca unsubscribe TOKEN1 {} {account} <new1@example.com> {server}",
            field(&new1, "emailMessageId")
        )]
    );
    // the next sync, and a copy into the Inbox of a message the account has: nothing runs again
    sync(&account);
    imap.add(
        "INBOX",
        &dated("<listed@example.com>", "unsubscribe LISTED", now()),
        &[],
    );
    sync(&account);
    assert_eq!(ran().len(), 1, "{:?}", ran());

    // POP3: the same, the newest first and by the Date header
    let pop = FakePop::start();
    pop.add("p-old", &dated("<pop-old@example.com>", "unsubscribe POPOLD", now()));
    let pop_account = out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("emailAddress", text("alice@example.com")),
                ("storeHost", text("127.0.0.1")),
                ("storePort", text(&pop.port.to_string())),
                ("storeProtocol", text("pop3")),
                ("mailUsername", text("alice")),
                ("mailPassword", text("pop-password")),
            ],
        ),
        "emailAccountId",
    );
    sync(&pop_account);
    assert_eq!(ran().len(), 1);
    pop.add("p-new", &dated("<pop-new@example.com>", "unsubscribe POPNEW", now()));
    pop.add(
        "p-late",
        &dated("<pop-late@example.com>", "unsubscribe POPLATE", now() - 10 * DAY),
    );
    let out = sync(&pop_account);
    assert_eq!(out_number(&out, "newCount"), 2, "{out:?}");
    sync(&pop_account);
    let lines = ran();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[1].starts_with("eca unsubscribe POPNEW ") && lines[1].contains(&pop_account),
        "{lines:?}"
    );
}
