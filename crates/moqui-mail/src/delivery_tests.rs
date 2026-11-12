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

//! The SMTP listener's side of the mail client ([`super::delivery`]), as the host calls it with no user and
//! authorization disabled: routing by account and identity address, case and plus tag, and which accounts it takes;
//! delivery once per account with the trace fields, files, threading, rules and email ECAs once, a resend not doubled;
//! the gateway's spam verdict to Junk and only its Authentication-Results kept; a store that fails as that account's
//! error, and the steps after the store failure tolerant; DSN's Original-Recipient and "delivered" report.

use moqui_lib::object::Value;

use super::delivery::{
    deliver, find_recipient, Envelope, Options, Outcome, RecipientAccount, RecipientDsn, RecipientIdentity,
    RecipientMap, Recipients, Report,
};
use super::sync_tests::{
    alice, by_message_id, field, folder_id, install, map, now, ok, one, out_text, rows, setup, setup_inline, stamp,
    text, update,
};
use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;

const MESSAGE: &str = "moqui.basic.email.EmailMessage";
const ACCOUNT: &str = "moqui.basic.email.EmailAccount";

fn account(kind: &str, address: &str, more: &[(&str, Value)]) -> String {
    let mut parameters = vec![("accountKindEnumId", text(kind)), ("emailAddress", text(address))];
    parameters.extend(more.iter().cloned());
    out_text(&ok("create#EmailAccount", &parameters), "emailAccountId")
}

fn identity(account: &str, address: &str) {
    ok(
        "create#EmailIdentity",
        &[("emailAccountId", text(account)), ("emailAddress", text(address))],
    );
}

fn envelope(from: &str) -> Envelope {
    Envelope {
        mail_from: from.to_string(),
        helo: "gw.example.com".to_string(),
        client_ip: "10.0.0.5".to_string(),
        protocol: "ESMTP".to_string(),
        session_id: "S1".to_string(),
        hostname: "moqui.example.com".to_string(),
        ..Envelope::default()
    }
}

fn options() -> Options {
    Options {
        spam_headers: vec!["X-Spam-Flag".to_string(), "X-Spam-Status".to_string()],
        authserv_id: "gw.example.com".to_string(),
    }
}

fn to(account: &str, addresses: &[&str]) -> Recipients {
    Recipients {
        email_account_id: account.to_string(),
        addresses: addresses.iter().map(|address| (*address).to_string()).collect(),
        ..Recipients::default()
    }
}

/// `deliver` as the listener calls it: no user, authorization disabled.
fn receive(facade: &EntityFacade, raw: &str, from: &str, recipients: &[Recipients]) -> Result<Report, String> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    deliver(facade, raw.as_bytes(), &envelope(from), recipients, &options())
}

fn stored(report: &Report, account: &str) -> (String, bool) {
    let delivery = report
        .deliveries
        .iter()
        .find(|delivery| delivery.email_account_id == account)
        .unwrap_or_else(|| panic!("no delivery for {account}: {report:?}"));
    match &delivery.result {
        Ok(Outcome::Stored { email_message_id, junk }) => (email_message_id.clone(), *junk),
        other => panic!("{account}: {other:?}"),
    }
}

fn eca_lines() -> Vec<String> {
    ExecutionContext::with_current(|ec| ec.message().messages().to_vec())
        .unwrap_or_default()
        .into_iter()
        .filter(|line| line.starts_with("eca "))
        .collect()
}

fn install_ecas(ctx: &ExecutionContext) {
    moqui_core::service::install_eca_rules(
        ctx.factory(),
        "",
        r#"<emecas>
            <emeca rule-name="Fails"><condition><expression>true</expression></condition>
                <actions><script>throw new Error('rule boom')</script></actions></emeca>
            <emeca rule-name="Seen">
                <actions><script>ec.message.addMessage('eca ' + emailAccountId + ' ' + fields.subject)</script></actions>
            </emeca>
        </emecas>"#,
    );
}

const FILES: &str = "Return-Path: <forged@example.net>\r
Authentication-Results: gw.example.com; spf=pass smtp.mailfrom=example.org; dkim=pass header.d=example.org\r
Authentication-Results: evil.example; dmarc=pass\r
Message-ID: <files@example.org>\r
From: Carol <carol@example.org>\r
To: sales@example.com, help@example.com\r
Subject: Files\r
Date: Mon, 19 Oct 2026 11:00:00 +0000\r
MIME-Version: 1.0\r
Content-Type: multipart/mixed; boundary=\"mix\"\r
\r
--mix\r
Content-Type: text/plain; charset=utf-8\r
\r
The report is attached.\r
--mix\r
Content-Type: application/pdf; name=\"report.pdf\"\r
Content-Disposition: attachment; filename=\"report.pdf\"\r
Content-Transfer-Encoding: base64\r
\r
JVBERi0xLjQK\r
--mix--\r
";

fn plain(message_id: &str, subject: &str, to: &str, extra: &str) -> String {
    format!(
        "Message-ID: {message_id}\r\nFrom: Dan <dan@example.org>\r\nTo: {to}\r\nSubject: {subject}\r\n\
         Date: Mon, 19 Oct 2026 12:00:00 +0000\r\n{extra}\r\nHello\r\n"
    )
}

/// Hosted and shared accounts by their address and identities, in any case and with a plus tag; external accounts,
/// a shared account the sync feeds and a removed one are not the listener's; an address two accounts claim is the
/// first one's.
#[test]
fn recipients_are_hosted_and_shared_accounts_by_address_identity_and_plus_tag() {
    let _lock = moqui_core::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let sales = account("EakHosted", "Sales@Example.com", &[]);
    let support = account("EakShared", "support@example.com", &[]);
    identity(&sales, "orders@example.com");
    let external = account("EakExternal", "alice@example.com", &[]);
    let synced = account(
        "EakShared",
        "team@example.com",
        &[
            ("storeHost", text("imap.example.com")),
            ("mailPassword", text("secret")),
        ],
    );
    let removed = account("EakHosted", "old@example.com", &[]);
    update(
        &facade,
        ACCOUNT,
        ("emailAccountId", &removed),
        &[("statusId", text("EasRemoved"))],
    );
    let later = account("EakHosted", "later@example.com", &[]);
    identity(&later, "ORDERS@example.com");
    identity(&later, "external-alias@example.com");
    identity(&external, "external-alias@example.com");
    let find = |address: &str, plus: &str| {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        find_recipient(&facade, address, plus).unwrap()
    };
    assert_eq!(find("sales@example.com", "+"), Some(sales.clone()));
    assert_eq!(find("SALES@EXAMPLE.COM", "+"), Some(sales.clone()));
    assert_eq!(find("sales+news@example.com", "+"), Some(sales.clone()));
    assert_eq!(find("sales-news@example.com", "+-"), Some(sales.clone()));
    assert_eq!(find("sales+news@example.com", ""), None, "plus addressing off");
    assert_eq!(find("+news@example.com", "+"), None);
    assert_eq!(find("support@example.com", "+"), Some(support.clone()));
    assert_eq!(
        find("orders@example.com", "+"),
        Some(sales.clone()),
        "the first account with it"
    );
    assert_eq!(find("orders+x@example.com", "+"), Some(sales.clone()));
    assert_eq!(find("later@example.com", "+"), Some(later.clone()));
    assert_eq!(
        find("external-alias@example.com", "+"),
        Some(later),
        "not the external account's"
    );
    assert_eq!(find("alice@example.com", "+"), None, "an external account");
    assert_eq!(find("team@example.com", "+"), None, "a shared account the sync feeds");
    assert_eq!(find("old@example.com", "+"), None, "a removed account");
    assert_eq!(find("nobody@example.com", "+"), None);
    assert_eq!(find("postmaster", "+"), None);
    let _ = (synced, support);

    // the edge node's map from copies of the same rows answers alike
    let copies = recipient_map(&facade);
    for (address, plus) in [
        ("sales@example.com", "+"),
        ("SALES@EXAMPLE.COM", "+"),
        ("sales+news@example.com", "+"),
        ("sales-news@example.com", "+-"),
        ("sales+news@example.com", ""),
        ("+news@example.com", "+"),
        ("support@example.com", "+"),
        ("orders@example.com", "+"),
        ("orders+x@example.com", "+"),
        ("later@example.com", "+"),
        ("external-alias@example.com", "+"),
        ("alice@example.com", "+"),
        ("team@example.com", "+"),
        ("old@example.com", "+"),
        ("nobody@example.com", "+"),
        ("postmaster", "+"),
    ] {
        assert_eq!(
            copies.find(address, plus),
            find(address, plus),
            "{address} with {plus:?}"
        );
    }
}

/// A [`RecipientMap`] of every account, identity and server row, as the edge node builds one from its copies.
fn recipient_map(facade: &EntityFacade) -> RecipientMap {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let list = |entity: &str| facade.find(entity).unwrap().list().unwrap().values().to_vec();
    let stores: Vec<String> = list("moqui.basic.email.EmailServer")
        .iter()
        .filter(|row| !field(row, "storeHost").is_empty())
        .map(|row| field(row, "emailServerId"))
        .collect();
    let accounts: Vec<RecipientAccount> = list(ACCOUNT)
        .iter()
        .map(|row| RecipientAccount {
            email_account_id: field(row, "emailAccountId"),
            email_address: field(row, "emailAddress"),
            account_kind_enum_id: field(row, "accountKindEnumId"),
            status_id: field(row, "statusId"),
            has_store_server: stores.contains(&field(row, "emailServerId")),
        })
        .collect();
    let identities: Vec<RecipientIdentity> = list("moqui.basic.email.EmailIdentity")
        .iter()
        .map(|row| RecipientIdentity {
            email_identity_id: field(row, "emailIdentityId"),
            email_address: field(row, "emailAddress"),
            email_account_id: field(row, "emailAccountId"),
        })
        .collect();
    RecipientMap::new(&accounts, &identities)
}

/// One message to two accounts (one of them by two addresses) is stored once in each Inbox, with the trace fields,
/// the gateway's Authentication-Results only, its file as a DbResource, and its thread; the rules and the email ECAs
/// run once on each copy, a failing ECA logged and the rest going on; the gateway sending it again stores nothing.
#[test]
fn a_message_is_stored_once_per_account_with_files_rules_and_ecas() {
    let _lock = moqui_core::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    install_ecas(&ctx);
    let sales = account("EakHosted", "sales@example.com", &[]);
    let support = account("EakShared", "support@example.com", &[]);
    identity(&support, "help@example.com");
    let rule = out_text(
        &ok(
            "create#EmailRule",
            &[
                ("ruleName", text("Files")),
                (
                    "conditionList",
                    Value::List(vec![map(&[
                        ("field", "subject"),
                        ("operator", "contains"),
                        ("value", "Files"),
                    ])]),
                ),
                ("actionList", Value::List(vec![map(&[("action", "flag")])])),
            ],
        ),
        "emailRuleId",
    );
    moqui_core::user::logout().unwrap();

    let report = receive(
        &facade,
        FILES,
        "carol@example.org",
        &[
            to(&sales, &["sales@example.com", "Sales+x@example.com"]),
            to(&support, &["help@example.com"]),
            to(&sales, &["sales@example.com"]),
        ],
    )
    .unwrap();
    assert!(!report.store_failed(), "{report:?}");
    assert!(!report.spam);
    assert_eq!(report.authentication, "spf=pass dkim=pass");
    assert_eq!(report.deliveries.len(), 2, "once per account: {report:?}");
    for (account, addresses) in [
        (&sales, vec!["sales@example.com", "Sales+x@example.com"]),
        (&support, vec!["help@example.com"]),
    ] {
        let (id, junk) = stored(&report, account);
        assert!(!junk);
        let row = one(&facade, MESSAGE, "emailMessageId", &id);
        assert_eq!(field(&row, "emailFolderId"), folder_id(&facade, account, "EfkInbox"));
        assert_eq!(field(&row, "statusId"), "ES_RECEIVED");
        assert_eq!(field(&row, "isRead"), "N");
        assert_eq!(field(&row, "isFlagged"), "Y", "the rule ran");
        assert_eq!(field(&row, "hasAttachment"), "Y");
        assert_eq!(field(&row, "subject"), "Files");
        assert_eq!(field(&row, "fromAddress"), "carol@example.org");
        let headers = field(&row, "headersString");
        assert!(headers.starts_with("Return-Path: <carol@example.org>"), "{headers}");
        assert_eq!(headers.matches("Return-Path:").count(), 1, "{headers}");
        for address in &addresses {
            assert!(headers.contains(&format!("Delivered-To: {address}")), "{headers}");
        }
        assert!(
            headers.contains("by moqui.example.com (mr-moqui) with ESMTP id S1"),
            "{headers}"
        );
        assert_eq!(
            headers.contains("for <help@example.com>"),
            addresses.len() == 1,
            "{headers}"
        );
        assert!(headers.contains("Authentication-Results: gw.example.com;"), "{headers}");
        assert!(!headers.contains("evil.example"), "{headers}");
        let files = rows(
            &facade,
            "moqui.basic.email.EmailMessageAttachment",
            &[("emailMessageId", &id)],
        );
        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(field(&files[0], "fileName"), "report.pdf");
        let location = field(&files[0], "attachmentLocation");
        assert!(
            location.starts_with(&format!("dbresource://email/{account}/{id}/")),
            "{location}"
        );
        let bytes = ctx.factory().resource().location_bytes(&location).unwrap();
        assert_eq!(bytes.as_deref(), Some(&b"%PDF-1.4\n"[..]));
    }
    assert_eq!(
        field(
            &one(&facade, "moqui.basic.email.EmailRule", "emailRuleId", &rule),
            "matchedCount"
        ),
        "2"
    );
    let mut lines = eca_lines();
    lines.sort();
    let mut expected = vec![format!("eca {sales} Files"), format!("eca {support} Files")];
    expected.sort();
    assert_eq!(lines, expected);

    // a reply joins the thread
    let reply = receive(
        &facade,
        &plain(
            "<reply@example.org>",
            "Re: Files",
            "sales@example.com",
            "In-Reply-To: <files@example.org>\r\n",
        ),
        "dan@example.org",
        &[to(&sales, &["sales@example.com"])],
    )
    .unwrap();
    let (reply_id, _) = stored(&reply, &sales);
    let root = by_message_id(&facade, &sales, "<files@example.org>").unwrap();
    assert_eq!(
        field(
            &one(&facade, MESSAGE, "emailMessageId", &reply_id),
            "rootEmailMessageId"
        ),
        field(&root, "emailMessageId")
    );

    // the gateway sends it again (after a 451 for another account): not stored twice, nothing runs again
    let again = receive(
        &facade,
        FILES,
        "carol@example.org",
        &[to(&sales, &["sales@example.com"])],
    )
    .unwrap();
    assert_eq!(again.deliveries[0].result, Ok(Outcome::Duplicate));
    assert_eq!(
        rows(
            &facade,
            MESSAGE,
            &[("emailAccountId", &sales), ("messageId", "<files@example.org>")]
        )
        .len(),
        1
    );
    assert_eq!(eca_lines().len(), 3, "the reply's, once");
    // a message with no header fields is refused
    assert!(receive(
        &facade,
        "\r\njust a body\r\n",
        "x@example.org",
        &[to(&sales, &["sales@example.com"])]
    )
    .is_err());
}

/// The gateway's spam verdict files the message in Junk, where no rule, ECA or automatic reply runs on it; a verdict
/// of no leaves it in the Inbox.
/// With no email ECA rule, Inbox mail and what the account's rules do to it commit in one frame; when the rules fail
/// there (here a rule's write that fails at the commit: a create of a row that is there), the message is stored alone and the rules run after it on their own,
/// so a failing rule never loses mail.
#[test]
fn a_rule_that_fails_with_the_store_leaves_the_message_stored() {
    let _lock = moqui_core::user::session::cookie_property_lock();
    let ctx = moqui_core::context::ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open_with(
        &moqui_core::context::framework_dir(),
        moqui_core::testing::Options {
            entity_eca: true,
            ..Default::default()
        },
    );
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    moqui_core::entity::load_feed::add_eca_rules(
        &facade,
        r#"<eecas>
            <eeca id="FlagRefused" entity="moqui.basic.email.EmailMessage" on-update="true">
                <condition><expression>entityValue.isFlagged == 'Y'</expression></condition>
                <actions><script>
                    const again = ec.entity.makeValue('moqui.basic.email.EmailAccount')
                    again.emailAccountId = entityValue.emailAccountId
                    again.create()
                </script></actions>
            </eeca>
        </eecas>"#,
    )
    .unwrap();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let sales = account("EakHosted", "sales@example.com", &[]);
    let quiet = account("EakHosted", "quiet@example.com", &[]);
    ok(
        "create#EmailRule",
        &[
            ("ruleName", text("Flag all")),
            ("emailAccountId", text(&sales)),
            (
                "conditionList",
                Value::List(vec![map(&[
                    ("field", "subject"),
                    ("operator", "contains"),
                    ("value", "Files"),
                ])]),
            ),
            ("actionList", Value::List(vec![map(&[("action", "flag")])])),
        ],
    );
    moqui_core::user::logout().unwrap();
    let frames = |address: &str, account: &str| {
        let before = moqui_core::entity::cache::tx_cache_stats();
        let report = receive(&facade, FILES, "carol@example.org", &[to(account, &[address])]).unwrap();
        assert!(!report.store_failed(), "{report:?}");
        let (id, _) = stored(&report, account);
        let after = moqui_core::entity::cache::tx_cache_stats();
        (id, after.frames - before.frames)
    };
    // an account with no rule: the message and its (no) rules in one frame
    let (_, together) = frames("quiet@example.com", &quiet);
    // the rule's write fails the commit: the message is stored again alone, in a frame of its own
    let (id, apart) = frames("sales@example.com", &sales);
    assert!(
        apart > together,
        "the store was done again alone, and the rules apart: {apart} frames, {together}"
    );
    let row = one(&facade, MESSAGE, "emailMessageId", &id);
    assert_eq!(field(&row, "subject"), "Files");
    assert_eq!(field(&row, "emailAccountId"), sales);
    let messages = rows(&facade, MESSAGE, &[("emailAccountId", sales.as_str())]);
    assert_eq!(messages.len(), 1, "stored once");
}

/// A rule's forward is claimed in the frame that stores the message and runs its rules, and sent once that commits:
/// when the frame fails after the rules ran, the message is stored alone and the rules run again, and the forward goes
/// once (plans/archive/High-Level-Review.md HL-028). The send registered for the commit runs with no user, as the listener's
/// delivery has none.
#[test]
fn a_forward_whose_storing_frame_fails_after_the_rules_is_sent_once() {
    let _lock = moqui_core::user::session::cookie_property_lock();
    let (ctx, facade) = setup_inline();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let sales = account(
        "EakHosted",
        "sales@example.com",
        &[
            ("smtpHost", text("127.0.0.1")),
            ("smtpPort", text("2525")),
            ("smtpStartTls", text("N")),
        ],
    );
    ok(
        "create#EmailRule",
        &[
            ("ruleName", text("Keep plans")),
            ("emailAccountId", text(&sales)),
            (
                "conditionList",
                Value::List(vec![map(&[
                    ("field", "subject"),
                    ("operator", "contains"),
                    ("value", "Plan"),
                ])]),
            ),
            (
                "actionList",
                Value::List(vec![map(&[("action", "forwardTo"), ("value", "keep@example.org")])]),
            ),
        ],
    );
    moqui_core::user::logout().unwrap();
    let sink = super::SinkGuard::install();
    super::delivery::FAIL_AFTER_RULES.with(|fail| fail.set(true));
    let report = receive(
        &facade,
        &plain("<plan@example.org>", "Plan", "sales@example.com", ""),
        "dan@example.org",
        &[to(&sales, &["sales@example.com"])],
    )
    .unwrap();
    assert!(
        !super::delivery::FAIL_AFTER_RULES.with(std::cell::Cell::get),
        "the frame with the rules failed"
    );
    let (id, _) = stored(&report, &sales);
    assert!(sink.sent().is_empty(), "nothing is sent before a commit");
    ctx.factory().run_pending();
    let sent = sink.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].to, ["keep@example.org"]);
    assert_eq!(sent[0].subject, "Fwd: Plan");
    let forwards = rows(&facade, MESSAGE, &[("parentEmailMessageId", id.as_str())]);
    assert_eq!(forwards.len(), 1, "the first frame's forward went with it");
    assert_eq!(field(&forwards[0], "statusId"), "ES_SENT");
    ctx.factory().run_pending();
    assert_eq!(sink.sent().len(), 1, "sent once");
}

#[test]
fn the_gateways_spam_verdict_files_in_junk() {
    let _lock = moqui_core::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    install_ecas(&ctx);
    let sales = account("EakHosted", "sales@example.com", &[]);
    moqui_core::user::logout().unwrap();
    let report = receive(
        &facade,
        &plain(
            "<spam@example.org>",
            "Win",
            "sales@example.com",
            "X-Spam-Flag: YES\r\nX-Spam-Status: Yes, score=9.1\r\n",
        ),
        "dan@example.org",
        &[to(&sales, &["sales@example.com"])],
    )
    .unwrap();
    assert!(report.spam);
    let (id, junk) = stored(&report, &sales);
    assert!(junk);
    assert_eq!(
        field(&one(&facade, MESSAGE, "emailMessageId", &id), "emailFolderId"),
        folder_id(&facade, &sales, "EfkJunk")
    );
    assert!(eca_lines().is_empty());
    let report = receive(
        &facade,
        &plain(
            "<ham@example.org>",
            "Lunch",
            "sales@example.com",
            "X-Spam-Status: No, score=0.2\r\n",
        ),
        "dan@example.org",
        &[to(&sales, &["sales@example.com"])],
    )
    .unwrap();
    let (id, junk) = stored(&report, &sales);
    assert!(!junk && !report.spam);
    assert_eq!(
        field(&one(&facade, MESSAGE, "emailMessageId", &id), "emailFolderId"),
        folder_id(&facade, &sales, "EfkInbox")
    );
    assert_eq!(eca_lines().len(), 1);
}

/// A store that fails is that account's error (the listener's 451) and the other account keeps its copy; after the
/// store, a failing ECA is logged and the automatic reply still goes.
#[test]
fn a_failed_store_is_that_accounts_error_and_later_steps_are_failure_tolerant() {
    let _lock = moqui_core::user::session::cookie_property_lock();
    let (ctx, facade) = setup_inline();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    install_ecas(&ctx);
    let broken = account("EakHosted", "broken@example.com", &[]);
    let support = account(
        "EakShared",
        "support@example.com",
        &[
            ("smtpHost", text("127.0.0.1")),
            ("smtpPort", text("2525")),
            ("smtpStartTls", text("N")),
        ],
    );
    ok(
        "store#EmailOutOfOffice",
        &[
            ("emailAccountId", text(&support)),
            ("fromDate", stamp(now() - 86_400_000)),
            ("bodyHtml", text("<p>We answer within a day</p>")),
        ],
    );
    let inbox = folder_id(&facade, &broken, "EfkInbox");
    {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let mut row = facade.make_value("moqui.basic.email.EmailFolder").unwrap();
        row.set("emailFolderId", text(&inbox));
        row.delete(&facade).unwrap();
    }
    moqui_core::user::logout().unwrap();
    let sink = super::SinkGuard::install();
    let report = receive(
        &facade,
        &plain(
            "<ask@example.org>",
            "Question",
            "broken@example.com, support@example.com",
            "",
        ),
        "dan@example.org",
        &[
            to(&broken, &["broken@example.com"]),
            to(&support, &["support@example.com"]),
        ],
    )
    .unwrap();
    assert!(report.store_failed());
    let failed = report
        .deliveries
        .iter()
        .find(|delivery| delivery.email_account_id == broken)
        .unwrap();
    assert!(
        failed
            .result
            .as_ref()
            .is_err_and(|text| text.contains("no Inbox folder")),
        "{failed:?}"
    );
    let (id, _) = stored(&report, &support);
    assert_eq!(
        field(&one(&facade, MESSAGE, "emailMessageId", &id), "subject"),
        "Question"
    );
    assert_eq!(eca_lines(), [format!("eca {support} Question")], "the other rule ran");
    ctx.factory().run_pending();
    let sent = sink.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].to, ["dan@example.org"]);
    assert!(rows(&facade, MESSAGE, &[("emailAccountId", &broken)]).is_empty());
}

/// A recipient with NOTIFY=SUCCESS gets one "delivered" report through its account's server, and an ORCPT its
/// Original-Recipient field; the gateway's resend (a duplicate) and a recipient that did not ask tell nothing.
#[test]
fn a_delivered_report_goes_to_the_sender_for_notify_success() {
    let _lock = moqui_core::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let server = [
        ("smtpHost", text("127.0.0.1")),
        ("smtpPort", text("2525")),
        ("smtpStartTls", text("N")),
    ];
    let sales = account("EakHosted", "sales@example.com", &server);
    let support = account("EakShared", "support@example.com", &server);
    moqui_core::user::logout().unwrap();
    let sink = super::SinkGuard::install();
    let recipients = [
        Recipients {
            dsn: vec![RecipientDsn {
                address: "sales@example.com".to_string(),
                notify: "SUCCESS,FAILURE".to_string(),
                original: "rfc822;Sales+news@example.com".to_string(),
            }],
            ..to(&sales, &["sales@example.com"])
        },
        Recipients {
            dsn: vec![RecipientDsn {
                address: "support@example.com".to_string(),
                notify: "FAILURE".to_string(),
                original: String::new(),
            }],
            ..to(&support, &["support@example.com"])
        },
    ];
    let raw = plain(
        "<dsn@example.org>",
        "Tell me",
        "sales@example.com, support@example.com",
        "",
    );
    let envelope = Envelope {
        dsn_envid: "QQ314159".to_string(),
        ..envelope("dan@example.org")
    };
    let delivered = || {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        deliver(&facade, raw.as_bytes(), &envelope, &recipients, &options()).unwrap()
    };
    let report = delivered();
    let (id, _) = stored(&report, &sales);
    let headers = field(&one(&facade, MESSAGE, "emailMessageId", &id), "headersString");
    assert!(
        headers.contains("Original-Recipient: rfc822;Sales+news@example.com"),
        "{headers}"
    );
    let sent = sink.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].to, ["dan@example.org"]);
    assert_eq!(sent[0].subject, "Delivery Status Notification (delivered)");
    let report_text = &sent[0].body_text;
    assert!(
        report_text.contains("Original-Envelope-Id: QQ314159\r\n"),
        "{report_text}"
    );
    assert!(
        report_text.contains("Final-Recipient: rfc822; sales@example.com\r\nAction: delivered\r\n"),
        "{report_text}"
    );
    assert!(!report_text.contains("support@example.com\r\nAction"), "{report_text}");
    assert!(report_text.contains("Subject: Tell me\r\n"), "the header is returned");
    let again = delivered();
    assert!(again
        .deliveries
        .iter()
        .all(|delivery| delivery.result == Ok(Outcome::Duplicate)));
    assert_eq!(sink.sent().len(), 1, "a resend tells nothing again");
}
