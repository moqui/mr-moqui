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

//! A local node linked to every member of an edge cluster (plans/Edge-Node2.md §4.6, E18 part A). The edge cluster is
//! two servers on one database in the test's own process: the first a gate, the second `common::start_member` on its
//! database, each with its own HTTPS port and `edge_link_url` and the one shared `edge_link_key`. The local node is a
//! gate on a database of its own, started first (`node_role` is a process property it must not read), linked by a
//! dialer driven with an explicit `LocalConfig` serving its app. Both ends have a test queue kind whose handler writes
//! an `edgetest.EdgeTestEcho` row per item, so an item applied twice would show.
//!
//! - The Hello and Ping answers list the edge cluster's live members with their link addresses; the local node dials
//!   each, learns each id, keeps the list in its file, and sends its work up one link: its outbox and replicated set
//!   arrive once, and each edge member's queue delivery and terminate host reach it. A link dialed for a member whose
//!   address reaches another member is refused, and corrected by the next list; a dialer with another pin links to no
//!   member.
//! - A restart with the URL it paired with gone links to the other member from the kept list, and a member gone from
//!   the list is not dialed again.
//! - Acceptance (run alone): with one edge member stopped, web and the queue go on through the other and nothing is
//!   delivered twice; a clustered all-in-one server takes links on every member.

#![cfg(all(unix, feature = "edge", feature = "ssh", feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_core::edge::{LinkState, LinkStatus};
use moqui_core::entity::EntityFacade;
use moqui_host::edge_link::{local_web, new_key, Backoff, Dialer, LocalConfig, MemberKey, Paired};
use moqui_lib::object::{to_plain_string, Value};
use rustls::pki_types::CertificateDer;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::watch;

use crate::common::{DirGuard, Gate};
use crate::edge_link::{quick, runtime, wait_status};
use crate::edge_web::{authority, call, issue, pair_node, wait_until};

pub(crate) const NODE: &str = "local-ec";
pub(crate) const SHOP: &str = "shop.localhost";
const ECHO: &str = "edgetest.EdgeTestEcho";
const HANDLER: &str = "edgetest.EdgeTestServices.apply#EchoItem";

/// The edge conf of every node: quick retries, a test queue kind, and a replicated set.
pub(crate) const EDGE_CONF: &str = r#"<edge queue-retry-seconds="0.1" queue-max-attempts="3">
    <queue-kind name="echo" service="edgetest.EdgeTestServices.apply#EchoItem"/>
    <replicate set="groups"><entity name="moqui.security.UserGroup" fields="description"/></replicate>
</edge>"#;

const COMPONENT: &str = r#"<component name="edgetest"/>"#;

const ENTITIES: &str = r#"<entities>
    <entity entity-name="EdgeTestEcho" package="edgetest" use="transactional" cache="never">
        <field name="echoId" type="id" is-pk="true"/>
        <field name="queueItemId" type="id"/>
        <field name="conversationKey" type="text-medium"/>
        <field name="payload" type="text-long"/>
    </entity>
</entities>"#;

const HANDLER_SERVICES: &str = r#"<services>
    <service verb="apply" noun="EchoItem">
        <in-parameters>
            <parameter name="queueItemId"/>
            <parameter name="kind"/>
            <parameter name="conversationKey" required="true"/>
            <parameter name="idempotencyKey"/>
            <parameter name="payload" allow-html="any"/>
            <parameter name="contentLocation"/>
        </in-parameters>
        <actions>
            <entity-make-value value-field="echo" entity-name="edgetest.EdgeTestEcho"/>
            <entity-sequenced-id-primary value-field="echo"/>
            <set field="echo.queueItemId" from="queueItemId"/>
            <set field="echo.conversationKey" from="conversationKey"/>
            <set field="echo.payload" from="payload"/>
            <entity-create value-field="echo"/>
        </actions>
    </service>
</services>"#;

pub(crate) fn files<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut files = vec![
        ("components/addon/edgetest/component.xml", COMPONENT),
        ("components/addon/edgetest/entity/EdgeTestEntities.xml", ENTITIES),
        (
            "components/addon/edgetest/service/edgetest/EdgeTestServices.xml",
            HANDLER_SERVICES,
        ),
    ];
    files.extend_from_slice(extra);
    files
}

pub(crate) fn in_context<T>(server: &moqui_host::Server, body: impl FnOnce(&EntityFacade) -> T) -> T {
    let factory = server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    body(&factory.entity().unwrap())
}

/// Grant the test handler to `_EDGE_`, as a comms phase's seed data does for its own handlers.
pub(crate) fn grant_handler(server: &moqui_host::Server) {
    in_context(server, |facade| {
        let mut member = facade.make_value("moqui.security.ArtifactGroupMember").unwrap();
        member.set("artifactGroupId", Value::string("EDGE_DELEGATE"));
        member.set("artifactName", Value::string(HANDLER));
        member.set("nameIsPattern", Value::string("N"));
        member.set("artifactTypeEnumId", Value::string("AT_SERVICE"));
        member.set("inheritAuthz", Value::string("Y"));
        member.create(facade).unwrap();
    });
}

/// Queue a test item on `server` for the other end.
pub(crate) fn enqueue(server: &moqui_host::Server, key: &str, payload: &str) {
    let factory = server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let parameters = BTreeMap::from([
        ("kind".to_string(), Value::string("echo")),
        ("conversationKey".to_string(), Value::string(key)),
        ("payload".to_string(), Value::string(payload)),
    ]);
    moqui_core::service::sync("org.moqui.edge.EdgeServices.enqueue#Item", &parameters).unwrap();
    let errors =
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    assert!(errors.is_empty(), "{errors:?}");
}

/// The payloads the test handler applied on `server` for `key`, in order.
pub(crate) fn echoed(server: &moqui_host::Server, key: &str) -> Vec<String> {
    in_context(server, |facade| {
        facade
            .find(ECHO)
            .unwrap()
            .eq("conversationKey", Value::string(key))
            .order_by("echoId", false)
            .use_cache(false)
            .list()
            .unwrap()
            .values()
            .iter()
            .map(|row| to_plain_string(row.get("payload").unwrap_or(&Value::Null)))
            .collect()
    })
}

/// The edge node's copy of the local node's group `id`: its description, or `None`.
fn replica(server: &moqui_host::Server, id: &str) -> Option<String> {
    let pk = BTreeMap::from([("userGroupId".to_string(), id.to_string())]);
    in_context(server, |facade| {
        moqui_core::edge::replica::get_value(facade, Some(NODE), "groups", "moqui.security.UserGroup", &pk)
            .unwrap()
            .map(|value| value.fields["description"].as_str().unwrap_or_default().to_string())
    })
}

fn client_config(root: &CertificateDer<'static>) -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.clone()).unwrap();
    let mut config =
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

/// A GET of `path` on [`SHOP`] through the edge member at `port`: the status (0 when it could not connect).
pub(crate) fn get(rt: &tokio::runtime::Runtime, tls: &Arc<rustls::ClientConfig>, port: u16, path: &str) -> u16 {
    let request = format!("GET {path} HTTP/1.1\r\nHost: {SHOP}\r\nConnection: close\r\n\r\n");
    rt.block_on(async {
        let Ok(tcp) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await else {
            return 0;
        };
        let name = rustls::pki_types::ServerName::try_from(SHOP).unwrap();
        let Ok(mut stream) = tokio_rustls::TlsConnector::from(Arc::clone(tls))
            .connect(name, tcp)
            .await
        else {
            return 0;
        };
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut answer)).await;
        String::from_utf8_lossy(&answer)
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0)
    })
}

/// The members a dialer's links are for, by instance id: their state and where each dials.
pub(crate) fn links(status: &LinkStatus) -> BTreeMap<String, (LinkState, String)> {
    status
        .members
        .iter()
        .map(|member| (member.instance_id.clone(), (member.state, member.url.clone())))
        .collect()
}

/// The list a local node kept: each member with its address, and the ids it learned.
fn kept(path: &Path) -> (BTreeMap<String, String>, Vec<String>) {
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let members = json["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| {
            (
                member["instanceId"].as_str().unwrap().to_string(),
                member["linkUrl"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let ids = json["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect();
    (members, ids)
}

/// An edge cluster of two members on one database, with [`SHOP`] a terminate host of the local node [`NODE`], paired.
pub(crate) struct EdgeCluster {
    pub(crate) rt: tokio::runtime::Runtime,
    pub(crate) tls: Arc<rustls::ClientConfig>,
    pub(crate) paired: Paired,
    pub(crate) ports: [u16; 2],
    pub(crate) ids: [String; 2],
    /// The second member: dropped to stop it.
    pub(crate) second: Option<(moqui_host::Server, DirGuard)>,
    pub(crate) first: Gate,
    pub(crate) local: Gate,
    scratch: PathBuf,
}

impl EdgeCluster {
    pub(crate) fn start(label: &str, role: &str) -> Self {
        let authority = authority();
        let cert = issue(&authority, &["localhost", SHOP]);
        let local = crate::common::start_with_conf_files(&format!("{label}-local"), None, EDGE_CONF, &[], &files(&[]));
        grant_handler(&local.server);
        // one link key for the cluster, as its shared .env has
        let link_key = new_key().unwrap();
        // each port taken just before the server that binds it, so another test's server has little time to take it
        let first_port = crate::common::free_port();
        let first_url = format!("https://localhost:{first_port}");
        let (first_port_text, cluster_port) = (first_port.to_string(), crate::common::free_port().to_string());
        let first = crate::common::start_second_with(
            &format!("{label}-edge1"),
            EDGE_CONF,
            &[
                ("node_role", role),
                ("webapp_https_listen_port", &first_port_text),
                ("edge_link_url", &first_url),
                ("edge_link_key", &link_key),
                ("cluster_https_port", &cluster_port),
            ],
            &files(&[("cert/default.crt", &cert.0), ("cert/default.key", &cert.1)]),
        );
        grant_handler(&first.server);
        let rt = runtime();
        let paired = pair_node(&rt, &first, &first_url, NODE, &authority.root);
        // the host before the second member starts, which reads the host names at its start (18B tells it of a change)
        call(
            &first,
            "add#EdgeHost",
            &[
                ("hostName", SHOP),
                ("pairedNodeId", NODE),
                ("modeEnumId", "EdgeHostTerminate"),
            ],
        )
        .unwrap();
        let ports = [first_port, crate::common::free_port()];
        let (second_port, second_url) = (ports[1].to_string(), format!("https://localhost:{}", ports[1]));
        let second = crate::common::start_member_with_files(
            &format!("{label}-edge2"),
            &first.cluster_dir,
            EDGE_CONF,
            &[
                ("node_role", role),
                ("webapp_https_listen_port", &second_port),
                ("edge_link_url", &second_url),
                ("edge_link_key", &link_key),
            ],
            &files(&[("cert/default.crt", &cert.0), ("cert/default.key", &cert.1)]),
        );
        let ids = [
            moqui_core::cluster::instance_id(first.server.factory()).unwrap(),
            moqui_core::cluster::instance_id(second.0.factory()).unwrap(),
        ];
        assert_ne!(ids[0], ids[1]);
        // before the local node links, each member answers the host with the offline page once it has read its host
        // names (at its start, off the start's thread): a 200 after is the local node's
        let tls = client_config(&authority.root);
        for port in ports {
            wait_until(Duration::from_secs(10), "the offline page on each member", || {
                get(&rt, &tls, port, "/Login") == 503
            });
        }
        let scratch = crate::common::temp_dir(&format!("moqui-edge-cluster-{label}"));
        std::fs::create_dir_all(&scratch).unwrap();
        Self {
            tls,
            rt,
            paired,
            ports,
            ids,
            second: Some(second),
            first,
            local,
            scratch,
        }
    }

    pub(crate) fn url(&self, member: usize) -> String {
        format!("https://localhost:{}", self.ports[member])
    }

    pub(crate) fn members_file(&self) -> PathBuf {
        self.scratch.join("db/edge_members")
    }

    /// The local node's config, dialing `url` first, with the list kept in the scratch directory and each id learned
    /// told to `learned`.
    pub(crate) fn config(&self, url: &str, learned: &Arc<Mutex<Vec<String>>>) -> LocalConfig {
        let mut config = LocalConfig::new(url, &self.paired.key, &self.paired.pin, NODE);
        quick(&mut config);
        config.web = local_web(self.local.server.factory());
        config.member_key = Some(MemberKey::load_or_create(&self.local.runtime).unwrap());
        config.members_file = Some(self.members_file());
        let learned = Arc::clone(learned);
        config.on_pin = Some(Arc::new(move |id: &str| learned.lock().unwrap().push(id.to_string())));
        config
    }

    pub(crate) fn dial(&self, config: LocalConfig) -> (Dialer, watch::Sender<bool>) {
        let (stop, shutdown) = watch::channel(false);
        (Dialer::start(config, self.rt.handle(), shutdown, None).unwrap(), stop)
    }

    /// Wait until the dialer has a link to each of `members`, all linked, and no other.
    pub(crate) fn wait_linked(&self, dialer: &Dialer, members: &[usize], what: &str) {
        let want: Vec<String> = members.iter().map(|at| self.ids[*at].clone()).collect();
        wait_status(dialer, Duration::from_secs(10), what, |status| {
            let links = links(status);
            links.len() == want.len()
                && want
                    .iter()
                    .all(|id| links.get(id).is_some_and(|(state, _)| *state == LinkState::Linked))
        });
    }

    /// Stop the second member: its server stops, and its member row goes, as its heartbeat would age out.
    pub(crate) fn stop_second(&mut self) {
        drop(self.second.take());
        let id = self.ids[1].clone();
        in_context(&self.first.server, |facade| {
            facade
                .find("moqui.server.ClusterMember")
                .unwrap()
                .eq("instanceId", Value::string(&id))
                .delete_all()
                .unwrap();
        });
    }
}

impl Drop for EdgeCluster {
    fn drop(&mut self) {
        // the member before the gate whose database it runs on
        drop(self.second.take());
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

#[test]
fn a_local_node_links_to_every_edge_member_and_sends_its_work_once() {
    if !crate::common::own_process() {
        return;
    }
    let cluster = EdgeCluster::start("edge-cluster", "front_edge");

    // the edge cluster as an edge member reports it: both live, each with its link address
    let out = call(&cluster.first, "get#EdgeCluster", &[]).unwrap();
    let Some(Value::List(list)) = out.get("memberList") else {
        panic!("memberList: {out:?}")
    };
    let reported: BTreeMap<String, String> = list
        .iter()
        .filter_map(|row| match row {
            Value::Map(row) if row["live"] == Value::Bool(true) => {
                Some((to_plain_string(&row["instanceId"]), to_plain_string(&row["linkUrl"])))
            }
            _ => None,
        })
        .collect();
    assert_eq!(reported.get(&cluster.ids[0]), Some(&cluster.url(0)));
    assert_eq!(reported.get(&cluster.ids[1]), Some(&cluster.url(1)));
    // each address is one the Hello answer sends (System › Edge Clients › Edge cluster shows no warning)
    assert!(
        list.iter()
            .all(|row| matches!(row, Value::Map(row) if row["linkUrlUsable"] == Value::Bool(true))),
        "{list:?}"
    );

    // a link dialed for a member whose address reaches another one is refused; the next list puts it right
    std::fs::create_dir_all(cluster.members_file().parent().unwrap()).unwrap();
    std::fs::write(
        cluster.members_file(),
        serde_json::json!({
            "url": cluster.url(0),
            "pin": cluster.paired.pin,
            "members": [{"instanceId": cluster.ids[1], "linkUrl": cluster.url(0)}],
            "ids": [],
        })
        .to_string(),
    )
    .unwrap();
    let learned = Arc::new(Mutex::new(Vec::new()));
    // a server's own waits: a misdirected link is a failure, not a refusal (30 s and up), and the corrected address is
    // tried at once
    let mut config = cluster.config(&cluster.url(0), &learned);
    config.backoff = Backoff::default();
    config.refused_backoff = Backoff::refused();
    let (dialer, _stop) = cluster.dial(config);
    wait_status(
        &dialer,
        Duration::from_secs(10),
        "the misdirected link failed",
        |status| {
            status.members.iter().any(|member| {
                member.instance_id == cluster.ids[1]
                    && member.state == LinkState::Retrying
                    && member.last_error.contains("names itself")
            })
        },
    );

    // every member linked at its own address, each id learned, and the list kept
    cluster.wait_linked(&dialer, &[0, 1], "a link to each edge member");
    let status = dialer.status();
    let links = links(&status);
    assert_eq!(links[&cluster.ids[0]].1, cluster.url(0));
    assert_eq!(links[&cluster.ids[1]].1, cluster.url(1));
    assert_eq!(
        status.members.iter().filter(|member| member.carries_work).count(),
        1,
        "one link carries the work: {status:?}"
    );
    let mut learned_ids = learned.lock().unwrap().clone();
    learned_ids.sort();
    let mut both = cluster.ids.to_vec();
    both.sort();
    assert_eq!(learned_ids, both, "each member's id learned on a pinned link");
    let (members, ids) = kept(&cluster.members_file());
    assert_eq!(members.get(&cluster.ids[0]), Some(&cluster.url(0)));
    assert_eq!(members.get(&cluster.ids[1]), Some(&cluster.url(1)));
    assert_eq!(ids.len(), 2);

    // a request to either member reaches the local node
    for port in cluster.ports {
        assert_eq!(
            get(&cluster.rt, &cluster.tls, port, "/Login"),
            200,
            "through port {port}"
        );
    }

    // the edge members' queue delivery: each item once, in order
    for at in 0..6 {
        enqueue(&cluster.first.server, "down", &format!("D{at}"));
    }
    let down: Vec<String> = (0..6).map(|at| format!("D{at}")).collect();
    wait_until(Duration::from_secs(10), "the items down", || {
        echoed(&cluster.local.server, "down").len() >= 6
    });
    // the local node's outbox and replicated set go up one link: each item once
    for at in 0..6 {
        enqueue(&cluster.local.server, "up", &format!("U{at}"));
    }
    let up: Vec<String> = (0..6).map(|at| format!("U{at}")).collect();
    wait_until(Duration::from_secs(10), "the items up", || {
        echoed(&cluster.first.server, "up").len() >= 6
    });
    wait_until(Duration::from_secs(10), "the replicated set", || {
        replica(&cluster.first.server, "ADMIN").is_some()
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(echoed(&cluster.local.server, "down"), down, "each item down once");
    assert_eq!(echoed(&cluster.first.server, "up"), up, "each item up once");
    dialer.stop();
    drop(dialer);

    // a dialer with another pin links to no member: the pin is of the link key every member presents
    let mut wrong = cluster.config(&cluster.url(0), &Arc::new(Mutex::new(Vec::new())));
    wrong.pin = "00".repeat(32);
    wrong.members_file = None;
    let (pinned, _stop) = cluster.dial(wrong);
    std::thread::sleep(Duration::from_millis(800));
    let status = pinned.status();
    assert!(
        status.members.iter().all(|member| member.state != LinkState::Linked),
        "{status:?}"
    );
    assert_ne!(status.state, LinkState::Linked);
}

#[test]
fn a_restart_dials_the_kept_members_and_not_one_gone_from_the_list() {
    if !crate::common::own_process() {
        return;
    }
    let mut cluster = EdgeCluster::start("edge-cluster-kept", "front_edge");
    // ids the local node has a use for are refused on any link: a member of its cluster, here one not live, and a media
    // node of its own
    let (second_id, first_id) = (cluster.ids[1].clone(), cluster.ids[0].clone());
    in_context(&cluster.local.server, |facade| {
        let mut member = facade.make_value("moqui.server.ClusterMember").unwrap();
        member.set("instanceId", Value::string(&second_id));
        member.set(
            "heartbeat",
            moqui_core::user::stamp_value(moqui_core::user::now_millis() - 3_600_000),
        );
        member.create(facade).unwrap();
        let mut media = facade.make_value("moqui.call.CallMediaNode").unwrap();
        media.set("instanceId", Value::string(&first_id));
        media.set("edgeLinked", Value::string("N"));
        media.create(facade).unwrap();
    });
    let remove = |entity: &str, id: &str| {
        in_context(&cluster.local.server, |facade| {
            facade
                .find(entity)
                .unwrap()
                .eq("instanceId", Value::string(id))
                .delete_all()
                .unwrap();
        });
    };
    // paired, as it were, at the second member's address
    let learned = Arc::new(Mutex::new(Vec::new()));
    let config = cluster.config(&cluster.url(1), &learned);
    let (dialer, _stop) = cluster.dial(config.clone());
    let refused = |id: &str, what: &'static str| {
        let id = id.to_string();
        move |status: &LinkStatus| {
            status.members.iter().any(|member| {
                (member.instance_id == id || member.instance_id.is_empty())
                    && member.state == LinkState::Refused
                    && member.last_error.contains(what)
            })
        }
    };
    wait_status(
        &dialer,
        Duration::from_secs(10),
        "a member of the local cluster refused",
        refused(&second_id, "a member of this node's cluster"),
    );
    remove("moqui.server.ClusterMember", &second_id);
    wait_status(
        &dialer,
        Duration::from_secs(10),
        "a media node of the local node's own refused",
        refused(&first_id, "a media node of this node's own"),
    );
    remove("moqui.call.CallMediaNode", &first_id);
    cluster.wait_linked(&dialer, &[0, 1], "a link to each edge member");
    assert!(!learned.lock().unwrap().is_empty());
    dialer.stop();
    drop(dialer);

    // the member it paired with stops; a restart finds the other from the kept list
    cluster.stop_second();
    let (dialer, _stop) = cluster.dial(config);
    wait_status(&dialer, Duration::from_secs(10), "the other member linked", |status| {
        status
            .members
            .iter()
            .any(|member| member.instance_id == cluster.ids[0] && member.state == LinkState::Linked)
    });
    // the stopped member leaves the list within a Ping, and is no longer dialed
    cluster.wait_linked(&dialer, &[0], "only the live member");
    let (members, _) = kept(&cluster.members_file());
    assert_eq!(
        members.keys().cloned().collect::<Vec<_>>(),
        vec![cluster.ids[0].clone()]
    );
    assert_eq!(get(&cluster.rt, &cluster.tls, cluster.ports[0], "/Login"), 200);
    let status = dialer.status();
    assert!(
        status.members.iter().all(|member| member.url == cluster.url(0)),
        "neither the stopped member nor the URL it paired with is dialed: {status:?}"
    );
}

/// Items both ways while one edge member stops: `count` of each, every one applied once.
fn items_go_on_with_one_member_stopped(role: &str, label: &str) {
    let mut cluster = EdgeCluster::start(label, role);
    let learned = Arc::new(Mutex::new(Vec::new()));
    let (dialer, _stop) = cluster.dial(cluster.config(&cluster.url(0), &learned));
    cluster.wait_linked(&dialer, &[0, 1], "a link to each edge member");
    for port in cluster.ports {
        assert_eq!(
            get(&cluster.rt, &cluster.tls, port, "/Login"),
            200,
            "through port {port}"
        );
    }
    let count = 40;
    for at in 0..count / 2 {
        enqueue(&cluster.first.server, "down", &format!("D{at}"));
        enqueue(&cluster.local.server, "up", &format!("U{at}"));
    }
    cluster.stop_second();
    for at in count / 2..count {
        enqueue(&cluster.first.server, "down", &format!("D{at}"));
        enqueue(&cluster.local.server, "up", &format!("U{at}"));
    }
    // web goes on through the other member, which carries the work now
    assert_eq!(get(&cluster.rt, &cluster.tls, cluster.ports[0], "/Login"), 200);
    cluster.wait_linked(&dialer, &[0], "only the live member");
    let status = dialer.status();
    assert!(status.members[0].carries_work, "{status:?}");
    let down: Vec<String> = (0..count).map(|at| format!("D{at}")).collect();
    let up: Vec<String> = (0..count).map(|at| format!("U{at}")).collect();
    wait_until(Duration::from_secs(20), "every item down", || {
        echoed(&cluster.local.server, "down").len() >= count
    });
    wait_until(Duration::from_secs(20), "every item up", || {
        echoed(&cluster.first.server, "up").len() >= count
    });
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        echoed(&cluster.local.server, "down"),
        down,
        "each item down once, in order"
    );
    assert_eq!(echoed(&cluster.first.server, "up"), up, "each item up once, in order");
}

#[test]
#[ignore = "acceptance: with one edge member stopped, web and the queue go on through the other, nothing delivered twice"]
fn acceptance_one_edge_member_stopped_leaves_web_and_the_queue_to_the_other() {
    if !crate::common::own_process() {
        return;
    }
    items_go_on_with_one_member_stopped("front_edge", "edge-cluster-stop");
}

#[test]
#[ignore = "acceptance: a clustered all-in-one server takes links on every member"]
fn acceptance_a_clustered_all_in_one_server_takes_links_on_every_member() {
    if !crate::common::own_process() {
        return;
    }
    items_go_on_with_one_member_stopped("all-in-one", "edge-cluster-aio");
}
