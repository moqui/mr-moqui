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

//! What an edge node keeps in memory, made right on every member of an edge cluster (plans/Edge-Node2.md §4.6, E18
//! part B), on `edge_cluster`'s two edge members of one database and its local node. Two servers of one process share
//! the notification bus, so a cluster notice reaches the other member on it as well as by the cluster's transport.
//!
//! - A host name removed on one member is no longer answered by the other, and one added there is answered here, at
//!   once (`edge-state`). An item queued on both members with one idempotency key, as a network's webhook repeated
//!   to the other member, is one item.
//! - A node with the pairing's key alone, or one naming a linked member's id with another key, is refused on one edge
//!   member while a member is linked on another (that member's `EdgeMemberLinks` row), and a new node takes the
//!   pairing over once none is linked anywhere. A revoke whose notice was missed closes the links at the next beat.
//! - A local member links to both edge members with no takeover; a batch applied on one moves the other's count of the
//!   set, so what it built from the copies is built again; a revoke on one member closes the links on both.

#![cfg(all(unix, feature = "edge", feature = "ssh", feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_core::edge::LinkState;
use moqui_host::edge_link::{Dialer, LocalConfig, MemberKey};
use moqui_lib::object::{to_plain_string, Value};
use tokio::sync::watch;

use crate::edge_cluster::{get, in_context, links, EdgeCluster, NODE, SHOP};
use crate::edge_link::{quick, wait_status};
use crate::edge_web::{call, wait_until};

const CLIENT: &str = "moqui.edge.EdgeClient";
const MEMBER_LINKS: &str = "moqui.edge.EdgeMemberLinks";

/// `org.moqui.edge.EdgeServices.{verb_noun}` on `server` as john.doe: its errors, if any.
fn call_on(server: &moqui_host::Server, verb_noun: &str, parameters: &[(&str, &str)]) -> Result<(), String> {
    let factory = server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let _authz = context.disable_authz();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
        .collect();
    moqui_core::service::sync(&format!("org.moqui.edge.EdgeServices.{verb_noun}"), &parameters)
        .map_err(|err| err.to_string())?;
    let errors =
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("{errors:?}"))
    }
}

/// A field of the pairing's `EdgeClient` row.
fn client_field(server: &moqui_host::Server, field: &str) -> String {
    in_context(server, |facade| {
        facade
            .find(CLIENT)
            .unwrap()
            .eq("pairedNodeId", Value::string(NODE))
            .use_cache(false)
            .one()
            .unwrap()
            .and_then(|row| row.get(field).map(to_plain_string))
            .unwrap_or_default()
    })
}

/// The edge members whose `EdgeMemberLinks` rows name local member `member` of the pairing.
fn linked_on(server: &moqui_host::Server, member: &str) -> Vec<String> {
    in_context(server, |facade| {
        let mut edges: Vec<String> = facade
            .find(MEMBER_LINKS)
            .unwrap()
            .use_cache(false)
            .list()
            .unwrap()
            .values()
            .iter()
            .filter(|row| {
                let json = to_plain_string(row.get("linksJson").unwrap_or(&Value::Null));
                serde_json::from_str::<BTreeMap<String, Vec<String>>>(&json).is_ok_and(|links| {
                    links
                        .get(NODE)
                        .is_some_and(|members| members.iter().any(|m| m == member))
                })
            })
            .map(|row| to_plain_string(row.get("edgeInstanceId").unwrap_or(&Value::Null)))
            .collect();
        edges.sort();
        edges
    })
}

/// Write the row of an edge member that is not one of the two, naming `members` of the pairing linked there.
fn row_elsewhere(server: &moqui_host::Server, members: &[&str]) {
    in_context(server, |facade| {
        let mut row = facade.make_value(MEMBER_LINKS).unwrap();
        row.set("edgeInstanceId", Value::string("edge-elsewhere"));
        row.set(
            "linksJson",
            Value::string(serde_json::json!({ NODE: members }).to_string()),
        );
        row.set("heartbeatDate", moqui_core::user::now_value());
        row.create_or_update(facade).unwrap();
    });
}

/// Set a field of the pairing's `EdgeClient` row directly, as another member's change whose notice was missed.
fn set_client_field(server: &moqui_host::Server, field: &str, value: &str) {
    in_context(server, |facade| {
        let mut row = facade.make_value(CLIENT).unwrap();
        row.set("pairedNodeId", Value::string(NODE));
        row.set(field, Value::string(value));
        row.update(facade).unwrap();
    });
}

/// A dialer of the pairing's key on edge member `at`, as local member `id` with a key of its own.
fn stranger(cluster: &EdgeCluster, at: usize, id: &str) -> (Dialer, watch::Sender<bool>, std::path::PathBuf) {
    let runtime = crate::common::temp_dir(&format!("moqui-ecs-{id}"));
    std::fs::create_dir_all(&runtime).unwrap();
    let mut config = LocalConfig::new(&cluster.url(at), &cluster.paired.key, &cluster.paired.pin, id);
    quick(&mut config);
    config.member_key = Some(MemberKey::load_or_create(&runtime).unwrap());
    let (stop, shutdown) = watch::channel(false);
    let dialer = Dialer::start(config, cluster.rt.handle(), shutdown, None).unwrap();
    (dialer, stop, runtime)
}

fn refused_to_join(dialer: &Dialer, what: &str) {
    wait_status(dialer, Duration::from_secs(10), what, |status| {
        status.last_error.contains("join the cluster first")
            || status
                .members
                .iter()
                .any(|member| member.last_error.contains("join the cluster first"))
    });
    assert!(
        links(&dialer.status())
            .values()
            .all(|(state, _)| *state != LinkState::Linked),
        "{what}: not linked"
    );
}

#[test]
fn a_host_change_on_one_edge_member_is_served_by_the_other_and_a_repeated_item_is_one() {
    if !crate::common::own_process() {
        return;
    }
    let cluster = EdgeCluster::start("ecs-hosts", "front_edge");
    let second = &cluster.second.as_ref().unwrap().0;
    // removed on the second member: the first no longer answers it as the local node's (its offline page) at once
    call_on(second, "remove#EdgeHost", &[("hostName", SHOP)]).unwrap();
    wait_until(Duration::from_secs(5), "the host gone on the first member", || {
        get(&cluster.rt, &cluster.tls, cluster.ports[0], "/Login") != 503
    });
    // added again on the first: the second answers it
    call(
        &cluster.first,
        "add#EdgeHost",
        &[
            ("hostName", SHOP),
            ("pairedNodeId", NODE),
            ("modeEnumId", "EdgeHostTerminate"),
        ],
    )
    .unwrap();
    wait_until(Duration::from_secs(5), "the host back on the second member", || {
        get(&cluster.rt, &cluster.tls, cluster.ports[1], "/Login") == 503
    });

    // a network's webhook repeated to the other member: its idempotency key makes it one item
    let item = moqui_core::edge::queue::NewItem {
        kind: "echo".to_string(),
        conversation_key: "bridge-1".to_string(),
        idempotency_key: "chatWebhook:B1:ev-1".to_string(),
        payload: Some("{}".to_string()),
        content: None,
        content_location: None,
    };
    let enqueue = |server: &moqui_host::Server| {
        in_context(server, |facade| {
            moqui_core::edge::queue::enqueue(server.factory(), facade, Some(NODE), &item).unwrap()
        })
    };
    let first = enqueue(&cluster.first.server);
    let again = enqueue(second);
    assert!(first.created);
    assert!(!again.created, "the repeat on the other member");
    assert_eq!(first.queue_item_id, again.queue_item_id);
    let items = in_context(&cluster.first.server, |facade| {
        facade
            .find("moqui.edge.EdgeQueueItem")
            .unwrap()
            .eq("idempotencyKey", Value::string("chatWebhook:B1:ev-1"))
            .use_cache(false)
            .list()
            .unwrap()
            .len()
    });
    assert_eq!(items, 1);
}

#[test]
fn a_key_holder_is_refused_while_a_member_is_linked_on_another_edge_member() {
    if !crate::common::own_process() {
        return;
    }
    let cluster = EdgeCluster::start("ecs-admit", "front_edge");
    // a member of the local node linked on another edge member (its row, fresh), its key pinned; none linked on these
    set_client_field(
        &cluster.first.server,
        "memberKeys",
        &serde_json::json!([[NODE, "a-key-not-the-strangers"]]).to_string(),
    );
    row_elsewhere(&cluster.first.server, &[NODE]);
    // a node with the pairing's key and a key of its own, never linked: refused, not a takeover
    let (dialer, stop, runtime) = stranger(&cluster, 1, "stranger-1");
    refused_to_join(&dialer, "the stranger refused");
    dialer.stop();
    drop(stop);
    // one naming the linked member's id with another key: refused too
    let (impostor, impostor_stop, impostor_runtime) = stranger(&cluster, 1, NODE);
    refused_to_join(&impostor, "the impostor refused");
    impostor.stop();
    drop(impostor_stop);
    assert_eq!(client_field(&cluster.first.server, "takeoverToId"), "", "no takeover");
    // once no member is linked anywhere, the key admits a new node, as a takeover
    in_context(&cluster.first.server, |facade| {
        facade
            .find(MEMBER_LINKS)
            .unwrap()
            .eq("edgeInstanceId", Value::string("edge-elsewhere"))
            .delete_all()
            .unwrap();
    });
    let (dialer, stop, runtime2) = stranger(&cluster, 1, "stranger-2");
    wait_until(Duration::from_secs(15), "the takeover", || {
        client_field(&cluster.first.server, "takeoverToId") == "stranger-2"
    });
    dialer.stop();
    drop(stop);
    for dir in [runtime, impostor_runtime, runtime2] {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A revoke made without a notice (one this member missed) closes the links at the next beat on every member.
#[test]
fn a_revoke_whose_notice_was_missed_closes_the_links_at_the_next_beat() {
    if !crate::common::own_process() {
        return;
    }
    let cluster = EdgeCluster::start("ecs-missed", "front_edge");
    let learned = Arc::new(Mutex::new(Vec::new()));
    let (dialer, _stop) = cluster.dial(cluster.config(&cluster.url(0), &learned));
    cluster.wait_linked(&dialer, &[0, 1], "a link to each edge member");
    set_client_field(&cluster.first.server, "statusId", "EdgeClientRevoked");
    wait_status(&dialer, Duration::from_secs(12), "no link up", |status| {
        links(status).values().all(|(state, _)| *state != LinkState::Linked)
    });
    dialer.stop();
}

#[test]
fn a_member_links_to_both_edge_members_a_batch_moves_both_and_a_revoke_closes_both() {
    if !crate::common::own_process() {
        return;
    }
    let cluster = EdgeCluster::start("ecs-links", "front_edge");
    let second = &cluster.second.as_ref().unwrap().0;
    let learned = Arc::new(Mutex::new(Vec::new()));
    let (dialer, _stop) = cluster.dial(cluster.config(&cluster.url(0), &learned));
    cluster.wait_linked(&dialer, &[0, 1], "a link to each edge member");
    // its link to the second edge member is another of its links, not a takeover; each edge member's row names it
    assert_eq!(client_field(&cluster.first.server, "takeoverToId"), "");
    let mut both = cluster.ids.to_vec();
    both.sort();
    wait_until(Duration::from_secs(5), "a row from each edge member", || {
        linked_on(&cluster.first.server, NODE) == both
    });

    // a batch applied on whichever member carries the work moves the set's count on both
    let counts = |set: &str| {
        [&cluster.first.server, second]
            .map(|server| in_context(server, |facade| moqui_core::edge::replica::applied_count(facade, set)))
    };
    wait_until(Duration::from_secs(10), "the set sent whole", || {
        counts("groups").iter().all(|count| *count > 0)
    });
    let before = counts("groups");
    in_context(&cluster.local.server, |facade| {
        let mut group = facade.make_value("moqui.security.UserGroup").unwrap();
        group.set("userGroupId", Value::string("ECS_GROUP"));
        group.set("description", Value::string("Seen by both"));
        group.create(facade).unwrap();
    });
    wait_until(
        Duration::from_secs(10),
        "the batch counted on both edge members",
        || {
            let now = counts("groups");
            now[0] > before[0] && now[1] > before[1]
        },
    );

    // revoked on the first member: the links to both close
    call(&cluster.first, "revoke#EdgeClient", &[("pairedNodeId", NODE)]).unwrap();
    wait_status(&dialer, Duration::from_secs(10), "no link up", |status| {
        links(status).values().all(|(state, _)| *state != LinkState::Linked)
    });
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        links(&dialer.status())
            .values()
            .all(|(state, _)| *state != LinkState::Linked),
        "the second member does not take it again"
    );
    dialer.stop();
}

/// Mail in through an edge cluster (plans/Edge-Node2.md §4.6, §5 E18): each member's LMTP listener answers from the
/// recipients built from the shared copies, built again on a member when another member applied a batch of them.
#[cfg(feature = "inbound-smtp")]
mod mail {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use moqui_host::edge_link::{local_web, new_key, Dialer, LocalConfig, MemberKey};
    use moqui_lib::object::{to_plain_string, Value};
    use tokio::sync::watch;

    use super::in_context;
    use crate::common::{DirGuard, Gate};
    use crate::edge_link::{quick, runtime, wait_status};
    use crate::edge_mail::{lmtp, message};
    use crate::edge_web::{authority, issue, pair_node, wait_until};

    const NODE: &str = "local-ecm";
    const LOCAL_CONF: &str = r#"<edge queue-retry-seconds="0.1"/>
    <email-client idle-enabled="false"/>"#;
    const EDGE_CONF: &str = r#"<edge queue-retry-seconds="0.1"/>
    <email-client idle-enabled="false"/>
    <smtp-server protocol="lmtp"/>"#;

    /// A local node with hosted mail accounts, linked to both members of an edge cluster with an LMTP listener each.
    struct MailCluster {
        rt: tokio::runtime::Runtime,
        dialer: Option<Dialer>,
        _stop: watch::Sender<bool>,
        lmtp: [std::net::SocketAddr; 2],
        second: Option<(moqui_host::Server, DirGuard)>,
        first: Gate,
        local: Gate,
    }

    impl MailCluster {
        fn start(label: &str) -> Self {
            let authority = authority();
            let cert = issue(&authority, &["localhost"]);
            let local = crate::common::start_with_conf_files(&format!("{label}-local"), None, LOCAL_CONF, &[], &[]);
            account(&local, "sales@example.com");
            let link_key = new_key().unwrap();
            let first_port = crate::common::free_port();
            let first_url = format!("https://localhost:{first_port}");
            let (first_port_text, cluster_port) = (first_port.to_string(), crate::common::free_port().to_string());
            let mail = [
                ("smtp_listen_port", "0"),
                ("smtp_allowed_addresses", "127.0.0.1"),
                ("smtp_hostname", "edge.example.com"),
                ("edge_mail_mta", "postfix"),
            ];
            let files = [
                ("cert/default.crt", cert.0.as_str()),
                ("cert/default.key", cert.1.as_str()),
            ];
            let mut properties = vec![
                ("node_role", "front_edge"),
                ("webapp_https_listen_port", first_port_text.as_str()),
                ("edge_link_url", first_url.as_str()),
                ("edge_link_key", link_key.as_str()),
                ("cluster_https_port", cluster_port.as_str()),
            ];
            properties.extend_from_slice(&mail);
            let first = crate::common::start_second_with(&format!("{label}-edge1"), EDGE_CONF, &properties, &files);
            let rt = runtime();
            let paired = pair_node(&rt, &first, &first_url, NODE, &authority.root);
            let second_port = crate::common::free_port();
            let (second_port_text, second_url) = (second_port.to_string(), format!("https://localhost:{second_port}"));
            let mut properties = vec![
                ("node_role", "front_edge"),
                ("webapp_https_listen_port", second_port_text.as_str()),
                ("edge_link_url", second_url.as_str()),
                ("edge_link_key", link_key.as_str()),
            ];
            properties.extend_from_slice(&mail);
            let second = crate::common::start_member_with_files(
                &format!("{label}-edge2"),
                &first.cluster_dir,
                EDGE_CONF,
                &properties,
                &files,
            );
            let lmtp = [
                first.server.smtp_address().expect("the first member's LMTP listener"),
                second.0.smtp_address().expect("the second member's LMTP listener"),
            ];
            let mut config = LocalConfig::new(&first_url, &paired.key, &paired.pin, NODE);
            quick(&mut config);
            config.web = local_web(local.server.factory());
            config.member_key = Some(MemberKey::load_or_create(&local.runtime).unwrap());
            let (stop, shutdown) = watch::channel(false);
            let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
            wait_status(
                &dialer,
                Duration::from_secs(10),
                "a link to each edge member",
                |status| {
                    status.members.len() == 2
                        && status
                            .members
                            .iter()
                            .all(|member| member.state == moqui_core::edge::LinkState::Linked)
                },
            );
            Self {
                rt,
                dialer: Some(dialer),
                _stop: stop,
                lmtp,
                second: Some(second),
                first,
                local,
            }
        }

        /// RCPT TO `address` at member `at`: its reply.
        fn rcpt(&self, at: usize, address: &str, id: &str) -> String {
            lmtp(self.lmtp[at], &[address], &message(id, "Through the edge cluster"))
                .0
                .remove(0)
        }
    }

    impl Drop for MailCluster {
        fn drop(&mut self) {
            if let Some(dialer) = self.dialer.take() {
                dialer.stop();
            }
            drop(self.second.take());
            let _ = &self.rt;
        }
    }

    /// A hosted account on the local node: its id.
    fn account(local: &Gate, address: &str) -> String {
        let factory = local.server.factory();
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let _authz = context.disable_authz();
        let parameters = BTreeMap::from([
            ("accountKindEnumId".to_string(), Value::string("EakHosted")),
            ("emailAddress".to_string(), Value::string(address)),
        ]);
        let out =
            moqui_core::service::sync("org.moqui.mail.EmailClientServices.create#EmailAccount", &parameters).unwrap();
        to_plain_string(&out["emailAccountId"])
    }

    /// The copies of `message_id` stored on the local node.
    fn stored(local: &Gate, message_id: &str) -> usize {
        in_context(&local.server, |facade| {
            facade
                .find("moqui.basic.email.EmailMessage")
                .unwrap()
                .eq("messageId", Value::string(message_id))
                .use_cache(false)
                .list()
                .unwrap()
                .len()
        })
    }

    /// A recipient added on the local node, whose batch one edge member applies, is taken by both members' listeners.
    #[test]
    fn a_new_recipient_is_taken_by_both_edge_members() {
        if !crate::common::own_process() {
            return;
        }
        let cluster = MailCluster::start("ecs-mail");
        wait_until(Duration::from_secs(10), "the accounts on both members", || {
            (0..2).all(|at| {
                cluster
                    .rcpt(at, "sales@example.com", "<probe@example.org>")
                    .starts_with("250")
            })
        });
        assert!(cluster
            .rcpt(1, "bob@example.com", "<none@example.org>")
            .starts_with("550"));
        account(&cluster.local, "bob@example.com");
        wait_until(Duration::from_secs(10), "the new account on both members", || {
            (0..2).all(|at| {
                cluster
                    .rcpt(at, "bob@example.com", "<probe2@example.org>")
                    .starts_with("250")
            })
        });
    }

    /// Mail in and the queue go on through the other edge member with one stopped, and nothing is delivered twice.
    #[test]
    #[ignore = "acceptance: with one edge member stopped, mail in and the queue go on through the other, nothing twice"]
    fn acceptance_mail_in_goes_on_through_the_other_edge_member() {
        if !crate::common::own_process() {
            return;
        }
        let mut cluster = MailCluster::start("ecs-mail-acc");
        wait_until(Duration::from_secs(10), "the accounts on both members", || {
            (0..2).all(|at| {
                cluster
                    .rcpt(at, "sales@example.com", "<probe@example.org>")
                    .starts_with("250")
            })
        });
        // a message through each member, each stored once
        for (at, id) in [(0, "<one@example.org>"), (1, "<two@example.org>")] {
            let (rcpt, after) = lmtp(cluster.lmtp[at], &["sales@example.com"], &message(id, "Before"));
            assert_eq!(rcpt, ["250 2.1.5 Ok"]);
            assert!(after[0].contains("Ok"), "{after:?}");
            wait_until(Duration::from_secs(10), "stored", || stored(&cluster.local, id) == 1);
        }
        // the second member stops; the local node's link to it ends, and the first takes the mail and the queue
        let stopped = cluster.second.take();
        drop(stopped);
        let id = moqui_core::cluster::instance_id(cluster.first.server.factory()).unwrap();
        let dialer = cluster.dialer.as_ref().unwrap();
        wait_status(dialer, Duration::from_secs(10), "the other link down", |status| {
            status
                .members
                .iter()
                .all(|member| member.instance_id == id || member.state != moqui_core::edge::LinkState::Linked)
        });
        // a new account reaches the member left, and its mail goes through it
        account(&cluster.local, "carol@example.com");
        wait_until(Duration::from_secs(10), "the new account on the member left", || {
            cluster
                .rcpt(0, "carol@example.com", "<probe3@example.org>")
                .starts_with("250")
        });
        for (address, id) in [
            ("sales@example.com", "<three@example.org>"),
            ("carol@example.com", "<four@example.org>"),
        ] {
            let (rcpt, _) = lmtp(cluster.lmtp[0], &[address], &message(id, "After"));
            assert_eq!(rcpt, ["250 2.1.5 Ok"]);
            wait_until(Duration::from_secs(10), "stored after", || {
                stored(&cluster.local, id) == 1
            });
        }
        // nothing twice
        std::thread::sleep(Duration::from_secs(1));
        for id in [
            "<one@example.org>",
            "<two@example.org>",
            "<three@example.org>",
            "<four@example.org>",
        ] {
            assert_eq!(stored(&cluster.local, id), 1, "{id} once");
        }
    }
}
