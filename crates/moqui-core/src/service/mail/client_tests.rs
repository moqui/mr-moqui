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

//! `org.moqui.impl.EmailClientServices`, the mail client: who reaches an account and with what permission, the
//! Internal account, the message list with its views, threads and date cursor, filing (flags, folders, labels, snooze,
//! trash, archive), drafts and sends through the test sink (now, scheduled, held for undo, failed and retried), the
//! jobs, the unread count, and the server test against scripted local SMTP and IMAP servers.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

use indexmap::IndexMap;
use moqui_lib::object::{basic_convert, to_plain_string, Value};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};

const SERVICES: &str = "org.moqui.impl.EmailClientServices";
const MESSAGE: &str = "moqui.basic.email.EmailMessage";
const DAY: i64 = 86_400_000;

fn setup() -> (ExecutionContext, EntityFacade) {
    setup_with("")
}

/// [`setup`] with the conf text `conf`.
fn setup_with(conf: &str) -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context_in(
        conf,
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

fn install() {
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

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn stamp(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
}

fn text(value: &str) -> Value {
    Value::string(value)
}

fn list(items: &[&str]) -> Value {
    Value::List(items.iter().map(|item| Value::string(*item)).collect())
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, Value)]) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value(entity).unwrap();
    for (field, value) in fields {
        row.set(*field, value.clone());
    }
    row.create(facade).unwrap();
}

fn user(facade: &EntityFacade, user_id: &str) {
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

fn login(user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    crate::user::internal_login_user(user_id).unwrap();
}

fn call(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    crate::service::sync(&format!("{SERVICES}.{name}"), &parameters).unwrap_or_else(|err| panic!("{name}: {err}"))
}

/// The errors so far, cleared.
fn errors() -> String {
    ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default()
}

/// `call`, which must leave no error.
fn ok(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let out = call(name, parameters);
    let errors = errors();
    assert!(errors.is_empty(), "{name}: {errors}");
    out
}

/// `call`, which must leave an error containing `expected`.
fn refused(name: &str, parameters: &[(&str, Value)], expected: &str) {
    let _ = call(name, parameters);
    let errors = errors();
    assert!(
        errors.contains(expected),
        "{name}: expected an error with {expected:?}, got {errors:?}"
    );
}

fn out_text(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

fn out_number(out: &BTreeMap<String, Value>, key: &str) -> i64 {
    out.get(key)
        .map(to_plain_string)
        .and_then(|text| text.parse().ok())
        .unwrap_or(-1)
}

fn maps(out: &BTreeMap<String, Value>, key: &str) -> Vec<IndexMap<String, Value>> {
    match out.get(key) {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::Map(map) => Some(map.clone()),
                _ => None,
            })
            .collect(),
        other => panic!("{key} is not a list: {other:?}"),
    }
}

fn field(map: &IndexMap<String, Value>, key: &str) -> String {
    map.get(key).map(to_plain_string).unwrap_or_default()
}

fn thread_ids(out: &BTreeMap<String, Value>) -> Vec<String> {
    maps(out, "threadList")
        .iter()
        .map(|row| field(row, "threadId"))
        .collect()
}

fn message_ids(out: &BTreeMap<String, Value>) -> Vec<String> {
    maps(out, "threadList")
        .iter()
        .map(|row| field(row, "emailMessageId"))
        .collect()
}

fn find_one(facade: &EntityFacade, entity: &str, key: &str, id: &str) -> Option<EntityValue> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade.find(entity).unwrap().eq(key, text(id)).one().unwrap()
}

fn message_field(facade: &EntityFacade, id: &str, name: &str) -> String {
    find_one(facade, MESSAGE, "emailMessageId", id)
        .unwrap_or_else(|| panic!("no message {id}"))
        .get(name)
        .map(to_plain_string)
        .unwrap_or_default()
}

fn set_message(facade: &EntityFacade, id: &str, fields: &[(&str, Value)]) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade
        .find(MESSAGE)
        .unwrap()
        .eq("emailMessageId", text(id))
        .one()
        .unwrap()
        .unwrap();
    for (name, value) in fields {
        row.set(*name, value.clone());
    }
    row.update(facade).unwrap();
}

/// The folder of `kind` in an account.
fn folder(facade: &EntityFacade, account: &str, kind: &str) -> String {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let rows = facade
        .find("moqui.basic.email.EmailFolder")
        .unwrap()
        .eq("emailAccountId", text(account))
        .eq("folderKindEnumId", text(kind))
        .list()
        .unwrap();
    to_plain_string(rows.values()[0].get("emailFolderId").unwrap())
}

/// A stored message of an account: id, folder kind (or "" for none), date offset from `base`, and more fields.
fn message(facade: &EntityFacade, account: &str, id: &str, kind: &str, millis: i64, more: &[(&str, Value)]) {
    let mut fields = vec![
        ("emailMessageId", text(id)),
        ("emailAccountId", text(account)),
        ("statusId", text("ES_RECEIVED")),
        ("subject", text(&format!("Subject {id}"))),
        ("body", text(&format!("<p>Body of {id}</p>"))),
        ("snippet", text(&format!("Body of {id}"))),
        ("fromAddress", text(&format!("{id}@sender.example.com"))),
        ("toAddresses", text("alice@example.com")),
        ("messageDate", stamp(millis)),
        ("isRead", text("N")),
    ];
    if !kind.is_empty() {
        fields.push(("emailFolderId", text(&folder(facade, account, kind))));
    }
    for (name, value) in more {
        fields.retain(|(existing, _)| existing != name);
        fields.push((name, value.clone()));
    }
    row(facade, MESSAGE, &fields);
}

/// Alice's shared account with a server of `smtp_host` and the members Bob (read), Carol (send) and Dave (manage).
fn shared_account(facade: &EntityFacade, smtp_host: &str) -> String {
    for id in ["alice", "bob", "carol", "dave", "eve"] {
        user(facade, id);
    }
    login("alice");
    let created = ok(
        "create#EmailAccount",
        &[
            ("accountKindEnumId", text("EakShared")),
            ("emailAddress", text("team@example.com")),
            ("accountName", text("Team")),
            ("smtpHost", text(smtp_host)),
            ("smtpStartTls", text("N")),
            ("storeHost", text("imap.example.com")),
            ("mailPassword", text("secret")),
        ],
    );
    let account = out_text(&created, "emailAccountId");
    for (member, permission) in [("bob", "EapRead"), ("carol", "EapSend"), ("dave", "EapManage")] {
        ok(
            "add#EmailAccountUser",
            &[
                ("emailAccountId", text(&account)),
                ("userId", text(member)),
                ("permissionEnumId", text(permission)),
            ],
        );
    }
    account
}

/// Who reaches an account, and with what permission; the Internal account is made once.
#[test]
fn account_access_by_owner_member_and_stranger() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    let account = shared_account(&facade, "smtp.example.com");

    // the account: its folders, its default identity and the server it owns
    let stored = find_one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &account).unwrap();
    assert_eq!(to_plain_string(stored.get("ownsEmailServer").unwrap()), "Y");
    assert_eq!(
        to_plain_string(stored.get("recordFilingEnumId").unwrap()),
        "ErfAllMatches"
    );
    for kind in ["EfkInbox", "EfkDrafts", "EfkSent", "EfkArchive", "EfkJunk", "EfkTrash"] {
        folder(&facade, &account, kind);
    }
    refused(
        "create#EmailAccount",
        &[
            ("emailAddress", text("sys@example.com")),
            ("emailServerId", text("SYSTEM")),
        ],
        "only by internal setup",
    );
    refused(
        "create#EmailAccount",
        &[
            ("emailAddress", text("me@example.com")),
            ("accountKindEnumId", text("EakInternal")),
        ],
        "ensure#InternalAccount",
    );

    // the Internal account, made once, listed first
    let first = ok("ensure#InternalAccount", &[]);
    assert_eq!(first.get("created"), Some(&Value::Bool(true)));
    let again = ok("ensure#InternalAccount", &[]);
    assert_eq!(again.get("created"), Some(&Value::Bool(false)));
    assert_eq!(out_text(&first, "emailAccountId"), out_text(&again, "emailAccountId"));
    let mine = ok("get#UserEmailAccounts", &[]);
    let accounts = maps(&mine, "accountList");
    assert_eq!(accounts.len(), 2);
    assert_eq!(field(&accounts[0], "accountKindEnumId"), "EakInternal");
    assert_eq!(
        field(&accounts[0], "emailAccountId"),
        out_text(&first, "emailAccountId")
    );
    assert_eq!(field(&accounts[1], "emailAccountId"), account);
    assert_eq!(field(&accounts[1], "isOwner"), "true");
    assert_eq!(field(&accounts[1], "canManage"), "true");
    assert!(!field(&accounts[1], "emailServerId").is_empty());
    assert!(accounts[1].get("mailPassword").is_none(), "no credentials in the list");
    let Some(Value::List(identities)) = accounts[1].get("identityList") else {
        panic!("{:?}", accounts[1])
    };
    assert_eq!(identities.len(), 1);
    // the same user's Internal account is not made twice by another call
    {
        let _authz = ctx.disable_authz();
        let internals = facade
            .find("moqui.basic.email.EmailAccount")
            .unwrap()
            .eq("ownerUserId", text("alice"))
            .eq("accountKindEnumId", text("EakInternal"))
            .list()
            .unwrap();
        assert_eq!(internals.len(), 1);
    }
    message(&facade, &account, "M1", "EfkInbox", now() - DAY, &[]);

    // a stranger reaches nothing
    login("eve");
    let none = ok("get#EmailAccountAccess", &[("emailAccountId", text(&account))]);
    assert_eq!(out_number(&none, "level"), 0);
    refused("find#Messages", &[("emailAccountId", text(&account))], "not found");
    refused("get#Thread", &[("emailMessageId", text("M1"))], "not found");
    refused(
        "set#MessageFlags",
        &[("emailMessageId", text("M1")), ("read", Value::Bool(true))],
        "not found",
    );
    refused(
        "update#EmailAccount",
        &[("emailAccountId", text(&account)), ("color", text("red"))],
        "not found",
    );
    refused(
        "get#EmailAccountAccess",
        &[("userId", text("alice"))],
        "not yours to see",
    );
    refused("count#UnreadEmail", &[("userId", text("alice"))], "not yours to see");

    // read: sees and marks read, nothing more
    login("bob");
    let inbox = ok("find#Messages", &[("emailAccountId", text(&account))]);
    assert_eq!(thread_ids(&inbox), ["M1"]);
    ok(
        "set#MessageFlags",
        &[("emailMessageId", text("M1")), ("read", Value::Bool(true))],
    );
    assert_eq!(message_field(&facade, "M1", "isRead"), "Y");
    refused(
        "set#MessageFlags",
        &[("emailMessageId", text("M1")), ("flagged", Value::Bool(true))],
        "You may not change the mail",
    );
    refused(
        "save#Draft",
        &[
            ("emailAccountId", text(&account)),
            ("toAddresses", text("x@example.com")),
        ],
        "You may not send",
    );
    refused(
        "archive#Messages",
        &[("emailMessageId", text("M1"))],
        "You may not change the mail",
    );
    refused(
        "update#EmailAccount",
        &[("emailAccountId", text(&account)), ("color", text("red"))],
        "You may not change",
    );
    let bobs = ok("get#UserEmailAccounts", &[]);
    let shared = &maps(&bobs, "accountList")[1];
    assert_eq!(field(shared, "permissionEnumId"), "EapRead");
    assert_eq!(field(shared, "canSend"), "false");

    // send: files and drafts, but does not change the account
    login("carol");
    ok(
        "set#MessageFlags",
        &[("emailMessageId", text("M1")), ("flagged", Value::Bool(true))],
    );
    let draft = ok(
        "save#Draft",
        &[
            ("emailAccountId", text(&account)),
            ("toAddresses", text("x@example.com")),
        ],
    );
    assert!(!out_text(&draft, "emailMessageId").is_empty());
    refused(
        "create#EmailLabel",
        &[("emailAccountId", text(&account)), ("labelName", text("Urgent"))],
        "You may not add labels",
    );
    refused(
        "add#EmailAccountUser",
        &[("emailAccountId", text(&account)), ("userId", text("eve"))],
        "You may not share",
    );

    // manage: changes the account and who shares it
    login("dave");
    ok(
        "create#EmailLabel",
        &[("emailAccountId", text(&account)), ("labelName", text("Urgent"))],
    );
    ok(
        "update#EmailAccount",
        &[("emailAccountId", text(&account)), ("color", text("teal"))],
    );
    refused(
        "update#EmailAccount",
        &[("emailAccountId", text(&account)), ("statusId", text("EasRemoved"))],
        "Active or Paused",
    );

    // a member leaves; then reaches nothing
    login("bob");
    ok(
        "remove#EmailAccountUser",
        &[("emailAccountId", text(&account)), ("userId", text("bob"))],
    );
    let gone = ok("get#EmailAccountAccess", &[("emailAccountId", text(&account))]);
    assert_eq!(out_number(&gone, "level"), 0);

    // another user's accounts and counts with authorization disabled, as the shell counts are made
    {
        let _authz = ctx.disable_authz();
        let theirs = ok("get#EmailAccountAccess", &[("userId", text("alice"))]);
        assert_eq!(maps(&theirs, "accountList").len(), 2);
    }
    let after = ok("get#EmailAccountAccess", &[("userId", text("bob"))]);
    assert!(maps(&after, "accountList")
        .iter()
        .all(|account| field(account, "isInternal") != "false"));
    let script_off = ctx.authz_disabled();
    assert!(!script_off, "authorization is enabled again after the internal calls");
}

/// The message list: the inbox in threads and messages, the date cursor, the views and the filters, and a thread.
#[test]
fn find_messages_views_threads_and_cursor() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    let account = shared_account(&facade, "smtp.example.com");
    let base = now() - 10 * DAY;
    let at = |offset: i64| base + offset * 1000;
    message(
        &facade,
        &account,
        "M1",
        "EfkInbox",
        at(1),
        &[("messageId", text("<m1@sender.example.com>"))],
    );
    message(
        &facade,
        &account,
        "M2",
        "EfkInbox",
        at(30),
        &[("rootEmailMessageId", text("M1")), ("parentEmailMessageId", text("M1"))],
    );
    message(&facade, &account, "M3", "EfkInbox", at(20), &[("isFlagged", text("Y"))]);
    message(
        &facade,
        &account,
        "M4",
        "EfkInbox",
        at(40),
        &[("snoozedThruDate", stamp(now() + DAY))],
    );
    message(
        &facade,
        &account,
        "M5",
        "EfkSent",
        at(35),
        &[
            ("rootEmailMessageId", text("M1")),
            ("parentEmailMessageId", text("M2")),
            ("statusId", text("ES_SENT")),
            ("isRead", text("Y")),
            ("fromAddress", text("team@example.com")),
            ("toAddresses", text("\"Sender, One\" <m1@sender.example.com>")),
        ],
    );
    message(&facade, &account, "M6", "EfkTrash", at(50), &[]);
    message(
        &facade,
        &account,
        "M7",
        "EfkArchive",
        at(15),
        &[("archivedDate", stamp(at(16)))],
    );
    message(
        &facade,
        &account,
        "M8",
        "EfkDrafts",
        at(60),
        &[("statusId", text("ES_DRAFT")), ("isRead", text("Y"))],
    );
    message(
        &facade,
        &account,
        "M9",
        "EfkInbox",
        at(25),
        &[
            ("subject", text("Invoice 42")),
            ("hasAttachment", text("Y")),
            ("messageSize", Value::I64(90_000)),
        ],
    );

    // the inbox as threads, newest first by the newest message in the view; the snoozed one is out
    let inbox = ok("find#Messages", &[]);
    assert_eq!(thread_ids(&inbox), ["M1", "M9", "M3"]);
    assert_eq!(out_text(&inbox, "view"), "inbox");
    let rows = maps(&inbox, "threadList");
    let thread = &rows[0];
    assert_eq!(field(thread, "store"), "email");
    assert_eq!(field(thread, "emailAccountId"), account);
    assert_eq!(
        field(thread, "emailMessageId"),
        "M2",
        "the newest message of the thread in the view"
    );
    assert_eq!(
        field(thread, "lastEmailMessageId"),
        "M5",
        "the thread's newest, the reply sent"
    );
    assert_eq!(field(thread, "messageCount"), "3");
    assert_eq!(field(thread, "unreadCount"), "2");
    assert_eq!(field(thread, "sortDate"), at(30).to_string());
    assert_eq!(field(thread, "subject"), "Subject M1");
    assert_eq!(field(thread, "isClosed"), "false");
    let Some(Value::List(participants)) = thread.get("participantAddressList") else {
        panic!("{thread:?}")
    };
    let participants: Vec<String> = participants.iter().map(to_plain_string).collect();
    assert!(
        participants.contains(&"M1@sender.example.com".to_string()),
        "{participants:?}"
    );
    assert!(
        participants.contains(&"team@example.com".to_string()),
        "{participants:?}"
    );

    // the cursor: two a page
    let first = ok("find#Messages", &[("pageSize", Value::I64(2))]);
    assert_eq!(thread_ids(&first), ["M1", "M9"]);
    assert_eq!(first.get("hasMore"), Some(&Value::Bool(true)));
    let next = out_number(&first, "nextBeforeDate");
    assert_eq!(next, at(25));
    let second = ok(
        "find#Messages",
        &[("pageSize", Value::I64(2)), ("beforeDate", stamp(next))],
    );
    assert_eq!(thread_ids(&second), ["M3"]);
    assert_eq!(second.get("hasMore"), Some(&Value::Bool(false)));
    // M1's own message is older than the cursor, but its thread was on the first page
    let late = ok("find#Messages", &[("beforeDate", stamp(at(10)))]);
    assert!(thread_ids(&late).is_empty(), "{:?}", thread_ids(&late));

    // messages, not threads
    let messages = ok("find#Messages", &[("threadMode", Value::Bool(false))]);
    assert_eq!(message_ids(&messages), ["M2", "M9", "M3", "M1"]);
    assert_eq!(field(&maps(&messages, "threadList")[0], "threadId"), "M1");

    // the views
    let view = |name: &str| thread_ids(&ok("find#Messages", &[("view", text(name))]));
    assert_eq!(view("flagged"), ["M3"]);
    assert_eq!(view("snoozed"), ["M4"]);
    assert_eq!(view("sent"), ["M1"]);
    assert_eq!(view("trash"), ["M6"]);
    assert_eq!(view("archive"), ["M7"]);
    assert_eq!(view("drafts"), ["M8"]);
    assert_eq!(view("all"), ["M8", "M4", "M1", "M9", "M3", "M7"]);
    assert!(view("outbox").is_empty());
    let trash = ok("find#Messages", &[("view", text("trash"))]);
    assert_eq!(field(&maps(&trash, "threadList")[0], "messageCount"), "1");
    refused("find#Messages", &[("view", text("nowhere"))], "Unknown view");
    let sent_folder = folder(&facade, &account, "EfkSent");
    let in_folder = ok("find#Messages", &[("emailFolderId", text(&sent_folder))]);
    assert_eq!(out_text(&in_folder, "view"), "folder");
    assert_eq!(message_ids(&in_folder), ["M5"]);

    // the filters
    let filtered = |parameters: &[(&str, Value)]| {
        let mut all = vec![("threadMode", Value::Bool(false))];
        all.extend_from_slice(parameters);
        message_ids(&ok("find#Messages", &all))
    };
    assert_eq!(filtered(&[("searchText", text("invoice"))]), ["M9"]);
    assert_eq!(filtered(&[("subject", text("INVOICE"))]), ["M9"]);
    assert_eq!(filtered(&[("hasAttachment", Value::Bool(true))]), ["M9"]);
    assert_eq!(filtered(&[("minSize", Value::I64(50_000))]), ["M9"]);
    assert_eq!(filtered(&[("fromAddress", text("m3@sender"))]), ["M3"]);
    assert_eq!(
        filtered(&[("fromDate", stamp(at(21))), ("thruDate", stamp(at(29)))]),
        ["M9"]
    );
    set_message(&facade, "M9", &[("isRead", text("Y"))]);
    assert_eq!(filtered(&[("unreadOnly", Value::Bool(true))]), ["M2", "M3", "M1"]);
    assert_eq!(
        filtered(&[("view", text("all")), ("toAddress", text("sender, one"))]),
        ["M5"]
    );

    // a label
    let label = out_text(
        &ok(
            "create#EmailLabel",
            &[
                ("emailAccountId", text(&account)),
                ("labelName", text("Customers")),
                ("color", text("blue")),
            ],
        ),
        "emailLabelId",
    );
    ok(
        "apply#Labels",
        &[("emailMessageId", text("M3")), ("addLabelIdList", list(&[&label]))],
    );
    let labeled = ok("find#Messages", &[("emailLabelId", text(&label))]);
    assert_eq!(out_text(&labeled, "view"), "label");
    assert_eq!(thread_ids(&labeled), ["M3"]);
    let Some(Value::List(labels)) = maps(&labeled, "threadList")[0].get("labelList").cloned() else {
        panic!()
    };
    assert_eq!(labels.len(), 1);

    // a thread, oldest first, by any of its messages
    row(
        &facade,
        "moqui.basic.email.EmailMessageAttachment",
        &[
            ("emailMessageAttachmentId", text("A1")),
            ("emailMessageId", text("M2")),
            ("attachmentLocation", text("dbresource://email/test/M2/report.pdf")),
            ("fileName", text("report.pdf")),
        ],
    );
    let thread = ok("get#Thread", &[("emailMessageId", text("M2"))]);
    assert_eq!(out_text(&thread, "rootEmailMessageId"), "M1");
    assert_eq!(out_text(&thread, "subject"), "Subject M1");
    let thread_messages = maps(&thread, "messageList");
    let ids: Vec<String> = thread_messages
        .iter()
        .map(|message| field(message, "emailMessageId"))
        .collect();
    assert_eq!(ids, ["M1", "M2", "M5"]);
    assert_eq!(field(&thread_messages[2], "folderKindEnumId"), "EfkSent");
    let Some(Value::List(attachments)) = thread_messages[1].get("attachmentList") else {
        panic!()
    };
    assert_eq!(attachments.len(), 1);
    assert_eq!(field(&thread_messages[0], "body"), "<p>Body of M1</p>");
    assert_eq!(out_text(&thread, "canSend"), "true");
    assert_eq!(
        message_field(&facade, "M1", "isRead"),
        "N",
        "reading the thread marks nothing"
    );
    refused("get#Thread", &[("emailMessageId", text("NOPE"))], "not found");
}

/// Filing: flags, folders, labels, snooze (and its return by the job), archive and trash, and the unread count.
#[test]
fn filing_snooze_archive_trash_and_the_unread_count() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    let account = shared_account(&facade, "smtp.example.com");
    let other = out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("emailAddress", text("alice@home.example.com")),
                ("smtpHost", text("smtp.home.example.com")),
            ],
        ),
        "emailAccountId",
    );
    let other_stored = find_one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &other).unwrap();
    assert_eq!(
        to_plain_string(other_stored.get("recordFilingEnumId").unwrap()),
        "ErfLinkedOnly",
        "a personal external account files only what is linked"
    );
    let base = now() - 10 * DAY;
    let at = |offset: i64| base + offset * 1000;
    message(&facade, &account, "M1", "EfkInbox", at(1), &[]);
    message(
        &facade,
        &account,
        "M2",
        "EfkInbox",
        at(2),
        &[("rootEmailMessageId", text("M1")), ("parentEmailMessageId", text("M1"))],
    );
    message(
        &facade,
        &account,
        "M3",
        "EfkInbox",
        at(3),
        &[("rootEmailMessageId", text("M1")), ("parentEmailMessageId", text("M2"))],
    );
    message(&facade, &account, "M4", "EfkInbox", at(4), &[]);
    message(&facade, &other, "H1", "EfkInbox", at(5), &[]);

    let unread = |user_id: Option<&str>| {
        let out = match user_id {
            Some(id) => {
                let _authz = ctx.disable_authz();
                ok("count#UnreadEmail", &[("userId", text(id))])
            }
            None => ok("count#UnreadEmail", &[]),
        };
        (out_number(&out, "unreadCount"), out_number(&out, "unreadThreadCount"))
    };
    assert_eq!(unread(None), (5, 3));
    assert_eq!(unread(Some("bob")), (4, 2), "Bob shares only the team account");

    // flags, for a thread
    let flagged = ok(
        "set#MessageFlags",
        &[
            ("rootEmailMessageId", text("M1")),
            ("read", Value::Bool(true)),
            ("flagged", Value::Bool(true)),
        ],
    );
    assert_eq!(out_number(&flagged, "updatedCount"), 3);
    assert_eq!(message_field(&facade, "M3", "isFlagged"), "Y");
    assert_eq!(unread(None), (2, 2));
    ok(
        "set#MessageFlags",
        &[("emailMessageIdList", list(&["M2"])), ("read", Value::Bool(false))],
    );
    assert_eq!(unread(None), (3, 3));
    refused(
        "set#MessageFlags",
        &[
            ("emailMessageIdList", list(&["M2", "NOPE"])),
            ("read", Value::Bool(true)),
        ],
        "Email message NOPE not found",
    );
    assert_eq!(
        message_field(&facade, "M2", "isRead"),
        "N",
        "nothing changes when one is not found"
    );

    // folders: a move within the account, not to another's
    let projects = out_text(
        &ok(
            "create#EmailFolder",
            &[("emailAccountId", text(&account)), ("folderName", text("Projects"))],
        ),
        "emailFolderId",
    );
    refused(
        "create#EmailFolder",
        &[("emailAccountId", text(&account)), ("folderName", text("projects"))],
        "already",
    );
    ok(
        "rename#EmailFolder",
        &[("emailFolderId", text(&projects)), ("folderName", text("Clients"))],
    );
    refused(
        "rename#EmailFolder",
        &[
            ("emailFolderId", text(&folder(&facade, &account, "EfkInbox"))),
            ("folderName", text("In")),
        ],
        "Inbox is not renamed",
    );
    ok(
        "move#Messages",
        &[("emailMessageId", text("M4")), ("emailFolderId", text(&projects))],
    );
    assert_eq!(message_field(&facade, "M4", "emailFolderId"), projects);
    assert_eq!(
        thread_ids(&ok("find#Messages", &[("emailAccountId", text(&account))])),
        ["M1"]
    );
    refused(
        "move#Messages",
        &[
            ("emailMessageId", text("M4")),
            ("emailFolderId", text(&folder(&facade, &other, "EfkInbox"))),
        ],
        "not found in the account",
    );

    // labels of the message's own account only
    let label = out_text(
        &ok(
            "create#EmailLabel",
            &[("emailAccountId", text(&account)), ("labelName", text("Follow up"))],
        ),
        "emailLabelId",
    );
    let home_label = out_text(
        &ok(
            "create#EmailLabel",
            &[("emailAccountId", text(&other)), ("labelName", text("Family"))],
        ),
        "emailLabelId",
    );
    let added = ok(
        "apply#Labels",
        &[("rootEmailMessageId", text("M1")), ("addLabelIdList", list(&[&label]))],
    );
    assert_eq!(out_number(&added, "updatedCount"), 3);
    refused(
        "apply#Labels",
        &[("emailMessageId", text("M1")), ("addLabelIdList", list(&[&home_label]))],
        "is not of the account",
    );
    ok(
        "apply#Labels",
        &[("emailMessageId", text("M3")), ("removeLabelIdList", list(&[&label]))],
    );
    assert_eq!(
        message_ids(&ok(
            "find#Messages",
            &[("emailLabelId", text(&label)), ("threadMode", Value::Bool(false))]
        )),
        ["M2", "M1"]
    );
    ok(
        "update#EmailLabel",
        &[
            ("emailLabelId", text(&label)),
            ("labelName", text("Later")),
            ("color", text("amber")),
        ],
    );
    refused(
        "update#EmailLabel",
        &[("emailLabelId", text(&label)), ("labelName", text(""))],
        "needs a name",
    );
    ok("delete#EmailLabel", &[("emailLabelId", text(&label))]);

    // snooze: out of the inbox and the count until then; the job brings it back unread
    refused(
        "snooze#Messages",
        &[("emailMessageId", text("H1")), ("snoozedThruDate", stamp(now() - 1000))],
        "in the future",
    );
    ok(
        "snooze#Messages",
        &[("emailMessageId", text("H1")), ("snoozedThruDate", stamp(now() + DAY))],
    );
    assert!(thread_ids(&ok("find#Messages", &[("emailAccountId", text(&other))])).is_empty());
    assert_eq!(thread_ids(&ok("find#Messages", &[("view", text("snoozed"))])), ["H1"]);
    assert_eq!(unread(None), (1, 1), "M2 is the other unread one in an Inbox");
    ok(
        "snooze#Messages",
        &[("emailMessageId", text("H1")), ("unsnooze", Value::Bool(true))],
    );
    assert_eq!(unread(None), (2, 2));
    ok(
        "snooze#Messages",
        &[
            ("rootEmailMessageId", text("M1")),
            ("snoozedThruDate", stamp(now() + DAY)),
        ],
    );
    // the time comes
    for id in ["M1", "M2", "M3"] {
        set_message(
            &facade,
            id,
            &[("snoozedThruDate", stamp(now() - 1000)), ("isRead", text("Y"))],
        );
    }
    let returned = {
        let _authz = ctx.disable_authz();
        ok("return#SnoozedMessages", &[])
    };
    assert_eq!(out_number(&returned, "returnedCount"), 3);
    assert_eq!(out_number(&returned, "failedCount"), 0);
    assert_eq!(message_field(&facade, "M2", "snoozedThruDate"), "");
    assert_eq!(
        message_field(&facade, "M3", "isRead"),
        "N",
        "the newest of the thread is unread again"
    );
    assert_eq!(message_field(&facade, "M1", "isRead"), "Y");

    // archive and back
    let archived = ok("archive#Messages", &[("rootEmailMessageId", text("M1"))]);
    assert_eq!(out_number(&archived, "updatedCount"), 3);
    assert_eq!(
        message_field(&facade, "M2", "emailFolderId"),
        folder(&facade, &account, "EfkArchive")
    );
    assert!(!message_field(&facade, "M2", "archivedDate").is_empty());
    assert_eq!(thread_ids(&ok("find#Messages", &[("view", text("archive"))])), ["M1"]);
    assert!(thread_ids(&ok("find#Messages", &[("emailAccountId", text(&account))])).is_empty());
    ok(
        "archive#Messages",
        &[("rootEmailMessageId", text("M1")), ("unarchive", Value::Bool(true))],
    );
    assert_eq!(
        message_field(&facade, "M2", "emailFolderId"),
        folder(&facade, &account, "EfkInbox")
    );
    assert_eq!(message_field(&facade, "M2", "archivedDate"), "");

    // trash, then delete forever: the thread keeps its other messages, re-rooted
    refused(
        "trash#Messages",
        &[("emailMessageId", text("M1")), ("deleteForever", Value::Bool(true))],
        "not in the Trash",
    );
    ok("trash#Messages", &[("emailMessageId", text("M1"))]);
    assert_eq!(
        message_field(&facade, "M1", "emailFolderId"),
        folder(&facade, &account, "EfkTrash")
    );
    let in_trash = ok("find#Messages", &[("view", text("trash"))]);
    assert_eq!(thread_ids(&in_trash), ["M1"]);
    assert_eq!(field(&maps(&in_trash, "threadList")[0], "messageCount"), "1");
    assert_eq!(
        thread_ids(&ok("find#Messages", &[("emailAccountId", text(&account))])),
        ["M1"],
        "the rest of the thread is still in the inbox"
    );
    let deleted = ok(
        "trash#Messages",
        &[("emailMessageId", text("M1")), ("deleteForever", Value::Bool(true))],
    );
    assert_eq!(out_number(&deleted, "deletedCount"), 1);
    assert!(find_one(&facade, MESSAGE, "emailMessageId", "M1").is_none());
    assert_eq!(message_field(&facade, "M2", "rootEmailMessageId"), "");
    assert_eq!(message_field(&facade, "M2", "parentEmailMessageId"), "");
    assert_eq!(message_field(&facade, "M3", "rootEmailMessageId"), "M2");
    assert_eq!(
        thread_ids(&ok("find#Messages", &[("emailAccountId", text(&account))])),
        ["M2"]
    );

    // a stranger's count is not told
    login("eve");
    assert_eq!(unread(None), (0, 0));
}

/// Set the `smtpHost` of the server of `account`.
fn set_smtp_host(facade: &EntityFacade, account: &str, host: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let server = find_one(facade, "moqui.basic.email.EmailAccount", "emailAccountId", account)
        .and_then(|row| row.get("emailServerId").map(to_plain_string))
        .unwrap();
    let mut row = find_one(facade, "moqui.basic.email.EmailServer", "emailServerId", &server).unwrap();
    row.set("smtpHost", text(host));
    row.update(facade).unwrap();
}

/// Drafts and sends through the test sink: a reply with an attachment now, a scheduled send and an undo-send hold by
/// the job, a cancel, and a failed send waiting in the Outbox for its retry.
#[test]
fn drafts_send_now_scheduled_held_and_failed() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup_with(r#"<moqui-conf><email-client allowed-hosts="example.com"/></moqui-conf>"#);
    let _entered = ctx.enter();
    install();
    let account = shared_account(&facade, "smtp.example.com");
    let base = now() - DAY;
    message(
        &facade,
        &account,
        "M1",
        "EfkInbox",
        base,
        &[
            ("messageId", text("<orig@sender.example.com>")),
            ("referencesHeader", text("<first@sender.example.com>")),
            ("subject", text("Order 7")),
        ],
    );
    let dir = std::env::temp_dir().join(format!("moqui-email-client-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("note.txt"), b"attached text").unwrap();
    std::fs::write(dir.join("logo.png"), b"\x89PNG not really").unwrap();
    let note = format!("file:{}", dir.join("note.txt").display());
    let logo = format!("file:{}", dir.join("logo.png").display());
    let sink = crate::service::SinkGuard::install();

    // the Internal account takes no email draft
    let internal = out_text(&ok("ensure#InternalAccount", &[]), "emailAccountId");
    refused(
        "save#Draft",
        &[("emailAccountId", text(&internal))],
        "not an email draft",
    );

    // a file the caller names is the draft's own or one of this account's mail files; anything else is refused, the
    // test's own files here, another account's and a path out of the account's folder
    let file = |location: &str| {
        Value::List(vec![Value::Map(IndexMap::from([(
            "attachmentLocation".to_string(),
            text(location),
        )]))])
    };
    for location in [
        note.clone(),
        "dbresource://email/OTHER_ACCOUNT/1/secret.pdf".to_string(),
        format!("dbresource://email/{account}/../OTHER_ACCOUNT/secret.pdf"),
    ] {
        refused(
            "save#Draft",
            &[("emailAccountId", text(&account)), ("attachmentList", file(&location))],
            "is not a file of this message",
        );
    }
    let own = ok(
        "save#Draft",
        &[
            ("emailAccountId", text(&account)),
            (
                "attachmentList",
                file(&format!("dbresource://email/{account}/upload/notes.txt")),
            ),
        ],
    );
    let own = out_text(&own, "emailMessageId");
    {
        let _authz = ctx.disable_authz();
        ok(
            "delete#LocalMessages",
            &[("emailMessageIdList", Value::List(vec![text(&own)]))],
        );
    }

    // a reply, saved twice, then sent now; its files are named by an internal caller (an app's upload service)
    let saved = {
        let _authz = ctx.disable_authz();
        ok(
            "save#Draft",
            &[
                ("emailAccountId", text(&account)),
                ("parentEmailMessageId", text("M1")),
                ("toAddresses", text("\"Sender\" <m1@sender.example.com>")),
                ("body", text("<p>Thanks</p>")),
                ("bodyText", text("Thanks")),
                (
                    "attachmentList",
                    Value::List(vec![
                        Value::Map(IndexMap::from([
                            ("attachmentLocation".to_string(), text(&note)),
                            ("contentType".to_string(), text("text/plain")),
                            ("fileSize".to_string(), Value::I64(13)),
                        ])),
                        Value::Map(IndexMap::from([
                            ("attachmentLocation".to_string(), text(&logo)),
                            ("contentType".to_string(), text("image/png")),
                            ("contentId".to_string(), text("<logo@example.com>")),
                            ("isInline".to_string(), Value::Bool(true)),
                        ])),
                    ]),
                ),
            ],
        )
    };
    let draft = out_text(&saved, "emailMessageId");
    assert_eq!(out_text(&saved, "rootEmailMessageId"), "M1");
    assert_eq!(message_field(&facade, &draft, "statusId"), "ES_DRAFT");
    assert_eq!(
        message_field(&facade, &draft, "emailFolderId"),
        folder(&facade, &account, "EfkDrafts")
    );
    assert_eq!(message_field(&facade, &draft, "subject"), "Re: Order 7");
    assert_eq!(message_field(&facade, &draft, "inReplyTo"), "<orig@sender.example.com>");
    assert_eq!(
        message_field(&facade, &draft, "referencesHeader"),
        "<first@sender.example.com> <orig@sender.example.com>"
    );
    assert_eq!(message_field(&facade, &draft, "fromAddress"), "team@example.com");
    assert_eq!(message_field(&facade, &draft, "hasAttachment"), "Y");
    assert_eq!(message_field(&facade, &draft, "snippet"), "Thanks");
    let again = ok(
        "save#Draft",
        &[
            ("emailMessageId", text(&draft)),
            ("subject", text("Re: Order 7 shipped?")),
        ],
    );
    assert_eq!(out_text(&again, "emailMessageId"), draft);
    assert_eq!(thread_ids(&ok("find#Messages", &[("view", text("drafts"))])), ["M1"]);
    let sent = ok("send#Draft", &[("emailMessageId", text(&draft))]);
    assert_eq!(sent.get("sent"), Some(&Value::Bool(true)), "{sent:?}");
    assert_eq!(out_text(&sent, "statusId"), "ES_SENT");
    let mail = sink.sent();
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].subject, "Re: Order 7 shipped?");
    assert_eq!(mail[0].to, ["\"Sender\" <m1@sender.example.com>"]);
    assert_eq!(mail[0].attachments.len(), 1, "the inline image is not an attachment");
    assert_eq!(mail[0].attachments[0].0, "note.txt");
    assert_eq!(mail[0].attachments[0].2, b"attached text");
    assert!(
        mail[0].mime.contains("In-Reply-To: <orig@sender.example.com>"),
        "{}",
        mail[0].mime
    );
    // the inline image is in a related part with the body, by its Content-ID, and not an attachment
    assert!(mail[0].mime.contains("multipart/related"), "{}", mail[0].mime);
    assert!(
        mail[0].mime.contains("Content-ID: <logo@example.com>"),
        "{}",
        mail[0].mime
    );
    assert!(
        mail[0]
            .mime
            .contains("References: <first@sender.example.com> <orig@sender.example.com>"),
        "{}",
        mail[0].mime
    );
    assert_eq!(message_field(&facade, &draft, "statusId"), "ES_SENT");
    assert_eq!(
        message_field(&facade, &draft, "emailFolderId"),
        folder(&facade, &account, "EfkSent")
    );
    assert!(!message_field(&facade, &draft, "messageId").is_empty());
    assert_eq!(message_field(&facade, "M1", "isAnswered"), "Y");
    refused("send#Draft", &[("emailMessageId", text(&draft))], "is not a draft");

    // scheduled: waits in the Outbox until the job finds it due
    let new_draft = |subject: &str| {
        out_text(
            &ok(
                "save#Draft",
                &[
                    ("emailAccountId", text(&account)),
                    ("toAddresses", text("someone@example.com")),
                    ("subject", text(subject)),
                    ("body", text("<p>Hello</p>")),
                ],
            ),
            "emailMessageId",
        )
    };
    let later = new_draft("Later");
    let scheduled = ok(
        "send#Draft",
        &[
            ("emailMessageId", text(&later)),
            ("scheduledSendDate", stamp(now() + DAY)),
        ],
    );
    assert_eq!(out_text(&scheduled, "statusId"), "ES_SCHEDULED");
    assert_eq!(message_field(&facade, &later, "emailFolderId"), "");
    assert_eq!(
        message_ids(&ok("find#Messages", &[("view", text("scheduled"))])),
        [later.as_str()]
    );
    assert_eq!(
        message_ids(&ok("find#Messages", &[("view", text("outbox"))])),
        [later.as_str()]
    );
    let job = |parameters: &[(&str, Value)]| {
        let _authz = ctx.disable_authz();
        ok("send#DueDrafts", parameters)
    };
    assert_eq!(out_number(&job(&[]), "sentCount"), 0, "not due yet");
    set_message(&facade, &later, &[("scheduledSendDate", stamp(now() - 1000))]);
    let ran = job(&[]);
    assert_eq!(out_number(&ran, "sentCount"), 1);
    assert_eq!(out_number(&ran, "failedCount"), 0);
    assert_eq!(sink.sent().len(), 2);
    assert_eq!(message_field(&facade, &later, "statusId"), "ES_SENT");
    assert_eq!(message_field(&facade, &later, "scheduledSendDate"), "");

    // the undo-send hold: undone back to Drafts, then held again and sent by the client when it ends
    let held = new_draft("Held");
    let holding = ok(
        "send#Draft",
        &[("emailMessageId", text(&held)), ("holdSeconds", Value::I64(10))],
    );
    assert_eq!(out_text(&holding, "statusId"), "ES_SCHEDULED");
    assert!(out_number(&holding, "scheduledSendDate") > now());
    ok("cancel#ScheduledSend", &[("emailMessageId", text(&held))]);
    assert_eq!(message_field(&facade, &held, "statusId"), "ES_DRAFT");
    assert_eq!(
        message_field(&facade, &held, "emailFolderId"),
        folder(&facade, &account, "EfkDrafts")
    );
    ok(
        "send#Draft",
        &[("emailMessageId", text(&held)), ("holdSeconds", Value::I64(10))],
    );
    let early = ok("send#DueDrafts", &[("emailMessageId", text(&held))]);
    assert_eq!(out_number(&early, "sentCount"), 0, "the hold has not ended");
    set_message(&facade, &held, &[("scheduledSendDate", stamp(now() - 1000))]);
    login("eve");
    refused("send#DueDrafts", &[("emailMessageId", text(&held))], "not found");
    refused("cancel#ScheduledSend", &[("emailMessageId", text(&held))], "not found");
    login("alice");
    let flushed = ok("send#DueDrafts", &[("emailMessageId", text(&held))]);
    assert_eq!(out_number(&flushed, "sentCount"), 1);
    assert_eq!(sink.sent().len(), 3);
    refused(
        "cancel#ScheduledSend",
        &[("emailMessageId", text(&held))],
        "sent already",
    );

    // a send that fails waits in the Outbox with its error, and goes on a retry
    let failing = new_draft("Fails");
    {
        set_smtp_host(&facade, &account, "smtp.other.example.org");
        let failed = ok("send#Draft", &[("emailMessageId", text(&failing))]);
        set_smtp_host(&facade, &account, "smtp.example.com");
        assert_eq!(failed.get("sent"), Some(&Value::Bool(false)));
        assert!(out_text(&failed, "sendError").contains("allowed-hosts"), "{failed:?}");
    }
    assert_eq!(message_field(&facade, &failing, "statusId"), "ES_READY");
    let outbox = ok("find#Messages", &[("view", text("outbox"))]);
    assert_eq!(message_ids(&outbox), [failing.as_str()]);
    assert!(field(&maps(&outbox, "threadList")[0], "sendError").contains("allowed-hosts"));
    let accounts = ok("get#UserEmailAccounts", &[]);
    assert_eq!(field(&maps(&accounts, "accountList")[1], "outboxErrorCount"), "1");
    refused(
        "move#Messages",
        &[
            ("emailMessageId", text(&failing)),
            ("emailFolderId", text(&folder(&facade, &account, "EfkArchive"))),
        ],
        "in the Outbox",
    );
    refused(
        "trash#Messages",
        &[("emailMessageId", text(&failing))],
        "cancel its send first",
    );
    // a failed send goes back to Drafts to be changed, and then sends from there
    ok("cancel#ScheduledSend", &[("emailMessageId", text(&failing))]);
    assert_eq!(message_field(&facade, &failing, "statusId"), "ES_DRAFT");
    assert_eq!(message_field(&facade, &failing, "sendError"), "");
    set_message(
        &facade,
        &failing,
        &[("statusId", text("ES_READY")), ("sendError", text("earlier failure"))],
    );
    let retried = ok("send#Draft", &[("emailMessageId", text(&failing))]);
    assert_eq!(retried.get("sent"), Some(&Value::Bool(true)), "{retried:?}");
    assert_eq!(sink.sent().len(), 4);
    assert_eq!(message_field(&facade, &failing, "sendError"), "");

    // what a send checks first
    let no_to = out_text(
        &ok("save#Draft", &[("emailAccountId", text(&account)), ("body", text("x"))]),
        "emailMessageId",
    );
    refused("send#Draft", &[("emailMessageId", text(&no_to))], "Add a recipient");
    ok(
        "save#Draft",
        &[
            ("emailMessageId", text(&no_to)),
            ("toAddresses", text("not an address")),
        ],
    );
    refused(
        "send#Draft",
        &[("emailMessageId", text(&no_to))],
        "Not an email address",
    );
    let no_server = out_text(
        &ok("create#EmailAccount", &[("emailAddress", text("local@example.com"))]),
        "emailAccountId",
    );
    let stranded = out_text(
        &ok(
            "save#Draft",
            &[
                ("emailAccountId", text(&no_server)),
                ("toAddresses", text("x@example.com")),
                ("body", text("x")),
            ],
        ),
        "emailMessageId",
    );
    refused(
        "send#Draft",
        &[("emailMessageId", text(&stranded))],
        "no outgoing (SMTP) server",
    );
    // discarding a draft deletes it
    ok("trash#Messages", &[("emailMessageId", text(&stranded))]);
    assert!(find_one(&facade, MESSAGE, "emailMessageId", &stranded).is_none());
    assert_eq!(sink.sent().len(), 4);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Identities, the automatic reply, text blocks and saved searches.
#[test]
fn identities_out_of_office_text_blocks_and_searches() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    let account = shared_account(&facade, "smtp.example.com");
    let alias = out_text(
        &ok(
            "create#EmailIdentity",
            &[
                ("emailAccountId", text(&account)),
                ("emailAddress", text("sales@example.com")),
                ("displayName", text("Sales")),
                ("replyToAddress", text("orders@example.com")),
                ("isDefault", Value::Bool(true)),
            ],
        ),
        "emailIdentityId",
    );
    let accounts = ok("get#UserEmailAccounts", &[]);
    let Some(Value::List(identities)) = maps(&accounts, "accountList")[1].get("identityList").cloned() else {
        panic!()
    };
    let defaults: Vec<String> = identities
        .iter()
        .filter_map(|item| match item {
            Value::Map(map) if field(map, "isDefault") == "true" => Some(field(map, "emailIdentityId")),
            _ => None,
        })
        .collect();
    assert_eq!(defaults, [alias.as_str()]);
    let draft = ok(
        "save#Draft",
        &[
            ("emailAccountId", text(&account)),
            ("toAddresses", text("x@example.com")),
        ],
    );
    let draft = out_text(&draft, "emailMessageId");
    assert_eq!(message_field(&facade, &draft, "fromAddress"), "sales@example.com");
    assert_eq!(message_field(&facade, &draft, "fromName"), "Sales");
    ok(
        "update#EmailIdentity",
        &[("emailIdentityId", text(&alias)), ("displayName", text("Sales Team"))],
    );
    refused(
        "create#EmailIdentity",
        &[("emailAccountId", text(&account)), ("emailAddress", text("nope"))],
        "Not an email address",
    );
    login("carol");
    refused(
        "update#EmailIdentity",
        &[("emailIdentityId", text(&alias)), ("displayName", text("Mine"))],
        "You may not change",
    );
    login("alice");
    ok("delete#EmailIdentity", &[("emailIdentityId", text(&alias))]);
    assert_eq!(message_field(&facade, &draft, "emailIdentityId"), "");

    // the automatic reply
    refused(
        "store#EmailOutOfOffice",
        &[("emailAccountId", text(&account)), ("enabled", Value::Bool(true))],
        "needs a message",
    );
    ok(
        "store#EmailOutOfOffice",
        &[
            ("emailAccountId", text(&account)),
            ("subject", text("Away")),
            ("bodyHtml", text("<p>Back Monday</p>")),
            ("fromDate", stamp(now() - DAY)),
            ("thruDate", stamp(now() + DAY)),
        ],
    );
    login("bob");
    let away = ok("get#EmailOutOfOffice", &[("emailAccountId", text(&account))]);
    assert_eq!(away.get("isActive"), Some(&Value::Bool(true)));
    refused(
        "store#EmailOutOfOffice",
        &[("emailAccountId", text(&account)), ("bodyHtml", text("x"))],
        "You may not change the automatic reply",
    );

    // text blocks: my own and those shared with a group I am in
    row(
        &facade,
        "moqui.security.UserGroup",
        &[("userGroupId", text("SUPPORT")), ("description", text("Support"))],
    );
    for member in ["alice", "bob"] {
        row(
            &facade,
            "moqui.security.UserGroupMember",
            &[
                ("userGroupId", text("SUPPORT")),
                ("userId", text(member)),
                ("fromDate", stamp(now() - DAY)),
            ],
        );
    }
    login("alice");
    let shared = out_text(
        &ok(
            "create#EmailTextBlock",
            &[
                ("title", text("Refund policy")),
                ("bodyHtml", text("<p>30 days</p>")),
                ("userGroupId", text("SUPPORT")),
            ],
        ),
        "emailTextBlockId",
    );
    ok(
        "create#EmailTextBlock",
        &[("title", text("My signature line")), ("shortcut", text("sig"))],
    );
    refused(
        "create#EmailTextBlock",
        &[("title", text("x")), ("userGroupId", text("ADMIN"))],
        "not in user group",
    );
    assert_eq!(maps(&ok("find#EmailTextBlocks", &[]), "textBlockList").len(), 2);
    login("bob");
    let bobs = maps(&ok("find#EmailTextBlocks", &[]), "textBlockList");
    assert_eq!(bobs.len(), 1);
    assert_eq!(field(&bobs[0], "title"), "Refund policy");
    assert_eq!(field(&bobs[0], "isOwner"), "false");
    refused(
        "update#EmailTextBlock",
        &[("emailTextBlockId", text(&shared)), ("title", text("Mine"))],
        "not found",
    );
    login("eve");
    assert!(maps(&ok("find#EmailTextBlocks", &[]), "textBlockList").is_empty());
    login("alice");
    assert_eq!(
        maps(
            &ok("find#EmailTextBlocks", &[("searchText", text("30 DAYS"))]),
            "textBlockList"
        )
        .len(),
        1
    );
    ok("delete#EmailTextBlock", &[("emailTextBlockId", text(&shared))]);

    // saved searches
    let search = out_text(
        &ok(
            "store#EmailSearch",
            &[
                ("searchName", text("Invoices")),
                ("queryText", text("subject:invoice has:attachment")),
            ],
        ),
        "emailSearchId",
    );
    assert_eq!(maps(&ok("find#EmailSearches", &[]), "searchList").len(), 1);
    login("bob");
    assert!(maps(&ok("find#EmailSearches", &[]), "searchList").is_empty());
    refused("delete#EmailSearch", &[("emailSearchId", text(&search))], "not found");
}

/// Removing an account: kept as Removed with its mail, or deleted with all of it.
#[test]
fn delete_account_keeps_or_deletes_local_copies() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    let account = shared_account(&facade, "smtp.example.com");
    message(&facade, &account, "K1", "EfkInbox", now() - DAY, &[]);
    login("carol");
    refused(
        "delete#EmailAccount",
        &[("emailAccountId", text(&account))],
        "You may not remove",
    );
    login("alice");
    let internal = out_text(&ok("ensure#InternalAccount", &[]), "emailAccountId");
    refused(
        "delete#EmailAccount",
        &[("emailAccountId", text(&internal))],
        "not removed",
    );
    let kept = ok("delete#EmailAccount", &[("emailAccountId", text(&account))]);
    assert_eq!(out_number(&kept, "keptMessageCount"), 1);
    let stored = find_one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &account).unwrap();
    assert_eq!(to_plain_string(stored.get("statusId").unwrap()), "EasRemoved");
    let server = to_plain_string(stored.get("emailServerId").unwrap());
    let server_row = find_one(&facade, "moqui.basic.email.EmailServer", "emailServerId", &server).unwrap();
    assert!(
        server_row.get("mailPassword").is_none_or(|value| value.is_null()),
        "the password is gone"
    );
    assert_eq!(maps(&ok("get#UserEmailAccounts", &[]), "accountList").len(), 1);
    login("carol");
    assert_eq!(
        out_number(
            &ok("get#EmailAccountAccess", &[("emailAccountId", text(&account))]),
            "level"
        ),
        0
    );
    assert!(find_one(&facade, MESSAGE, "emailMessageId", "K1").is_some());

    login("alice");
    let other = out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("emailAddress", text("old@example.com")),
                ("smtpHost", text("smtp.old.example.com")),
            ],
        ),
        "emailAccountId",
    );
    message(&facade, &other, "D1", "EfkInbox", now() - 2 * DAY, &[]);
    message(
        &facade,
        &other,
        "D2",
        "EfkInbox",
        now() - DAY,
        &[("rootEmailMessageId", text("D1")), ("parentEmailMessageId", text("D1"))],
    );
    let label = out_text(
        &ok(
            "create#EmailLabel",
            &[("emailAccountId", text(&other)), ("labelName", text("Old"))],
        ),
        "emailLabelId",
    );
    ok(
        "apply#Labels",
        &[("rootEmailMessageId", text("D1")), ("addLabelIdList", list(&[&label]))],
    );
    let other_server = to_plain_string(
        find_one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &other)
            .unwrap()
            .get("emailServerId")
            .unwrap(),
    );
    let deleted = ok(
        "delete#EmailAccount",
        &[
            ("emailAccountId", text(&other)),
            ("deleteLocalCopies", Value::Bool(true)),
        ],
    );
    assert_eq!(out_number(&deleted, "deletedMessageCount"), 2);
    assert!(find_one(&facade, MESSAGE, "emailMessageId", "D1").is_none());
    assert!(find_one(&facade, "moqui.basic.email.EmailAccount", "emailAccountId", &other).is_none());
    assert!(find_one(&facade, "moqui.basic.email.EmailServer", "emailServerId", &other_server).is_none());
    assert!(find_one(&facade, "moqui.basic.email.EmailLabel", "emailLabelId", &label).is_none());
}

/// A scripted SMTP server: greeting, EHLO with AUTH, AUTH, NOOP, QUIT. Returns its port.
fn fake_smtp() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else { return };
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let _ = writer.write_all(b"220 fake.example.com ESMTP\r\n");
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            let command = line.trim_end().to_ascii_uppercase();
            let reply: &[u8] = if command.starts_with("EHLO") {
                b"250-fake.example.com\r\n250 AUTH PLAIN LOGIN\r\n"
            } else if command.starts_with("AUTH") {
                b"235 2.7.0 Authentication successful\r\n"
            } else if command.starts_with("QUIT") {
                let _ = writer.write_all(b"221 Bye\r\n");
                break;
            } else {
                b"250 OK\r\n"
            };
            let _ = writer.write_all(reply);
            line.clear();
        }
    });
    port
}

/// A scripted IMAP server: greeting, CAPABILITY, LOGIN, LIST with special-use attributes, LOGOUT. Returns its port.
fn fake_imap() -> u16 {
    fake_imap_with(false, std::sync::Arc::new(std::sync::Mutex::new(Vec::new())))
}

/// [`fake_imap`], offering STARTTLS when `starttls` (and closing after its OK, so the TLS handshake fails); every
/// command it gets goes into `log`.
fn fake_imap_with(starttls: bool, log: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else { return };
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let _ = writer.write_all(b"* OK fake IMAP ready\r\n");
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            let mut parts = line.trim_end().splitn(3, ' ');
            let tag = parts.next().unwrap_or("*").to_string();
            let command = parts.next().unwrap_or("").to_ascii_uppercase();
            log.lock().unwrap().push(command.clone());
            let reply = match command.as_str() {
                "CAPABILITY" if starttls => {
                    format!("* CAPABILITY IMAP4rev1 STARTTLS LOGINDISABLED\r\n{tag} OK CAPABILITY completed\r\n")
                }
                "CAPABILITY" => format!("* CAPABILITY IMAP4rev1\r\n{tag} OK CAPABILITY completed\r\n"),
                "STARTTLS" => {
                    let _ = writer.write_all(format!("{tag} OK Begin TLS negotiation now\r\n").as_bytes());
                    break;
                }
                "LOGIN" => format!("{tag} OK LOGIN completed\r\n"),
                "LIST" => format!(
                    concat!(
                        "* LIST (\\HasNoChildren) \"/\" INBOX\r\n",
                        "* LIST (\\HasNoChildren \\Sent) \"/\" \"Sent Mail\"\r\n",
                        "* LIST (\\Noselect \\HasChildren) \"/\" Projects\r\n",
                        "{} OK LIST completed\r\n"
                    ),
                    tag
                ),
                "LOGOUT" => {
                    let _ = writer.write_all(format!("* BYE logging out\r\n{tag} OK LOGOUT completed\r\n").as_bytes());
                    break;
                }
                _ => format!("{tag} OK\r\n"),
            };
            let _ = writer.write_all(reply.as_bytes());
            line.clear();
        }
    });
    port
}

/// The server test step by step against scripted local servers, and a step that fails.
#[test]
fn test_email_account_against_local_servers() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    for id in ["alice", "carol"] {
        user(&facade, id);
    }
    login("alice");
    let smtp = fake_smtp();
    let imap = fake_imap();
    let account = out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("emailAddress", text("alice@example.com")),
                ("smtpHost", text("127.0.0.1")),
                ("smtpPort", text(&smtp.to_string())),
                ("smtpStartTls", text("N")),
                ("storeHost", text("127.0.0.1")),
                ("storePort", text(&imap.to_string())),
                ("storeProtocol", text("imap")),
                ("mailUsername", text("alice")),
                ("mailPassword", text("app-password")),
            ],
        ),
        "emailAccountId",
    );
    let tested = ok("test#EmailAccount", &[("emailAccountId", text(&account))]);
    let steps = maps(&tested, "stepList");
    let summary: Vec<(String, String)> = steps
        .iter()
        .map(|step| (field(step, "step"), field(step, "ok")))
        .collect();
    assert_eq!(
        summary,
        [
            ("smtp".to_string(), "true".to_string()),
            ("storeLogin".to_string(), "true".to_string()),
            ("storeList".to_string(), "true".to_string())
        ],
        "{steps:?}"
    );
    assert!(field(&steps[0], "message").contains("signed in"), "{steps:?}");
    assert_eq!(tested.get("success"), Some(&Value::Bool(true)));
    let folders = maps(&tested, "folderList");
    let kinds: Vec<(String, String)> = folders
        .iter()
        .map(|folder| (field(folder, "folderPath"), field(folder, "folderKindEnumId")))
        .collect();
    assert_eq!(
        kinds,
        [
            ("INBOX".to_string(), "EfkInbox".to_string()),
            ("Sent Mail".to_string(), "EfkSent".to_string()),
            ("Projects".to_string(), "EfkOther".to_string())
        ]
    );
    assert!(
        field(&folders[2], "attributeList").contains("\\Noselect"),
        "{:?}",
        folders[2]
    );

    // nothing listens now: the SMTP step fails and says why, the store is not asked
    let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    ok(
        "update#EmailAccount",
        &[
            ("emailAccountId", text(&account)),
            ("smtpPort", text(&closed.to_string())),
        ],
    );
    let failed = ok(
        "test#EmailAccount",
        &[("emailAccountId", text(&account)), ("testStore", Value::Bool(false))],
    );
    assert_eq!(failed.get("success"), Some(&Value::Bool(false)));
    let steps = maps(&failed, "stepList");
    assert_eq!(steps.len(), 1);
    assert_eq!(field(&steps[0], "ok"), "false");
    assert!(field(&steps[0], "message").contains("SMTP 127.0.0.1"), "{steps:?}");

    // only a manager tests an account
    ok(
        "add#EmailAccountUser",
        &[
            ("emailAccountId", text(&account)),
            ("userId", text("carol")),
            ("permissionEnumId", text("EapSend")),
        ],
    );
    login("carol");
    refused(
        "test#EmailAccount",
        &[("emailAccountId", text(&account))],
        "You may not test",
    );

    // a plain IMAP port whose server offers STARTTLS: the test upgrades to TLS as the sync does, and never signs in in
    // plain text (here the TLS handshake fails, since the scripted server has no TLS)
    login("alice");
    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let secure = fake_imap_with(true, log.clone());
    ok(
        "update#EmailAccount",
        &[
            ("emailAccountId", text(&account)),
            ("storePort", text(&secure.to_string())),
        ],
    );
    let upgraded = ok(
        "test#EmailAccount",
        &[("emailAccountId", text(&account)), ("testSmtp", Value::Bool(false))],
    );
    let steps = maps(&upgraded, "stepList");
    assert_eq!(field(&steps[0], "step"), "storeLogin", "{steps:?}");
    assert_eq!(field(&steps[0], "ok"), "false", "{steps:?}");
    let commands = log.lock().unwrap().clone();
    assert!(commands.contains(&"STARTTLS".to_string()), "{commands:?}");
    assert!(!commands.contains(&"LOGIN".to_string()), "{commands:?}");
}

/// Message ids of a find#Messages answer listed by message, for a search line in a view.
fn searched(view: &str, search_text: &str) -> Vec<String> {
    let mut parameters = vec![("threadMode", Value::Bool(false)), ("searchText", text(search_text))];
    if !view.is_empty() {
        parameters.push(("view", text(view)));
    }
    message_ids(&ok("find#Messages", &parameters))
}

/// The search line without the search index (no backend here): Gmail's operators as conditions, in: as the view or
/// a folder by name, the free text as words that must each be somewhere (a phrase as it is, -words out), the errors
/// for a value that is not understood, and only the mail of the accounts the user reaches.
#[test]
fn the_search_line_operators_and_free_text_without_the_index() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    let account = shared_account(&facade, "smtp.example.com");
    let day = |days: i64| now() - days * DAY;
    message(
        &facade,
        &account,
        "S1",
        "EfkInbox",
        day(9),
        &[
            ("fromAddress", text("ada@math.example.com")),
            ("fromName", text("Ada Lovelace")),
            ("subject", text("Weekly report")),
            ("body", text("<p>Numbers for the engine</p>")),
            ("bodyText", text("Numbers for the engine")),
            ("messageSize", Value::I64(1_000)),
        ],
    );
    message(
        &facade,
        &account,
        "S2",
        "EfkInbox",
        day(8),
        &[
            ("fromAddress", text("bob@builder.example.com")),
            ("subject", text("Invoice 77")),
            ("ccAddresses", text("Ada <ada@math.example.com>")),
            ("hasAttachment", text("Y")),
            ("isFlagged", text("Y")),
            ("isRead", text("Y")),
            ("messageSize", Value::I64(200_000)),
        ],
    );
    message(
        &facade,
        &account,
        "S3",
        "EfkSent",
        day(5),
        &[
            ("statusId", text("ES_SENT")),
            ("fromAddress", text("team@example.com")),
            ("toAddresses", text("carol@example.com")),
            ("subject", text("Re: weekly report")),
            ("isRead", text("Y")),
        ],
    );
    message(
        &facade,
        &account,
        "S4",
        "EfkInbox",
        day(3),
        &[
            ("fromAddress", text("noreply@news.example.com")),
            ("subject", text("News")),
            ("body", text("<p>Engine news</p>")),
        ],
    );
    let projects = out_text(
        &ok(
            "create#EmailFolder",
            &[("emailAccountId", text(&account)), ("folderName", text("Projects"))],
        ),
        "emailFolderId",
    );
    message(
        &facade,
        &account,
        "S5",
        "",
        day(12),
        &[("emailFolderId", text(&projects)), ("subject", text("Engine plans"))],
    );
    message(
        &facade,
        &account,
        "S6",
        "EfkTrash",
        day(1),
        &[("subject", text("Engine in the trash"))],
    );
    row(
        &facade,
        "moqui.basic.email.EmailLabel",
        &[
            ("emailLabelId", text("SL1")),
            ("emailAccountId", text(&account)),
            ("labelName", text("News letters")),
        ],
    );
    row(
        &facade,
        "moqui.basic.email.EmailMessageLabel",
        &[("emailMessageId", text("S4")), ("emailLabelId", text("SL1"))],
    );
    row(
        &facade,
        "moqui.basic.email.EmailMessageAttachment",
        &[
            ("emailMessageAttachmentId", text("SA1")),
            ("emailMessageId", text("S2")),
            ("fileName", text("invoice-77.pdf")),
        ],
    );

    // the free text: each word somewhere, a phrase as it is, a -word out; LIKE, as no search index is configured
    let out = ok(
        "find#Messages",
        &[
            ("view", text("all")),
            ("threadMode", Value::Bool(false)),
            ("searchText", text("engine")),
        ],
    );
    assert_eq!(out_text(&out, "searchMode"), "text");
    assert_eq!(message_ids(&out), ["S4", "S1", "S5"]);
    assert_eq!(searched("all", "engine numbers"), ["S1"]);
    assert_eq!(searched("all", "\"weekly report\""), ["S3", "S1"]);
    assert_eq!(searched("all", "engine -news"), ["S1", "S5"]);
    assert_eq!(searched("anywhere", "engine"), ["S6", "S4", "S1", "S5"]);
    assert_eq!(searched("", "in:anywhere engine"), ["S6", "S4", "S1", "S5"]);

    // the operators
    assert_eq!(searched("all", "from:ada"), ["S1"]);
    assert_eq!(searched("all", "from:(Ada Lovelace)"), ["S1"]);
    assert_eq!(searched("all", "-from:ada"), ["S4", "S3", "S2", "S5"]);
    assert_eq!(searched("all", "to:ada"), ["S2"]);
    assert_eq!(searched("all", "to:carol"), ["S3"]);
    assert_eq!(searched("all", "subject:\"weekly report\""), ["S3", "S1"]);
    assert_eq!(searched("all", "has:attachment"), ["S2"]);
    assert_eq!(searched("all", "-has:attachment"), ["S4", "S3", "S1", "S5"]);
    assert_eq!(searched("all", "label:news-letters"), ["S4"]);
    assert_eq!(searched("all", "label:\"News Letters\""), ["S4"]);
    assert_eq!(searched("all", "-label:news-letters"), ["S3", "S2", "S1", "S5"]);
    assert!(searched("all", "label:nothing").is_empty());
    assert_eq!(searched("all", "is:unread"), ["S4", "S1", "S5"]);
    assert_eq!(searched("all", "is:read"), ["S3", "S2"]);
    assert_eq!(searched("all", "is:starred"), ["S2"]);
    assert_eq!(searched("all", "-is:flagged is:unread"), ["S4", "S1", "S5"]);
    assert_eq!(searched("all", "older_than:6d"), ["S2", "S1", "S5"]);
    assert_eq!(searched("all", "newer_than:6d"), ["S4", "S3"]);
    let date = |days: i64| {
        jiff::Timestamp::from_millisecond(day(days))
            .unwrap()
            .to_zoned(jiff::tz::TimeZone::UTC)
            .strftime("%Y/%m/%d")
            .to_string()
    };
    assert_eq!(searched("all", &format!("after:{}", date(6))), ["S4", "S3"]);
    assert_eq!(searched("all", &format!("before:{}", date(6))), ["S2", "S1", "S5"]);
    assert_eq!(searched("all", "larger:100K"), ["S2"]);
    assert_eq!(searched("all", "smaller:10k"), ["S1"]);
    assert_eq!(searched("all", "filename:invoice"), ["S2"]);
    assert_eq!(searched("all", "from:ada engine"), ["S1"]);

    // in: is the view, or the folders of that name
    let sent = ok("find#Messages", &[("searchText", text("in:sent"))]);
    assert_eq!(out_text(&sent, "view"), "sent");
    assert_eq!(thread_ids(&sent), ["S3"]);
    let folder = ok("find#Messages", &[("searchText", text("in:projects"))]);
    assert_eq!(out_text(&folder, "view"), "folder");
    assert_eq!(thread_ids(&folder), ["S5"]);
    assert_eq!(out_text(&folder, "searchMode"), "");

    // the terms for the chips
    let chips = ok(
        "find#Messages",
        &[("view", text("all")), ("searchText", text("from:ada -is:read report"))],
    );
    let terms: Vec<(String, String, String)> = maps(&chips, "termList")
        .iter()
        .map(|term| (field(term, "operator"), field(term, "value"), field(term, "negate")))
        .collect();
    assert_eq!(
        terms,
        [
            ("from".to_string(), "ada".to_string(), "false".to_string()),
            ("is".to_string(), "read".to_string(), "true".to_string())
        ]
    );
    let parsed = ok(
        "parse#SearchText",
        &[(
            "searchText",
            text(
                "from:ada \"weekly report\" -news https://x.example.com/a:b subject:(big plan) larger:5M nothing:here",
            ),
        )],
    );
    assert_eq!(
        parsed.get("wordList"),
        Some(&list(&["https://x.example.com/a:b", "nothing:here"]))
    );
    assert_eq!(parsed.get("phraseList"), Some(&list(&["weekly report"])));
    assert_eq!(parsed.get("excludeList"), Some(&list(&["news"])));
    assert_eq!(
        maps(&parsed, "termList")
            .iter()
            .map(|term| format!("{}:{}", field(term, "operator"), field(term, "value")))
            .collect::<Vec<_>>(),
        ["from:ada", "subject:big plan", "larger:5M"]
    );

    // a value that is not understood is an error naming it
    for (line, expected) in [
        ("before:someday", "needs a date"),
        ("is:purple", "is:purple is not a search term"),
        ("has:boat", "has:boat is not a search term"),
        ("-in:trash", "-in:trash is not a search term"),
        ("in:nowhere", "No folder nowhere"),
        ("larger:huge", "needs a size"),
        ("older_than:soon", "needs a count"),
    ] {
        refused(
            "find#Messages",
            &[("view", text("all")), ("searchText", text(line))],
            expected,
        );
    }

    // a member reads the same; a stranger finds none of it
    login("bob");
    assert_eq!(searched("all", "engine"), ["S4", "S1", "S5"]);
    login("eve");
    assert!(searched("all", "engine").is_empty());
}

/// New mail in a snoozed thread brings the thread back (Gmail); mail the rules sent to Junk does not, and only the sync
/// (authorization disabled) calls it.
#[test]
fn new_mail_wakes_its_snoozed_thread() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    let account = shared_account(&facade, "smtp.example.com");
    let later = stamp(now() + DAY);
    message(
        &facade,
        &account,
        "W1",
        "EfkInbox",
        now() - DAY,
        &[("snoozedThruDate", later.clone())],
    );
    message(
        &facade,
        &account,
        "W2",
        "EfkInbox",
        now(),
        &[("rootEmailMessageId", text("W1")), ("parentEmailMessageId", text("W1"))],
    );
    message(
        &facade,
        &account,
        "W3",
        "EfkInbox",
        now() - DAY,
        &[("snoozedThruDate", later)],
    );
    message(
        &facade,
        &account,
        "W4",
        "EfkJunk",
        now(),
        &[("rootEmailMessageId", text("W3"))],
    );
    refused(
        "wake#SnoozedThreads",
        &[
            ("emailAccountId", text(&account)),
            ("emailMessageIdList", list(&["W2"])),
        ],
        "internal",
    );
    let woken = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        ok(
            "wake#SnoozedThreads",
            &[
                ("emailAccountId", text(&account)),
                ("emailMessageIdList", list(&["W2", "W4"])),
            ],
        )
    };
    assert_eq!(out_number(&woken, "wokenCount"), 1);
    assert_eq!(message_field(&facade, "W1", "snoozedThruDate"), "");
    assert_ne!(message_field(&facade, "W3", "snoozedThruDate"), "");
    let inbox = ok("find#Messages", &[]);
    assert!(thread_ids(&inbox).contains(&"W1".to_string()), "{inbox:?}");
}

/// The automatic reply: once per sender in its days, the internal text to a user or an address of the organization's
/// domains, never to a list, bulk or automatic mail, a no-reply sender, mail the account was not sent to, mail from
/// before it was on, or spam; with Auto-Submitted on it; and storing it again starts over.
#[test]
fn the_automatic_reply_goes_once_to_each_sender_it_is_for() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    for id in ["alice", "bob"] {
        user(&facade, id);
    }
    login("alice");
    let account = out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("emailAddress", text("alice@home.example.net")),
                ("accountName", text("Alice")),
                ("smtpHost", text("smtp.example.net")),
                ("smtpStartTls", text("N")),
            ],
        ),
        "emailAccountId",
    );
    // a shared account makes its domain the organization's own
    ok(
        "create#EmailAccount",
        &[
            ("accountKindEnumId", text("EakShared")),
            ("emailAddress", text("orders@acme.example.com")),
            ("smtpHost", text("smtp.example.net")),
        ],
    );
    let store = || {
        ok(
            "store#EmailOutOfOffice",
            &[
                ("emailAccountId", text(&account)),
                ("fromDate", stamp(now() - DAY)),
                ("subject", text("Away")),
                ("bodyHtml", text("<p>Away until Monday</p>")),
                ("internalBodyHtml", text("<p>Away; ask Dave</p>")),
                ("replyOncePerDays", Value::I64(4)),
            ],
        )
    };
    store();
    let received = |id: &str, from: &str, kind: &str, more: &[(&str, Value)]| {
        let mut fields = vec![
            ("fromAddress", text(from)),
            ("toAddresses", text("Alice <alice@home.example.net>")),
            ("receivedDate", stamp(now() - 60_000)),
            ("messageId", text(&format!("<{id}@mail.example.org>"))),
        ];
        fields.extend_from_slice(more);
        message(&facade, &account, id, kind, now() - 60_000, &fields);
    };
    received("O1", "ada@math.example.org", "EfkInbox", &[]);
    received("O2", "ADA@math.example.org", "EfkInbox", &[]);
    received("O3", "dave@acme.example.com", "EfkInbox", &[]);
    received("O4", "bob@example.com", "EfkInbox", &[]);
    received(
        "O5",
        "news@lists.example.org",
        "EfkInbox",
        &[("headersString", text("List-Id: <news.lists.example.org>"))],
    );
    received(
        "O6",
        "deals@shop.example.org",
        "EfkInbox",
        &[("headersString", text("Precedence: bulk"))],
    );
    received(
        "O7",
        "robot@x.example.org",
        "EfkInbox",
        &[("headersString", text("Auto-Submitted: auto-replied"))],
    );
    received("O8", "no-reply@x.example.org", "EfkInbox", &[]);
    received(
        "O9",
        "someone@x.example.org",
        "EfkInbox",
        &[("toAddresses", text("list@lists.example.org"))],
    );
    received(
        "O10",
        "old@x.example.org",
        "EfkInbox",
        &[("receivedDate", stamp(now() - 10 * DAY))],
    );
    received("O11", "spam@x.example.org", "EfkJunk", &[]);
    let sink = crate::service::SinkGuard::install();
    let reply = |ids: &[&str]| {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        ok(
            "send#OutOfOfficeReplies",
            &[("emailAccountId", text(&account)), ("emailMessageIdList", list(ids))],
        )
    };
    let out = reply(&["O1", "O2", "O3", "O4", "O5", "O6", "O7", "O8", "O9", "O10", "O11"]);
    assert_eq!(out_number(&out, "sentCount"), 3, "{out:?}");
    let sent = sink.sent();
    let to: Vec<String> = sent.iter().map(|mail| mail.to.join(",")).collect();
    assert_eq!(to, ["ada@math.example.org", "dave@acme.example.com", "bob@example.com"]);
    assert!(sent.iter().all(|mail| mail.subject == "Away"), "{sent:?}");
    assert!(sent[0].body.contains("Away until Monday"), "{:?}", sent[0].body);
    assert!(sent[1].body.contains("ask Dave"), "{:?}", sent[1].body);
    assert!(sent[2].body.contains("ask Dave"), "{:?}", sent[2].body);
    assert!(
        sent[0].mime.contains("Auto-Submitted: auto-replied"),
        "{}",
        sent[0].mime
    );
    assert!(
        sent[0].mime.contains("In-Reply-To: <O1@mail.example.org>"),
        "{}",
        sent[0].mime
    );
    // the mail answered is not marked answered, and the reply is not in its thread
    assert_eq!(message_field(&facade, "O1", "isAnswered"), "");

    // once per sender in its days; a new automatic reply goes to everyone again
    received("O12", "ada@math.example.org", "EfkInbox", &[]);
    assert_eq!(out_number(&reply(&["O12"]), "sentCount"), 0);
    store();
    assert_eq!(out_number(&reply(&["O12"]), "sentCount"), 1);
    assert_eq!(sink.sent().len(), 4);

    // off: nothing
    ok(
        "store#EmailOutOfOffice",
        &[
            ("emailAccountId", text(&account)),
            ("enabled", Value::Bool(false)),
            ("bodyHtml", text("<p>Away</p>")),
        ],
    );
    received("O13", "carol@x.example.org", "EfkInbox", &[]);
    assert_eq!(out_number(&reply(&["O13"]), "sentCount"), 0);
}

/// The test's HTTP sender: it answers the list's addresses and passes any other request on, as a test elsewhere in
/// the crate may make one meanwhile.
struct ListServer {
    seen: std::sync::Mutex<Vec<crate::outbound::Request>>,
    status: std::sync::atomic::AtomicU16,
}

impl crate::outbound::HttpSender for ListServer {
    fn send(&self, request: crate::outbound::Request) -> Result<crate::outbound::Response, moqui_lib::error::Error> {
        if !request.url.starts_with("https://lists.example.org/") {
            return crate::outbound::send_default(request);
        }
        self.seen.lock().unwrap().push(request);
        Ok(crate::outbound::Response {
            status: self.status.load(std::sync::atomic::Ordering::SeqCst),
            body: "ok".to_string(),
            ..Default::default()
        })
    }
}

/// Unsubscribe by List-Unsubscribe: the one-click POST (RFC 8058), the mailto when the POST fails, the address for the
/// client to open, a private host refused, and get#Thread saying which.
#[test]
fn unsubscribe_one_click_mailto_and_url() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    let account = shared_account(&facade, "smtp.example.com");
    let both = "List-Unsubscribe: <mailto:leave@lists.example.org?subject=leave%20now>,\n <https://lists.example.org/u/1>\nList-Unsubscribe-Post: List-Unsubscribe=One-Click";
    message(
        &facade,
        &account,
        "U1",
        "EfkInbox",
        now(),
        &[("headersString", text(both))],
    );
    message(
        &facade,
        &account,
        "U2",
        "EfkInbox",
        now(),
        &[("headersString", text(both))],
    );
    message(
        &facade,
        &account,
        "U3",
        "EfkInbox",
        now(),
        &[(
            "headersString",
            text("List-Unsubscribe: <https://lists.example.org/page?id=3>"),
        )],
    );
    message(
        &facade,
        &account,
        "U4",
        "EfkInbox",
        now(),
        &[(
            "headersString",
            text("List-Unsubscribe: <https://192.168.1.9/u>\nList-Unsubscribe-Post: List-Unsubscribe=One-Click"),
        )],
    );
    message(&facade, &account, "U5", "EfkInbox", now(), &[]);
    let server = std::sync::Arc::new(ListServer {
        seen: std::sync::Mutex::new(Vec::new()),
        status: std::sync::atomic::AtomicU16::new(200),
    });
    crate::outbound::install_http_sender(server.clone());
    let sink = crate::service::SinkGuard::install();

    let one = ok("unsubscribe#Message", &[("emailMessageId", text("U1"))]);
    assert_eq!(out_text(&one, "method"), "oneClick");
    assert_eq!(one.get("unsubscribed"), Some(&Value::Bool(true)));
    {
        let seen = server.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, crate::outbound::Method::Post);
        assert_eq!(seen[0].url, "https://lists.example.org/u/1");
        assert_eq!(seen[0].body.as_deref(), Some("List-Unsubscribe=One-Click"));
        assert!(
            seen[0]
                .headers
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case("content-type")
                    && value == "application/x-www-form-urlencoded"),
            "{:?}",
            seen[0].headers
        );
    }
    assert!(sink.sent().is_empty());

    // the POST fails: the mailto goes from the account
    server.status.store(500, std::sync::atomic::Ordering::SeqCst);
    let mailto = ok("unsubscribe#Message", &[("emailMessageId", text("U2"))]);
    assert_eq!(out_text(&mailto, "method"), "mailto");
    assert!(out_text(&mailto, "postError").contains("HTTP 500"), "{mailto:?}");
    let sent = sink.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, ["leave@lists.example.org"]);
    assert_eq!(sent[0].subject, "leave now");

    // no one-click: the page for the client to open, nothing sent
    let url = ok("unsubscribe#Message", &[("emailMessageId", text("U3"))]);
    assert_eq!(out_text(&url, "method"), "url");
    assert_eq!(out_text(&url, "url"), "https://lists.example.org/page?id=3");
    // a private host is not posted to
    let private = ok("unsubscribe#Message", &[("emailMessageId", text("U4"))]);
    assert_eq!(out_text(&private, "method"), "url");
    assert!(out_text(&private, "postError").contains("public host"), "{private:?}");
    assert_eq!(server.seen.lock().unwrap().len(), 2);
    refused(
        "unsubscribe#Message",
        &[("emailMessageId", text("U5"))],
        "no List-Unsubscribe",
    );
    crate::outbound::clear_http_sender();

    let thread = |id: &str| {
        field(
            &maps(&ok("get#Thread", &[("emailMessageId", text(id))]), "messageList")[0],
            "unsubscribeMethod",
        )
    };
    assert_eq!(thread("U1"), "oneClick");
    assert_eq!(thread("U3"), "url");
    assert_eq!(thread("U5"), "");
}

/// A test message: from the account to its own address through its SMTP (kept by MailCapture), then in Sent, by a user
/// who may send from it; a send that fails answers its error and leaves nothing in the Outbox.
#[test]
fn a_test_message_goes_from_the_account_to_itself() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup_with(r#"<moqui-conf><email-client allowed-hosts="example.com"/></moqui-conf>"#);
    let _entered = ctx.enter();
    install();
    let account = shared_account(&facade, "smtp.example.com");
    let capture = crate::service::mail_capture::MailCapture::of(ctx.factory());
    capture.set_enabled(Some(true));
    capture.clear();

    let out = ok("send#TestMessage", &[("emailAccountId", text(&account))]);
    assert_eq!(out.get("sent"), Some(&Value::Bool(true)), "{out:?}");
    assert_eq!(out_text(&out, "toAddress"), "team@example.com");
    let mails = capture.messages();
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].to, ["team@example.com"]);
    assert_eq!(mails[0].from_address, "team@example.com");
    assert!(mails[0].subject.starts_with("[Test] "), "{}", mails[0].subject);
    assert_eq!(mails[0].subject, out_text(&out, "subject"));
    assert!(mails[0].body_text.contains("smtp.example.com"), "{}", mails[0].body_text);
    let sent = out_text(&out, "emailMessageId");
    assert_eq!(message_field(&facade, &sent, "statusId"), "ES_SENT");
    assert_eq!(message_field(&facade, &sent, "emailFolderId"), folder(&facade, &account, "EfkSent"));

    // send permission: Carol sends; Bob only reads; Eve does not reach the account; the Internal account has no server
    login("carol");
    ok("send#TestMessage", &[("emailAccountId", text(&account))]);
    assert_eq!(capture.messages().len(), 2);
    login("bob");
    refused("send#TestMessage", &[("emailAccountId", text(&account))], "may not send");
    login("eve");
    refused("send#TestMessage", &[("emailAccountId", text(&account))], "not found");
    login("alice");
    let internal = out_text(&ok("ensure#InternalAccount", &[]), "emailAccountId");
    refused("send#TestMessage", &[("emailAccountId", text(&internal))], "no server");

    // a send that fails: its error, and no test message waiting in the Outbox
    set_smtp_host(&facade, &account, "smtp.other.example.org");
    let failed = ok("send#TestMessage", &[("emailAccountId", text(&account))]);
    set_smtp_host(&facade, &account, "smtp.example.com");
    assert_eq!(failed.get("sent"), Some(&Value::Bool(false)), "{failed:?}");
    assert!(out_text(&failed, "sendError").contains("allowed-hosts"), "{failed:?}");
    assert_eq!(out_text(&failed, "emailMessageId"), "");
    assert!(message_ids(&ok("find#Messages", &[("view", text("outbox"))])).is_empty());
    assert_eq!(capture.messages().len(), 2);
    capture.set_enabled(None);
}

/// The mail search index of a test: every message in it matches the free text; a search answers those dated before
/// the filter's messageDate bound (and among its emailMessageId values, when it has them), newest first, at most its
/// limit, and keeps each limit asked for.
struct MailIndex {
    documents: Vec<(String, i64)>,
    limits: std::sync::Mutex<Vec<usize>>,
}

struct MailIndexBackend(std::sync::Arc<MailIndex>);

/// The messageDate bound (lt, as epoch millis) and the emailMessageId values of a search filter.
fn filter_bounds(query: &crate::search::Query, before: &mut Option<i64>, ids: &mut Option<Vec<String>>) {
    match query {
        crate::search::Query::Bool { all, .. } => {
            for inner in all {
                filter_bounds(inner, before, ids);
            }
        }
        crate::search::Query::Field(field) if field.name == "messageDate" => {
            *before = match &field.lt {
                Some(Value::Timestamp(at)) => Some(at.as_millisecond()),
                Some(value) => to_plain_string(value).parse::<f64>().ok().map(|millis| millis as i64),
                None => None,
            };
        }
        crate::search::Query::Field(field) if field.name == "emailMessageId" => {
            *ids = Some(field.values.iter().map(to_plain_string).collect());
        }
        _ => {}
    }
}

impl crate::search::SearchBackend for MailIndexBackend {
    fn name(&self) -> &'static str {
        "mail-index"
    }
    fn ensure_index(&self, _mapping: &crate::search::DocumentMapping) -> Result<(), moqui_lib::error::Error> {
        Ok(())
    }
    fn index_exists(&self, _mapping: &crate::search::DocumentMapping) -> Result<bool, moqui_lib::error::Error> {
        Ok(true)
    }
    fn index_documents(
        &self,
        _mapping: &crate::search::DocumentMapping,
        documents: &[(String, crate::entity::document::Document)],
    ) -> Result<usize, moqui_lib::error::Error> {
        Ok(documents.len())
    }
    fn delete_document(
        &self,
        _mapping: &crate::search::DocumentMapping,
        _id: &str,
    ) -> Result<bool, moqui_lib::error::Error> {
        Ok(true)
    }
    fn delete_matching(
        &self,
        _mapping: &crate::search::DocumentMapping,
        _filter: &crate::search::Query,
    ) -> Result<u64, moqui_lib::error::Error> {
        Ok(0)
    }
    fn get_document(
        &self,
        _mapping: &crate::search::DocumentMapping,
        _id: &str,
    ) -> Result<Option<crate::entity::document::Document>, moqui_lib::error::Error> {
        Ok(None)
    }
    fn search(
        &self,
        _mappings: &[std::sync::Arc<crate::search::DocumentMapping>],
        request: &crate::search::SearchRequest,
    ) -> Result<crate::search::SearchPage, moqui_lib::error::Error> {
        moqui_lib::sync::lock(&self.0.limits).push(request.limit);
        let (mut before, mut ids) = (None, None);
        if let Some(filter) = &request.filter {
            filter_bounds(filter, &mut before, &mut ids);
        }
        let mut found: Vec<&(String, i64)> = self
            .0
            .documents
            .iter()
            .filter(|(id, at)| before.is_none_or(|bound| *at < bound) && ids.as_ref().is_none_or(|ids| ids.contains(id)))
            .collect();
        found.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        let total = found.len() as u64;
        let hits = found
            .into_iter()
            .take(request.limit)
            .map(|(id, _)| crate::search::SearchHit {
                document_id: "MoquiEmailMessage".to_string(),
                id: id.clone(),
                index: "moqui_mail".to_string(),
                score: 1.0,
                body: Default::default(),
                highlights: Default::default(),
            })
            .collect();
        Ok(crate::search::SearchPage {
            hits,
            total,
            ..Default::default()
        })
    }
    fn aggregate(
        &self,
        _mapping: &crate::search::DocumentMapping,
        _filter: &crate::search::Query,
        _aggs: &IndexMap<String, crate::search::Agg>,
    ) -> Result<IndexMap<String, Value>, moqui_lib::error::Error> {
        Ok(IndexMap::new())
    }
    fn drop_index(&self, _mapping: &crate::search::DocumentMapping) -> Result<(), moqui_lib::error::Error> {
        Ok(())
    }
}

/// The index search reads its matches searchLimit at a time from the page's cursor, so a view with more matches than
/// one read still pages through all of them, by message and by thread, with a thread shown on an earlier page (by a
/// newer reply that matches) not shown again.
#[test]
fn the_index_search_pages_past_one_read_of_the_index() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    let account = shared_account(&facade, "smtp.example.com");
    const HOUR: i64 = 3_600_000;
    let base = now() - 30 * DAY;
    // I1 (oldest) to I12 match the search; I9 answers I3; I10 is archived; N1 to N3 do not match
    let mut documents = Vec::new();
    for number in 1..=12 {
        let id = format!("I{number}");
        let at = base + number * HOUR;
        let mut more = vec![("subject", text(&format!("Invoice {number}")))];
        if number == 9 {
            more.extend([("rootEmailMessageId", text("I3")), ("parentEmailMessageId", text("I3"))]);
        }
        if number == 10 {
            more.push(("archivedDate", stamp(at)));
        }
        message(&facade, &account, &id, "EfkInbox", at, &more);
        documents.push((id, at));
    }
    for (number, at) in [(1, base + 5 * HOUR + HOUR / 2), (2, base + 9 * HOUR + HOUR / 2), (3, base + 13 * HOUR)] {
        message(&facade, &account, &format!("N{number}"), "EfkInbox", at, &[]);
    }
    {
        let _authz = ctx.disable_authz();
        let data = std::fs::read_to_string(crate::context::framework_dir().join("data/MailSearchData.xml")).unwrap();
        crate::entity::load_feed::load_entity_xml_text(&facade, "MailSearchData.xml", &data, &Default::default())
            .unwrap();
    }
    let index = std::sync::Arc::new(MailIndex {
        documents,
        limits: Default::default(),
    });
    crate::search::install(
        ctx.factory(),
        crate::search::SearchFacade::new(
            std::sync::Arc::new(MailIndexBackend(std::sync::Arc::clone(&index))),
            facade.clone(),
        ),
    );

    // the pages of a search for invoice in a view, 4 rows a page, the index read 5 matches at a time
    let pages = |view: &str, thread_mode: bool| {
        let mut pages = Vec::new();
        let mut before: Option<i64> = None;
        loop {
            let mut parameters = vec![
                ("view", text(view)),
                ("threadMode", Value::Bool(thread_mode)),
                ("searchText", text("invoice")),
                ("searchLimit", Value::I64(5)),
                ("pageSize", Value::I64(4)),
            ];
            if let Some(millis) = before {
                parameters.push(("beforeDate", stamp(millis)));
            }
            let out = ok("find#Messages", &parameters);
            assert_eq!(out_text(&out, "searchMode"), "index");
            pages.push(if thread_mode { thread_ids(&out) } else { message_ids(&out) });
            if out.get("hasMore") != Some(&Value::Bool(true)) {
                break;
            }
            before = Some(out_number(&out, "nextBeforeDate"));
            assert!(pages.len() < 10, "{pages:?}");
        }
        pages
    };
    assert_eq!(
        pages("all", false),
        [
            ["I12", "I11", "I10", "I9"],
            ["I8", "I7", "I6", "I5"],
            ["I4", "I3", "I2", "I1"]
        ]
    );
    // threads in the Inbox: I3's thread comes at its reply I9, and not again at I3 on the last page
    assert_eq!(
        pages("inbox", true),
        [
            vec!["I12", "I11", "I3", "I8"],
            vec!["I7", "I6", "I5", "I4"],
            vec!["I2", "I1"]
        ]
    );
    // no read of the window took more than searchLimit
    let limits = moqui_lib::sync::lock(&index.limits).clone();
    assert!(limits.iter().all(|limit| *limit <= 5), "{limits:?}");
    assert!(limits.len() > 6, "{limits:?}");
}

/// The automatic reply's own internal domains: kept in lower case, each once; one that is not a domain is refused; a
/// sender at one of them is internal (the internal message, and the internal-only audience).
#[test]
fn the_automatic_reply_counts_its_own_domains_as_internal() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    user(&facade, "alice");
    login("alice");
    let account = out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("emailAddress", text("alice@home.example.net")),
                ("accountName", text("Alice")),
                ("smtpHost", text("smtp.example.net")),
                ("smtpStartTls", text("N")),
            ],
        ),
        "emailAccountId",
    );
    let store = |domains: &str| {
        call(
            "store#EmailOutOfOffice",
            &[
                ("emailAccountId", text(&account)),
                ("fromDate", stamp(now() - DAY)),
                ("bodyHtml", text("<p>Away until Monday</p>")),
                ("internalBodyHtml", text("<p>Away; ask Dave</p>")),
                ("replyAudienceEnumId", text("EraInternal")),
                ("internalDomains", text(domains)),
            ],
        )
    };
    store(" Partner.Example.org, @ally.example.com partner.example.org;");
    assert_eq!(errors(), "");
    let away = ok("get#EmailOutOfOffice", &[("emailAccountId", text(&account))]);
    let reply = match away.get("outOfOffice") {
        Some(Value::Map(map)) => map.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(field(&reply, "internalDomains"), "partner.example.org,ally.example.com");
    store("partner.example.org, intranet");
    assert!(errors().contains("Not a domain: intranet"));
    // the refused list left the stored one
    let away = ok("get#EmailOutOfOffice", &[("emailAccountId", text(&account))]);
    assert!(matches!(away.get("outOfOffice"), Some(Value::Map(map)) if field(map, "internalDomains") == "partner.example.org,ally.example.com"));

    let received = |id: &str, from: &str| {
        message(
            &facade,
            &account,
            id,
            "EfkInbox",
            now() - 60_000,
            &[
                ("fromAddress", text(from)),
                ("toAddresses", text("alice@home.example.net")),
                ("receivedDate", stamp(now() - 60_000)),
                ("messageId", text(&format!("<{id}@mail.example.org>"))),
            ],
        );
    };
    received("D1", "Pat@Partner.example.org");
    received("D2", "sam@ally.example.com");
    received("D3", "kim@elsewhere.example.org");
    let sink = crate::service::SinkGuard::install();
    let out = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        ok(
            "send#OutOfOfficeReplies",
            &[("emailAccountId", text(&account)), ("emailMessageIdList", list(&["D1", "D2", "D3"]))],
        )
    };
    assert_eq!(out_number(&out, "sentCount"), 2, "{out:?}");
    let sent = sink.sent();
    let to: Vec<String> = sent.iter().map(|mail| mail.to.join(",")).collect();
    assert_eq!(to, ["Pat@Partner.example.org", "sam@ally.example.com"]);
    assert!(sent.iter().all(|mail| mail.body.contains("ask Dave")), "{sent:?}");
}
