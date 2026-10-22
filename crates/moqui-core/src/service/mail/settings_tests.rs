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

//! The mail settings of `org.moqui.impl.EmailClientServices` against the scripted local servers of
//! [`super::fake_servers`]: a separate SMTP sign-in (used, tried before it is stored, cleared), the account kind
//! (personal and shared), a folder kind a user chose and an unsubscribed folder through the sync, the automatic reply's
//! audience with the contact hook, a rule tried without acting, a removed account deleted by its owner, and the live
//! progress of a sync.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use super::fake_servers::{FakeImap, FakeSmtp};
use super::sync_tests::{
    alice, by_message_id, call, errors, field, folder_id, imap_account, login, mail, now, ok, one, out_number,
    out_text, row, rows, setup, setup_with, stamp, sync, text, update, user,
};
use crate::context::ExecutionContext;
use crate::entity::EntityFacade;

const ACCOUNT: &str = "moqui.basic.email.EmailAccount";
const SERVER: &str = "moqui.basic.email.EmailServer";
const FOLDER: &str = "moqui.basic.email.EmailFolder";
const MESSAGE: &str = "moqui.basic.email.EmailMessage";
const DAY: i64 = 86_400_000;

/// The contact hooks the tests point the email_contact_service property and the theme value at: a contact is an address
/// at friends.example.org (of alice), and one that fails.
const STUBS: &str = r#"<services>
    <service verb="filter" noun="Contacts">
        <in-parameters><parameter name="userId"/><parameter name="emailAddressList" type="List"/></in-parameters>
        <out-parameters><parameter name="contactAddressList" type="List"/></out-parameters>
        <actions><script><![CDATA[
            contactAddressList = Array.from(emailAddressList || []).filter((address) => userId === 'alice' &&
                String(address).toUpperCase().endsWith('@FRIENDS.EXAMPLE.ORG'))
        ]]></script></actions>
    </service>
    <service verb="fail" noun="Contacts">
        <in-parameters><parameter name="userId"/><parameter name="emailAddressList" type="List"/></in-parameters>
        <actions><script><![CDATA[ ec.message.addError('the contacts are away') ]]></script></actions>
    </service>
    <service verb="record" noun="RuleAction">
        <in-parameters><parameter name="emailMessageId"/><parameter name="emailAccountId"/><parameter name="emailRuleId"/>
            <parameter name="ruleOwnerUserId"/><parameter name="value"/></in-parameters>
        <out-parameters><parameter name="stopProcessing" type="Boolean"/></out-parameters>
        <actions><script><![CDATA[
            const message = ec.entity.find('moqui.basic.email.EmailMessage').condition('emailMessageId', emailMessageId).one()
            message.set('snippet', [value, emailAccountId === message.emailAccountId ? 'account' : 'other', emailRuleId, ruleOwnerUserId].join('|'))
            message.update()
            stopProcessing = value === 'stop'
        ]]></script></actions>
    </service>
    <service verb="fail" noun="RuleAction">
        <in-parameters><parameter name="emailMessageId"/><parameter name="value"/></in-parameters>
        <actions><script><![CDATA[ ec.message.addError('the app is away') ]]></script></actions>
    </service>
</services>"#;

/// The mail services and, with `stubs`, the contact hooks (test.MailStub) from a directory of the test's own.
fn install_with(stubs: Option<&std::path::Path>) {
    let framework = crate::context::framework_dir().join("service/org/moqui");
    let mut paths = vec![
        framework.join("EmailServices.xml"),
        framework.join("impl/EmailServices.xml"),
        framework.join("impl/EmailClientServices.xml"),
        framework.join("impl/ShellServices.xml"),
    ];
    if let Some(dir) = stubs {
        paths.push(dir.join("service/test/MailStub.xml"));
    }
    let failures = crate::service::lint(&paths).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

fn stub_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-mail-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("service/test")).unwrap();
    std::fs::write(dir.join("service/test/MailStub.xml"), STUBS).unwrap();
    dir
}

/// The property a test sets, cleared after it.
struct Property(&'static str);

impl Property {
    fn set(name: &'static str, value: &str) -> Self {
        moqui_lib::env::set_property(name, value);
        Property(name)
    }
}

impl Drop for Property {
    fn drop(&mut self) {
        moqui_lib::env::clear_property(self.0);
    }
}

fn refused(name: &str, parameters: &[(&str, Value)], expected: &str) {
    let _ = call(name, parameters);
    let errors = errors();
    assert!(
        errors.contains(expected),
        "{name}: expected an error with {expected:?}, got {errors:?}"
    );
}

fn maps(value: Option<&Value>) -> Vec<IndexMap<String, Value>> {
    match value {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::Map(map) => Some(map.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn get(map: &IndexMap<String, Value>, key: &str) -> String {
    map.get(key).map(to_plain_string).unwrap_or_default()
}

fn list(items: &[&str]) -> Value {
    Value::List(items.iter().map(|item| Value::string(*item)).collect())
}

fn conditions(items: &[(&str, &str, &str)]) -> Value {
    Value::List(
        items
            .iter()
            .map(|(field, operator, value)| {
                let mut map = IndexMap::new();
                map.insert("field".to_string(), text(field));
                map.insert("operator".to_string(), text(operator));
                map.insert("value".to_string(), text(value));
                Value::Map(map)
            })
            .collect(),
    )
}

/// A stored message of an account in its folder of `kind`, `age` before now.
fn message(facade: &EntityFacade, account: &str, id: &str, kind: &str, age: i64, more: &[(&str, Value)]) {
    let mut fields = vec![
        ("emailMessageId", text(id)),
        ("emailAccountId", text(account)),
        ("emailFolderId", text(&folder_id(facade, account, kind))),
        ("statusId", text("ES_RECEIVED")),
        ("subject", text(&format!("Subject {id}"))),
        ("snippet", text(&format!("Body of {id}"))),
        (
            "fromAddress",
            text(&format!("{}@sender.example.com", id.to_lowercase())),
        ),
        ("toAddresses", text("alice@example.com")),
        ("messageDate", stamp(now() - age)),
        ("receivedDate", stamp(now() - age)),
        ("messageId", text(&format!("<{id}@mail.example.org>"))),
        ("isRead", text("N")),
    ];
    for (name, value) in more {
        fields.retain(|(existing, _)| existing != name);
        fields.push((name, value.clone()));
    }
    row(facade, MESSAGE, &fields);
}

fn server_of(facade: &EntityFacade, account: &str) -> String {
    field(&one(facade, ACCOUNT, "emailAccountId", account), "emailServerId")
}

/// SMTP signs in as its own user when the server has one, a sign-in is tried before it is stored, and without its own
/// user SMTP signs in as the store does again (a server made before has none, so it works as it did).
#[test]
fn a_separate_smtp_sign_in_is_used_tried_and_cleared() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install_with(None);
    alice(&facade);
    let smtp = FakeSmtp::start("unused");
    lock(&smtp.state).passwords = vec![
        ("alice-smtp".to_string(), "smtp-secret".to_string()),
        ("alice".to_string(), "app-password".to_string()),
    ];
    let account = out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("emailAddress", text("alice@example.com")),
                ("smtpHost", text("127.0.0.1")),
                ("smtpPort", text(&smtp.port.to_string())),
                ("smtpStartTls", text("N")),
                ("mailUsername", text("alice")),
                ("mailPassword", text("app-password")),
                ("smtpUsername", text("alice-smtp")),
                ("smtpPassword", text("smtp-secret")),
            ],
        ),
        "emailAccountId",
    );
    let server = server_of(&facade, &account);
    assert_eq!(
        field(&one(&facade, SERVER, "emailServerId", &server), "smtpUsername"),
        "alice-smtp"
    );
    let auths = || lock(&smtp.state).auths.clone();

    let tested = ok(
        "test#EmailAccount",
        &[("emailAccountId", text(&account)), ("testStore", Value::Bool(false))],
    );
    assert_eq!(tested.get("success"), Some(&Value::Bool(true)), "{tested:?}");
    let steps = maps(tested.get("stepList"));
    assert!(
        get(&steps[0], "message").contains("signed in as alice-smtp"),
        "{steps:?}"
    );
    assert_eq!(auths(), ["PLAIN alice-smtp ok"]);

    // a password tried before it is stored, which it is not
    let tried = ok(
        "test#EmailAccount",
        &[
            ("emailAccountId", text(&account)),
            ("testStore", Value::Bool(false)),
            ("smtpPassword", text("wrong")),
        ],
    );
    assert_eq!(tried.get("success"), Some(&Value::Bool(false)), "{tried:?}");
    assert_eq!(auths().last().map(String::as_str), Some("PLAIN alice-smtp refused"));
    assert_eq!(
        field(&one(&facade, SERVER, "emailServerId", &server), "smtpPassword"),
        "smtp-secret"
    );

    // a send through the account
    row(
        &facade,
        MESSAGE,
        &[
            ("emailMessageId", text("SMTP_SEND")),
            ("emailAccountId", text(&account)),
            ("statusId", text("ES_READY")),
            ("fromAddress", text("alice@example.com")),
            ("toAddresses", text("bob@example.com")),
            ("subject", text("Through its own SMTP sign-in")),
            ("bodyText", text("Hello Bob")),
        ],
    );
    let sent = crate::service::sync(
        "org.moqui.impl.EmailServices.send#EmailMessage",
        &BTreeMap::from([("emailMessageId".to_string(), text("SMTP_SEND"))]),
    )
    .unwrap();
    assert_eq!(errors(), "");
    assert!(!out_text(&sent, "messageId").is_empty(), "{sent:?}");
    assert_eq!(lock(&smtp.state).messages.len(), 1);
    assert_eq!(auths().last().map(String::as_str), Some("PLAIN alice-smtp ok"));

    // an empty password keeps the one stored; no SMTP user of its own clears both
    ok(
        "update#EmailAccount",
        &[("emailAccountId", text(&account)), ("smtpPassword", text(""))],
    );
    assert_eq!(
        field(&one(&facade, SERVER, "emailServerId", &server), "smtpPassword"),
        "smtp-secret"
    );
    ok(
        "update#EmailAccount",
        &[("emailAccountId", text(&account)), ("smtpUsername", text(""))],
    );
    let cleared = one(&facade, SERVER, "emailServerId", &server);
    assert_eq!(field(&cleared, "smtpUsername"), "");
    assert_eq!(field(&cleared, "smtpPassword"), "");
    let tested = ok(
        "test#EmailAccount",
        &[("emailAccountId", text(&account)), ("testStore", Value::Bool(false))],
    );
    assert_eq!(tested.get("success"), Some(&Value::Bool(true)), "{tested:?}");
    assert!(
        get(&maps(tested.get("stepList"))[0], "message").ends_with("signed in as alice"),
        "{tested:?}"
    );
    assert_eq!(auths().last().map(String::as_str), Some("PLAIN alice ok"));
    // set again through update#EmailAccount
    ok(
        "update#EmailAccount",
        &[
            ("emailAccountId", text(&account)),
            ("smtpUsername", text(" alice-smtp ")),
            ("smtpPassword", text("smtp-secret")),
        ],
    );
    ok(
        "test#EmailAccount",
        &[("emailAccountId", text(&account)), ("testStore", Value::Bool(false))],
    );
    assert_eq!(auths().last().map(String::as_str), Some("PLAIN alice-smtp ok"));
}

fn kind_and_filing(facade: &EntityFacade, account: &str) -> (String, String) {
    let row = one(facade, ACCOUNT, "emailAccountId", account);
    (field(&row, "accountKindEnumId"), field(&row, "recordFilingEnumId"))
}

/// A manager changes an account between personal and shared: to shared at any time, back only when one person is left
/// (an account with no owner goes to its last member); the filing policy follows the kind unless given.
#[test]
fn the_account_kind_changes_between_personal_and_shared() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install_with(None);
    for id in ["bob", "carol"] {
        user(&facade, id);
    }
    alice(&facade);
    let account = out_text(
        &ok("create#EmailAccount", &[("emailAddress", text("alice@example.com"))]),
        "emailAccountId",
    );
    let kind = |kind: &str| [("emailAccountId", text(&account)), ("accountKindEnumId", text(kind))];
    assert_eq!(
        kind_and_filing(&facade, &account),
        ("EakExternal".to_string(), "ErfLinkedOnly".to_string())
    );
    ok("update#EmailAccount", &kind("EakShared"));
    assert_eq!(
        kind_and_filing(&facade, &account),
        ("EakShared".to_string(), "ErfAllMatches".to_string())
    );
    ok(
        "add#EmailAccountUser",
        &[
            ("emailAccountId", text(&account)),
            ("userId", text("bob")),
            ("permissionEnumId", text("EapManage")),
        ],
    );
    ok(
        "add#EmailAccountUser",
        &[
            ("emailAccountId", text(&account)),
            ("userId", text("carol")),
            ("permissionEnumId", text("EapRead")),
        ],
    );
    // in plain words, without the account's id
    let _ = call("update#EmailAccount", &kind("EakExternal"));
    let refusal = errors();
    assert_eq!(
        refusal,
        "This account is shared with 2 more people: remove them first, so only one is left, to make it personal"
    );
    assert!(!refusal.contains(&account), "{refusal}");
    login("carol");
    refused("update#EmailAccount", &kind("EakExternal"), "You may not change");
    login("bob");
    refused("update#EmailAccount", &kind("EakExternal"), "remove them first");
    login("alice");
    for member in ["bob", "carol"] {
        ok(
            "remove#EmailAccountUser",
            &[("emailAccountId", text(&account)), ("userId", text(member))],
        );
    }
    ok("update#EmailAccount", &kind("EakExternal"));
    assert_eq!(
        kind_and_filing(&facade, &account),
        ("EakExternal".to_string(), "ErfLinkedOnly".to_string())
    );
    // a policy given wins over the kind's
    ok(
        "update#EmailAccount",
        &[
            ("emailAccountId", text(&account)),
            ("accountKindEnumId", text("EakShared")),
            ("recordFilingEnumId", text("ErfLinkedOnly")),
        ],
    );
    assert_eq!(
        kind_and_filing(&facade, &account),
        ("EakShared".to_string(), "ErfLinkedOnly".to_string())
    );
    refused(
        "update#EmailAccount",
        &kind("EakInternal"),
        "does not become the Internal account",
    );
    refused("update#EmailAccount", &kind("EakPigeon"), "Unknown account kind");
    let internal = out_text(&ok("ensure#InternalAccount", &[]), "emailAccountId");
    refused(
        "update#EmailAccount",
        &[
            ("emailAccountId", text(&internal)),
            ("accountKindEnumId", text("EakShared")),
        ],
        "takes only a name",
    );

    // a shared account of an organization with no owner: the one member left becomes its owner
    update(
        &facade,
        ACCOUNT,
        ("emailAccountId", &account),
        &[("ownerUserId", Value::Null)],
    );
    row(
        &facade,
        "moqui.basic.email.EmailAccountUser",
        &[
            ("emailAccountId", text(&account)),
            ("userId", text("bob")),
            ("fromDate", stamp(now() - 1000)),
            ("permissionEnumId", text("EapManage")),
        ],
    );
    login("bob");
    ok("update#EmailAccount", &kind("EakExternal"));
    let row = one(&facade, ACCOUNT, "emailAccountId", &account);
    assert_eq!(field(&row, "ownerUserId"), "bob");
    assert_eq!(field(&row, "accountKindEnumId"), "EakExternal");
    let access = ok("get#EmailAccountAccess", &[("emailAccountId", text(&account))]);
    assert_eq!(out_number(&access, "level"), 3);
}

/// A folder kind a user chose holds against the server's special use, also while its folder is gone from the server; a
/// placeholder made with the account goes; an unsubscribed folder is not read; the Inbox stays.
#[test]
fn a_folder_kind_a_user_chose_holds_and_an_unsubscribed_folder_is_not_synced() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install_with(None);
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS", "MOVE", "SPECIAL-USE"]);
    imap.add_folder("Projects", "\\HasNoChildren", true);
    let account = imap_account(imap.port, "app-password");
    // by the server's path: the folders made with the account keep their own names
    let by_path = |path: &str| {
        rows(&facade, FOLDER, &[("emailAccountId", &account), ("folderPath", path)])
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("no folder {path}"))
    };
    let kind_of = |path: &str| field(&by_path(path), "folderKindEnumId");

    // before the first sync: Archive is a folder made here; the account's empty Archive placeholder goes
    let kept = out_text(
        &ok(
            "create#EmailFolder",
            &[("emailAccountId", text(&account)), ("folderName", text("Kept"))],
        ),
        "emailFolderId",
    );
    let placeholder = folder_id(&facade, &account, "EfkArchive");
    let mapped = ok(
        "update#EmailFolder",
        &[("emailFolderId", text(&kept)), ("folderKindEnumId", text("EfkArchive"))],
    );
    assert_eq!(out_text(&mapped, "formerFolderId"), placeholder);
    assert_eq!(mapped.get("formerFolderDeleted"), Some(&Value::Bool(true)));
    assert!(rows(&facade, FOLDER, &[("emailFolderId", &placeholder)]).is_empty());
    sync(&account);
    let kept_row = one(&facade, FOLDER, "emailFolderId", &kept);
    assert_eq!(field(&kept_row, "folderKindEnumId"), "EfkArchive");
    assert_eq!(field(&kept_row, "kindByUser"), "Y");
    assert_eq!(field(&kept_row, "folderPath"), "Kept");
    // the server's \Archive folder is another folder here
    assert_eq!(kind_of("Archive"), "EfkOther");

    // after: Projects becomes the Sent folder; Sent Mail is another folder, and a new \Sent one does not take it
    let projects = field(&by_path("Projects"), "emailFolderId");
    let sent_mail = folder_id(&facade, &account, "EfkSent");
    let mapped = ok(
        "update#EmailFolder",
        &[
            ("emailFolderId", text(&projects)),
            ("folderKindEnumId", text("EfkSent")),
        ],
    );
    assert_eq!(out_text(&mapped, "formerFolderId"), sent_mail);
    assert_eq!(mapped.get("formerFolderDeleted"), Some(&Value::Bool(false)));
    imap.add_folder("Sent Items", "\\HasNoChildren \\Sent", true);
    imap.add(
        "Projects",
        &mail(
            "<p1@example.com>",
            "alice@example.com",
            "bob@example.com",
            "Plan",
            "",
            "The plan",
        ),
        &["\\Seen"],
    );
    sync(&account);
    assert_eq!(kind_of("Projects"), "EfkSent");
    assert_eq!(kind_of("Sent Mail"), "EfkOther");
    assert_eq!(kind_of("Sent Items"), "EfkOther");
    // the sync honors the mapping: mail in the Sent folder is sent mail
    assert_eq!(
        field(
            &by_message_id(&facade, &account, "<p1@example.com>").unwrap(),
            "statusId"
        ),
        "ES_SENT"
    );

    // the chosen folder gone from the server still holds its kind: a new \Archive folder is another folder
    lock(&imap.state).folders.retain(|folder| folder.name != "Kept");
    sync(&account);
    assert!(!field(&one(&facade, FOLDER, "emailFolderId", &kept), "remoteRemovedDate").is_empty());
    imap.add_folder("Old Mail", "\\HasNoChildren \\Archive", true);
    sync(&account);
    assert_eq!(kind_of("Old Mail"), "EfkOther");
    assert_eq!(
        field(&one(&facade, FOLDER, "emailFolderId", &kept), "folderKindEnumId"),
        "EfkArchive"
    );

    // an unsubscribed folder is not read, and is again once subscribed
    ok(
        "update#EmailFolder",
        &[("emailFolderId", text(&sent_mail)), ("subscribed", text("N"))],
    );
    imap.add(
        "Sent Mail",
        &mail(
            "<u1@example.com>",
            "alice@example.com",
            "bob@example.com",
            "Later",
            "",
            "Later",
        ),
        &["\\Seen"],
    );
    sync(&account);
    assert!(by_message_id(&facade, &account, "<u1@example.com>").is_none());
    ok(
        "update#EmailFolder",
        &[("emailFolderId", text(&sent_mail)), ("subscribed", Value::Bool(true))],
    );
    sync(&account);
    assert!(by_message_id(&facade, &account, "<u1@example.com>").is_some());

    // the server test shows the account's kinds over the server's special use
    let tested = ok(
        "test#EmailAccount",
        &[("emailAccountId", text(&account)), ("testSmtp", Value::Bool(false))],
    );
    let folders = maps(tested.get("folderList"));
    let sent_listed = folders
        .iter()
        .find(|folder| get(folder, "folderPath") == "Sent Mail")
        .expect("Sent Mail");
    assert_eq!(get(sent_listed, "folderKindEnumId"), "EfkOther");
    assert_eq!(get(sent_listed, "serverKindEnumId"), "EfkSent");
    assert_eq!(get(sent_listed, "emailFolderId"), sent_mail);

    // what does not change
    let inbox = folder_id(&facade, &account, "EfkInbox");
    refused(
        "update#EmailFolder",
        &[("emailFolderId", text(&inbox)), ("subscribed", text("N"))],
        "always synced",
    );
    refused(
        "update#EmailFolder",
        &[
            ("emailFolderId", text(&inbox)),
            ("folderKindEnumId", text("EfkArchive")),
        ],
        "stays the Inbox",
    );
    refused(
        "update#EmailFolder",
        &[
            ("emailFolderId", text(&projects)),
            ("folderKindEnumId", text("EfkInbox")),
        ],
        "INBOX",
    );
    refused(
        "update#EmailFolder",
        &[
            ("emailFolderId", text(&projects)),
            ("folderKindEnumId", text("EfkOther")),
        ],
        "keeps a Sent folder",
    );
    user(&facade, "mallory");
    login("mallory");
    refused(
        "update#EmailFolder",
        &[("emailFolderId", text(&projects)), ("subscribed", text("N"))],
        "not found",
    );
}

/// The automatic reply's audience: everyone, internal senders and the owner's contacts (by the hook of the theme value,
/// or of the property, which wins; every sender with no hook, none outside with a failing one), or internal only.
#[test]
fn the_automatic_reply_goes_to_its_audience() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    let dir = stub_dir("audience");
    install_with(Some(&dir));
    alice(&facade);
    let account = out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("emailAddress", text("alice@home.example.net")),
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
        ],
    );
    let sink = crate::service::SinkGuard::install();
    let mut round = 0;
    let mut answered = |audience: &str| -> Vec<String> {
        ok(
            "store#EmailOutOfOffice",
            &[
                ("emailAccountId", text(&account)),
                ("fromDate", stamp(now() - DAY)),
                ("bodyHtml", text("<p>Away</p>")),
                ("replyAudienceEnumId", text(audience)),
            ],
        );
        round += 1;
        let ids: Vec<String> = [
            "ada@friends.example.org",
            "zed@strangers.example.org",
            "dave@acme.example.com",
        ]
        .iter()
        .enumerate()
        .map(|(index, from)| {
            let id = format!("A{round}_{index}");
            message(
                &facade,
                &account,
                &id,
                "EfkInbox",
                60_000,
                &[
                    ("fromAddress", text(from)),
                    ("toAddresses", text("alice@home.example.net")),
                ],
            );
            id
        })
        .collect();
        let before = sink.sent().len();
        {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            ok(
                "send#OutOfOfficeReplies",
                &[
                    ("emailAccountId", text(&account)),
                    ("emailMessageIdList", list(&id_refs)),
                ],
            );
        }
        sink.sent()[before..].iter().map(|mail| mail.to.join(",")).collect()
    };
    let everyone = [
        "ada@friends.example.org",
        "zed@strangers.example.org",
        "dave@acme.example.com",
    ];

    // no hook: every sender is a contact
    assert_eq!(answered("EraContacts"), everyone);
    let state = ok("get#EmailOutOfOffice", &[("emailAccountId", text(&account))]);
    let reply = match state.get("outOfOffice") {
        Some(Value::Map(map)) => map.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(get(&reply, "replyAudienceEnumId"), "EraContacts");
    assert_eq!(state.get("contactHookService"), Some(&Value::Null));

    // the theme names the hook: contacts and internal senders
    row(
        &facade,
        "moqui.screen.ScreenThemeResource",
        &[
            ("screenThemeId", text("DEFAULT_QUASAR")),
            ("sequenceNum", Value::I64(991)),
            ("resourceTypeEnumId", text("STRT_EMAIL_CONTACT_SERVICE")),
            ("resourceValue", text("test.MailStub.filter#Contacts")),
        ],
    );
    assert_eq!(
        answered("EraContacts"),
        ["ada@friends.example.org", "dave@acme.example.com"]
    );
    let state = ok("get#EmailOutOfOffice", &[("emailAccountId", text(&account))]);
    assert_eq!(out_text(&state, "contactHookService"), "test.MailStub.filter#Contacts");
    let filtered = ok(
        "filter#ContactAddresses",
        &[(
            "emailAddressList",
            list(&["Ada@Friends.example.org", "zed@strangers.example.org"]),
        )],
    );
    let found: Vec<String> = match filtered.get("contactAddressList") {
        Some(Value::List(items)) => items.iter().map(to_plain_string).collect(),
        other => panic!("{other:?}"),
    };
    assert_eq!(found, ["ada@friends.example.org"]);
    // another user's contacts only for internal work
    refused(
        "filter#ContactAddresses",
        &[("userId", text("bob")), ("emailAddressList", list(&["x@y.z"]))],
        "only with authorization disabled",
    );

    // internal senders only
    assert_eq!(answered("EraInternal"), ["dave@acme.example.com"]);
    // everyone
    assert_eq!(answered("EraEveryone"), everyone);

    // the property wins over the theme; a hook that fails counts no one outside as a contact
    {
        let _property = Property::set("email_contact_service", "test.MailStub.fail#Contacts");
        assert_eq!(answered("EraContacts"), ["dave@acme.example.com"]);
        let failed = ok("filter#ContactAddresses", &[("emailAddressList", list(&["x@y.z"]))]);
        assert_eq!(failed.get("hookFailed"), Some(&Value::Bool(true)));
        assert_eq!(out_text(&failed, "hookService"), "test.MailStub.fail#Contacts");
    }
    refused(
        "store#EmailOutOfOffice",
        &[
            ("emailAccountId", text(&account)),
            ("bodyHtml", text("<p>Away</p>")),
            ("replyAudienceEnumId", text("EraPigeons")),
        ],
        "Unknown automatic reply audience",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A rule is tried on the newest mail of a folder without acting: a saved one, an edit of it, and one not yet saved; by
/// a reader too, but a saved rule only by one who may change it.
#[test]
fn a_rule_is_tried_on_recent_mail_without_acting() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install_with(None);
    user(&facade, "bob");
    alice(&facade);
    let account = out_text(
        &ok("create#EmailAccount", &[("emailAddress", text("alice@example.com"))]),
        "emailAccountId",
    );
    let bills = out_text(
        &ok(
            "create#EmailFolder",
            &[("emailAccountId", text(&account)), ("folderName", text("Bills"))],
        ),
        "emailFolderId",
    );
    message(
        &facade,
        &account,
        "R1",
        "EfkInbox",
        3 * DAY,
        &[("subject", text("Invoice 1"))],
    );
    message(
        &facade,
        &account,
        "R2",
        "EfkInbox",
        2 * DAY,
        &[("subject", text("Lunch")), ("fromAddress", text("lunch@x.example.org"))],
    );
    message(
        &facade,
        &account,
        "R3",
        "EfkInbox",
        DAY,
        &[("subject", text("Invoice 3")), ("rootEmailMessageId", text("R1"))],
    );
    message(
        &facade,
        &account,
        "R4",
        "EfkInbox",
        60_000,
        &[("subject", text("Invoice draft")), ("statusId", text("ES_DRAFT"))],
    );
    let mut actions = IndexMap::new();
    actions.insert("action".to_string(), text("moveToFolder"));
    actions.insert("value".to_string(), text(&bills));
    let rule = out_text(
        &ok(
            "create#EmailRule",
            &[
                ("ruleName", text("Bills")),
                ("emailAccountId", text(&account)),
                ("conditionList", conditions(&[("subject", "contains", "invoice")])),
                ("actionList", Value::List(vec![Value::Map(actions)])),
            ],
        ),
        "emailRuleId",
    );
    let ids = |out: &BTreeMap<String, Value>| -> Vec<String> {
        maps(out.get("matchList"))
            .iter()
            .map(|found| get(found, "emailMessageId"))
            .collect()
    };
    let tried = ok("test#EmailRule", &[("emailRuleId", text(&rule))]);
    assert_eq!(ids(&tried), ["R3", "R1"], "{tried:?}");
    assert_eq!(out_number(&tried, "testedCount"), 3);
    assert_eq!(out_number(&tried, "matchedCount"), 2);
    assert_eq!(get(&maps(tried.get("matchList"))[0], "subject"), "Invoice 3");
    // each match carries what opens it in the Inbox: the store, its thread (R3 is a reply in R1's), account and folder
    let inbox = folder_id(&facade, &account, "EfkInbox");
    let opens = |found: &IndexMap<String, Value>| {
        [
            "store",
            "threadId",
            "rootEmailMessageId",
            "emailAccountId",
            "emailFolderId",
        ]
        .map(|name| get(found, name))
    };
    let matches = maps(tried.get("matchList"));
    assert_eq!(
        opens(&matches[0]),
        ["email", "R1", "R1", account.as_str(), inbox.as_str()]
    );
    assert_eq!(
        opens(&matches[1]),
        ["email", "R1", "R1", account.as_str(), inbox.as_str()]
    );
    // nothing moved and nothing counted
    assert_eq!(
        field(&one(&facade, MESSAGE, "emailMessageId", "R1"), "emailFolderId"),
        inbox
    );
    assert_eq!(
        field(
            &one(&facade, "moqui.basic.email.EmailRule", "emailRuleId", &rule),
            "matchedCount"
        ),
        "0"
    );
    // the newest only
    let newest = ok(
        "test#EmailRule",
        &[("emailRuleId", text(&rule)), ("limit", Value::I64(2))],
    );
    assert_eq!(ids(&newest), ["R3"]);
    assert_eq!(out_number(&newest, "testedCount"), 2);
    // an edit of the saved rule, and a rule not yet saved
    let edited = ok(
        "test#EmailRule",
        &[
            ("emailRuleId", text(&rule)),
            ("conditionsJson", text(r#"[{"field": "subject", "value": "lunch"}]"#)),
        ],
    );
    assert_eq!(ids(&edited), ["R2"]);
    let unsaved = ok(
        "test#EmailRule",
        &[
            ("emailAccountId", text(&account)),
            (
                "conditionList",
                conditions(&[
                    ("from", "is", "lunch@x.example.org"),
                    ("subject", "startsWith", "invoice 3"),
                ]),
            ),
            ("matchAllEnumId", text("ErmAny")),
        ],
    );
    assert_eq!(ids(&unsaved), ["R3", "R2"]);
    refused(
        "test#EmailRule",
        &[
            ("emailAccountId", text(&account)),
            ("conditionList", Value::List(Vec::new())),
        ],
        "needs a condition",
    );
    refused(
        "run#EmailRules",
        &[
            ("emailFolderId", text(&inbox)),
            ("conditionsJson", text(r#"[{"field": "subject", "value": "x"}]"#)),
        ],
        "only tried",
    );

    // a reader tries a rule on the account's mail, not alice's saved one
    ok(
        "add#EmailAccountUser",
        &[
            ("emailAccountId", text(&account)),
            ("userId", text("bob")),
            ("permissionEnumId", text("EapRead")),
        ],
    );
    login("bob");
    let read = ok(
        "test#EmailRule",
        &[
            ("emailFolderId", text(&inbox)),
            ("conditionList", conditions(&[("subject", "contains", "invoice")])),
        ],
    );
    assert_eq!(ids(&read), ["R3", "R1"]);
    refused("test#EmailRule", &[("emailRuleId", text(&rule))], "You may not try");
    user(&facade, "mallory");
    login("mallory");
    refused(
        "test#EmailRule",
        &[
            ("emailAccountId", text(&account)),
            ("conditionList", conditions(&[("subject", "contains", "invoice")])),
        ],
        "not found",
    );
}

/// An action an app adds to the rules (an EmailRuleAction enumeration naming its service in optionValue) is accepted in a
/// rule and run with the message, account, rule and owner; it may stop the rules; one that reports an error is logged and
/// the rule's other actions go on; an action no app makes is skipped; an unknown action is refused in a rule.
#[test]
fn an_app_action_of_a_rule_runs_the_service_its_enumeration_names() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    let dir = stub_dir("rule-actions");
    install_with(Some(&dir));
    alice(&facade);
    row(
        &facade,
        "moqui.basic.EnumerationType",
        &[
            ("enumTypeId", text("EmailRuleAction")),
            ("description", text("Email Rule Action")),
        ],
    );
    for (id, code, service) in [
        ("TestRecordIt", "recordIt", "test.MailStub.record#RuleAction"),
        ("TestFailIt", "failIt", "test.MailStub.fail#RuleAction"),
    ] {
        row(
            &facade,
            "moqui.basic.Enumeration",
            &[
                ("enumId", text(id)),
                ("enumTypeId", text("EmailRuleAction")),
                ("enumCode", text(code)),
                ("optionValue", text(service)),
                ("description", text(code)),
            ],
        );
    }
    let account = out_text(
        &ok("create#EmailAccount", &[("emailAddress", text("alice@example.com"))]),
        "emailAccountId",
    );
    message(
        &facade,
        &account,
        "T1",
        "EfkInbox",
        3 * DAY,
        &[("subject", text("Ticket one"))],
    );
    message(
        &facade,
        &account,
        "T2",
        "EfkInbox",
        2 * DAY,
        &[("subject", text("Ticket halt"))],
    );
    message(&facade, &account, "T3", "EfkInbox", DAY, &[("subject", text("Lunch"))]);
    let action = |name: &str, value: &str| {
        let mut map = IndexMap::new();
        map.insert("action".to_string(), text(name));
        if !value.is_empty() {
            map.insert("value".to_string(), text(value));
        }
        Value::Map(map)
    };
    // a rule with an app's action is accepted; an action nobody makes is not
    let halt = out_text(
        &ok(
            "create#EmailRule",
            &[
                ("ruleName", text("Halt")),
                ("emailAccountId", text(&account)),
                ("sequenceNum", Value::I64(1)),
                ("conditionList", conditions(&[("subject", "contains", "halt")])),
                ("actionList", Value::List(vec![action("recordIt", "stop")])),
            ],
        ),
        "emailRuleId",
    );
    let tickets = out_text(
        &ok(
            "create#EmailRule",
            &[
                ("ruleName", text("Tickets")),
                ("emailAccountId", text(&account)),
                ("sequenceNum", Value::I64(2)),
                ("conditionList", conditions(&[("subject", "contains", "ticket")])),
                (
                    "actionList",
                    Value::List(vec![action("recordIt", "P1"), action("markRead", "")]),
                ),
            ],
        ),
        "emailRuleId",
    );
    refused(
        "create#EmailRule",
        &[
            ("ruleName", text("Nope")),
            ("emailAccountId", text(&account)),
            ("conditionList", conditions(&[("subject", "contains", "x")])),
            ("actionList", Value::List(vec![action("nope", "")])),
        ],
        "Unknown rule action nope",
    );
    // an app action that fails, and one no app makes (written before the app left), each beside an action that goes on
    row(
        &facade,
        "moqui.basic.email.EmailRule",
        &[
            ("emailRuleId", text("LUNCH")),
            ("ownerUserId", text("alice")),
            ("emailAccountId", text(&account)),
            ("ruleName", text("Lunch")),
            ("sequenceNum", Value::I64(3)),
            ("enabled", text("Y")),
            (
                "conditionsJson",
                text(r#"[{"field": "subject", "operator": "contains", "value": "lunch"}]"#),
            ),
            (
                "actionsJson",
                text(r#"[{"action": "failIt"}, {"action": "createTask"}, {"action": "flag"}]"#),
            ),
        ],
    );
    let inbox = folder_id(&facade, &account, "EfkInbox");
    let ran = ok("run#EmailRules", &[("emailFolderId", text(&inbox))]);
    let message_of = |id: &str| one(&facade, MESSAGE, "emailMessageId", id);
    assert_eq!(
        field(&message_of("T1"), "snippet"),
        format!("P1|account|{tickets}|alice"),
        "{ran:?}"
    );
    assert_eq!(field(&message_of("T1"), "isRead"), "Y");
    // the halt rule's action stopped the rules for its message: Tickets did not run on it
    assert_eq!(
        field(&message_of("T2"), "snippet"),
        format!("stop|account|{halt}|alice")
    );
    assert_eq!(field(&message_of("T2"), "isRead"), "N");
    // the failure is the rule's, named; the action no app makes is skipped; the flag went on
    let rule_errors = out_text(&ran, "ruleErrorList");
    assert!(
        rule_errors.contains("Lunch") && rule_errors.contains("failIt") && rule_errors.contains("the app is away"),
        "{rule_errors}"
    );
    assert!(!rule_errors.contains("createTask"), "{rule_errors}");
    assert_eq!(field(&message_of("T3"), "isFlagged"), "Y");
    assert_eq!(field(&message_of("T3"), "snippet"), "Body of T3");
    assert_eq!(out_number(&ran, "matchedCount"), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A removed account whose mail was kept is listed to its owner only (includeRemoved) and deleted by them, with its mail.
#[test]
fn a_removed_account_is_deleted_by_its_owner_only() {
    let _lock = crate::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install_with(None);
    user(&facade, "bob");
    alice(&facade);
    let account = out_text(
        &ok("create#EmailAccount", &[("emailAddress", text("alice@example.com"))]),
        "emailAccountId",
    );
    message(&facade, &account, "K1", "EfkInbox", DAY, &[]);
    message(&facade, &account, "K2", "EfkInbox", 2 * DAY, &[]);
    ok(
        "add#EmailAccountUser",
        &[("emailAccountId", text(&account)), ("userId", text("bob"))],
    );
    let removed = ok("delete#EmailAccount", &[("emailAccountId", text(&account))]);
    assert_eq!(out_number(&removed, "keptMessageCount"), 2);
    let listed = |include: bool| -> Vec<IndexMap<String, Value>> {
        maps(ok("get#EmailAccountAccess", &[("includeRemoved", Value::Bool(include))]).get("accountList"))
            .into_iter()
            .filter(|item| get(item, "emailAccountId") == account)
            .collect()
    };
    assert!(listed(false).is_empty());
    let shown = listed(true);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].get("isRemoved"), Some(&Value::Bool(true)));
    assert_eq!(shown[0].get("canDelete"), Some(&Value::Bool(true)));
    assert_eq!(get(&shown[0], "level"), "0");
    assert_eq!(get(&shown[0], "keptMessageCount"), "2");
    let one_access = ok(
        "get#EmailAccountAccess",
        &[
            ("emailAccountId", text(&account)),
            ("includeRemoved", Value::Bool(true)),
        ],
    );
    assert_eq!(out_number(&one_access, "level"), 0);
    // nothing else is done with it
    refused(
        "update#EmailAccount",
        &[("emailAccountId", text(&account)), ("accountName", text("Again"))],
        "not found",
    );
    // a former member neither sees nor deletes it
    login("bob");
    let bobs = maps(ok("get#EmailAccountAccess", &[("includeRemoved", Value::Bool(true))]).get("accountList"));
    assert!(bobs.iter().all(|item| get(item, "emailAccountId") != account));
    refused(
        "delete#EmailAccount",
        &[("emailAccountId", text(&account))],
        "not found",
    );
    // its owner deletes it with its mail
    login("alice");
    let deleted = ok("delete#EmailAccount", &[("emailAccountId", text(&account))]);
    assert_eq!(out_number(&deleted, "deletedMessageCount"), 2);
    assert!(rows(&facade, ACCOUNT, &[("emailAccountId", &account)]).is_empty());
    assert!(rows(&facade, MESSAGE, &[("emailAccountId", &account)]).is_empty());
    assert!(listed(true).is_empty());
}

/// A progress message as a socket gets it: when, the message, and the users it is for.
type Seen = (std::time::Instant, IndexMap<String, Value>, Vec<String>);

/// A sync with mail to fetch sends its progress to the account's users, at most one a second, then one when done; a sync
/// with nothing to fetch sends none.
#[test]
fn a_sync_sends_its_progress() {
    let _lock = crate::user::session::cookie_property_lock();
    // two a session: the first sync fetches two of the three, the next the third
    let (ctx, facade) = setup_with(r#"<moqui-conf><email-client sync-batch="2"/></moqui-conf>"#);
    let _entered = ctx.enter();
    install_with(None);
    alice(&facade);
    let imap = FakeImap::start(&["UIDPLUS", "MOVE"]);
    for index in 1..=3 {
        imap.add(
            "INBOX",
            &mail(
                &format!("<g{index}@example.com>"),
                "x@example.com",
                "alice@example.com",
                &format!("Mail {index}"),
                "",
                "Hello",
            ),
            &[],
        );
    }
    let account = imap_account(imap.port, "app-password");
    // a listener on the notification topic, as the host's socket, keeping this account's progress
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    let wanted = account.clone();
    let subscription = crate::notify::subscribe(
        crate::notify::NOTIFICATION_MESSAGE,
        Arc::new(move |message: &crate::notify::TopicMessage| {
            let Value::Map(body) = moqui_lib::json::from_json(&message.body) else {
                return;
            };
            if body.get("topic").map(to_plain_string).as_deref() != Some(super::sync::PROGRESS_TOPIC) {
                return;
            }
            let Some(Value::Map(progress)) = body.get("message").cloned() else {
                return;
            };
            if progress.get("emailAccountId").map(to_plain_string).as_deref() != Some(wanted.as_str()) {
                return;
            }
            let users = match body.get(crate::notify::ROUTE_USER_IDS) {
                Some(Value::List(items)) => items.iter().map(to_plain_string).collect(),
                _ => Vec::new(),
            };
            lock(&kept).push((std::time::Instant::now(), progress, users));
        }),
    )
    .unwrap();
    let out = sync(&account);
    assert_eq!(out_number(&out, "newCount"), 2);
    let sent = lock(&seen).clone();
    assert!(sent.len() >= 2, "{sent:?}");
    let (_, first, users) = &sent[0];
    assert_eq!(first.get("done"), Some(&Value::Bool(false)), "{first:?}");
    assert_eq!(get(first, "folderName"), "Inbox");
    assert_eq!(get(first, "folderToFetch"), "2");
    assert_eq!(get(first, "folderPending"), "3");
    assert_eq!(get(first, "folderKept"), "0");
    assert_eq!(get(first, "toFetch"), "2");
    assert_eq!(get(first, "pending"), "3");
    assert!(!get(first, "folderCount").is_empty() && get(first, "folderCount") != "0");
    assert_eq!(users, &["alice".to_string()]);
    let (_, last, _) = sent.last().unwrap();
    assert_eq!(last.get("done"), Some(&Value::Bool(true)), "{last:?}");
    assert_eq!(get(last, "outcome"), "synced");
    assert_eq!(get(last, "newCount"), "2");
    assert_eq!(get(last, "fetched"), "2");
    // throttled: the messages before the last are a second apart
    let running: Vec<std::time::Instant> = sent[..sent.len() - 1].iter().map(|(at, _, _)| *at).collect();
    for pair in running.windows(2) {
        assert!(
            pair[1].duration_since(pair[0]) >= std::time::Duration::from_millis(990),
            "{sent:?}"
        );
    }
    // the next session: the rest, with what was kept
    lock(&seen).clear();
    assert_eq!(out_number(&sync(&account), "newCount"), 1);
    let sent = lock(&seen).clone();
    let (_, first, _) = &sent[0];
    assert_eq!(get(first, "folderKept"), "2", "{first:?}");
    assert_eq!(get(first, "folderPending"), "1");
    // nothing to fetch: nothing sent
    lock(&seen).clear();
    sync(&account);
    assert!(lock(&seen).is_empty(), "{:?}", lock(&seen));
    drop(subscription);
    let _ = facade;
}
