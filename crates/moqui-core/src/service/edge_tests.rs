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

use std::sync::Arc;

use super::*;
use crate::edge::{EdgeLink, LinkState, LinkStatus, PairResult, PairingCode};

/// The tests install into the one process-wide slot, so they take turns.
fn slot_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    moqui_lib::sync::lock(&LOCK)
}

struct Fake;

impl EdgeLink for Fake {
    fn status(&self) -> LinkStatus {
        LinkStatus {
            state: LinkState::Linked,
            url: "https://edge.test".to_string(),
            generation: 7,
            rtt_ms: Some(1.5),
            link_fingerprint: "cd".repeat(32),
            // a link to each member of an edge cluster, the second carrying the work up
            members: vec![
                crate::edge::EdgeMemberLink {
                    instance_id: "edge-a".to_string(),
                    url: "https://edge-a.test".to_string(),
                    state: LinkState::Retrying,
                    last_error: "refused".to_string(),
                    retired: true,
                    ..crate::edge::EdgeMemberLink::default()
                },
                crate::edge::EdgeMemberLink {
                    instance_id: "edge-b".to_string(),
                    url: "https://edge-b.test".to_string(),
                    state: LinkState::Linked,
                    generation: 3,
                    rtt_ms: Some(2.5),
                    carries_work: true,
                    ..crate::edge::EdgeMemberLink::default()
                },
            ],
            ..LinkStatus::default()
        }
    }
    fn check(&self, url: &str) -> Result<EdgeInfo, LibError> {
        Ok(EdgeInfo {
            name: url.to_string(),
            fingerprint: "ab".repeat(32),
            compatible: true,
            ..EdgeInfo::default()
        })
    }
    fn pair(&self, url: &str, code: &str, expect: Option<&str>) -> Result<PairResult, LibError> {
        if expect.is_some_and(|expect| expect != "ab".repeat(32)) {
            return Err(LibError::new("the edge node's fingerprint is not the one confirmed"));
        }
        Ok(PairResult {
            edge: self.check(url)?,
            key_fingerprint: format!("key-for-{code}"),
        })
    }
    fn unpair(&self) -> Result<(), LibError> {
        Ok(())
    }
    fn create_pairing_code(&self, valid_minutes: u32) -> Result<PairingCode, LibError> {
        Ok(PairingCode {
            pairing_code_id: "100".to_string(),
            code: format!("CODE-{valid_minutes}"),
            expire_date: 0,
        })
    }
    fn clients(&self) -> Result<Vec<EdgeClientInfo>, LibError> {
        Ok(vec![EdgeClientInfo {
            paired_node_id: "local-1".to_string(),
            linked: true,
            generation: 3,
            ..EdgeClientInfo::default()
        }])
    }
    fn revoke(&self, paired_node_id: &str) -> Result<bool, LibError> {
        Ok(paired_node_id == "local-1")
    }
    fn ping_client(&self, _paired_node_id: &str) -> Result<f64, LibError> {
        Ok(2.0)
    }
}

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
        .collect()
}

#[test]
fn without_a_link_each_call_says_it_is_not_built_and_the_status_says_so_too() {
    let _lock = slot_lock();
    crate::edge::clear_edge_link();
    let facade = crate::testing::open();
    let status = get_link_status(&BTreeMap::new(), &facade).unwrap();
    assert_eq!(to_plain_string(&status["state"]), "notBuilt");
    assert_eq!(to_plain_string(&status["lastError"]), crate::edge::NOT_BUILT);
    let err = create_pairing_code(&BTreeMap::new(), &facade).unwrap_err();
    assert_eq!(err.message(), crate::edge::NOT_BUILT);
    let err = pair_edge_node(&params(&[("url", "https://edge.test"), ("code", "x")]), &facade).unwrap_err();
    assert_eq!(err.message(), crate::edge::NOT_BUILT);
}

#[test]
fn a_build_with_the_link_but_no_listener_says_not_started() {
    assert_eq!(unlinked(false).0, "notBuilt");
    assert_eq!(unlinked(false).1, crate::edge::NOT_BUILT);
    assert_eq!(unlinked(true).0, "notStarted");
}

#[test]
fn the_services_pass_their_parameters_through_and_shape_the_answers() {
    let _lock = slot_lock();
    crate::edge::install_edge_link(Arc::new(Fake));
    let facade = crate::testing::open();
    let status = get_link_status(&BTreeMap::new(), &facade).unwrap();
    assert_eq!(to_plain_string(&status["state"]), "linked");
    assert_eq!(status["generation"], Value::I64(7));
    assert_eq!(to_plain_string(&status["linkFingerprint"]), "cd".repeat(32));
    let Value::List(links) = &status["edgeMemberList"] else {
        panic!("{status:?}")
    };
    let field = |at: usize, name: &str| match &links[at] {
        Value::Map(row) => row[name].clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(links.len(), 2);
    assert_eq!(to_plain_string(&field(0, "url")), "https://edge-a.test");
    assert_eq!(to_plain_string(&field(0, "state")), "retrying");
    assert_eq!(field(0, "retired"), Value::Bool(true));
    assert_eq!(field(0, "carriesWork"), Value::Bool(false));
    assert_eq!(to_plain_string(&field(1, "instanceId")), "edge-b");
    assert_eq!(field(1, "carriesWork"), Value::Bool(true));
    assert_eq!(field(1, "generation"), Value::I64(3));
    assert_eq!(field(1, "rttMs"), Value::F64(2.5));
    let paired = pair_edge_node(
        &params(&[
            ("url", "https://edge.test"),
            ("code", "C1"),
            ("expectFingerprint", &"ab".repeat(32)),
        ]),
        &facade,
    )
    .unwrap();
    assert_eq!(to_plain_string(&paired["keyFingerprint"]), "key-for-C1");
    assert_eq!(paired["compatible"], Value::Bool(true));
    assert!(
        pair_edge_node(&params(&[("url", "https://edge.test")]), &facade).is_err(),
        "a code is required"
    );
    let code = create_pairing_code(&params(&[("validMinutes", "30")]), &facade).unwrap();
    assert_eq!(to_plain_string(&code["code"]), "CODE-30");
    assert!(create_pairing_code(&params(&[("validMinutes", "0")]), &facade).is_err());
    assert!(create_pairing_code(&params(&[("validMinutes", "2000")]), &facade).is_err());
    let clients = get_edge_clients(&BTreeMap::new(), &facade).unwrap();
    let Some(Value::List(list)) = clients.get("clientList") else {
        panic!("clientList")
    };
    let Some(Value::Map(row)) = list.first() else {
        panic!("a client")
    };
    assert_eq!(to_plain_string(&row["pairedNodeId"]), "local-1");
    assert_eq!(row["linked"], Value::Bool(true));
    let revoked = revoke_edge_client(&params(&[("pairedNodeId", "local-1")]), &facade).unwrap();
    assert_eq!(revoked["revoked"], Value::Bool(true));
    let ping = ping_edge_client(&params(&[("pairedNodeId", "local-1")]), &facade).unwrap();
    assert_eq!(ping["rttMs"], Value::F64(2.0));
    // the host name services read and write rows (crate::edge::hosts) on a node that takes links, not the link
    assert!(get_edge_hosts(&BTreeMap::new(), &facade)
        .unwrap_err()
        .message()
        .contains("only a node that takes links"));
    crate::edge::clear_edge_link();
}

/// A link installed with its HTTPS listener off: it serves no links.
struct Off;

impl EdgeLink for Off {
    fn serving(&self) -> bool {
        false
    }
    fn status(&self) -> LinkStatus {
        Fake.status()
    }
    fn check(&self, url: &str) -> Result<EdgeInfo, LibError> {
        Fake.check(url)
    }
    fn pair(&self, url: &str, code: &str, expect: Option<&str>) -> Result<PairResult, LibError> {
        Fake.pair(url, code, expect)
    }
    fn unpair(&self) -> Result<(), LibError> {
        Ok(())
    }
    fn create_pairing_code(&self, valid_minutes: u32) -> Result<PairingCode, LibError> {
        Fake.create_pairing_code(valid_minutes)
    }
    fn clients(&self) -> Result<Vec<EdgeClientInfo>, LibError> {
        Err(LibError::new(
            "this edge node takes no links: its HTTPS listener is off",
        ))
    }
    fn revoke(&self, _paired_node_id: &str) -> Result<bool, LibError> {
        Ok(false)
    }
    fn ping_client(&self, _paired_node_id: &str) -> Result<f64, LibError> {
        Err(LibError::new("off"))
    }
}

#[test]
fn a_link_whose_listener_is_off_is_not_started_and_serves_nothing_in_memory() {
    let _lock = slot_lock();
    crate::edge::install_edge_link(Arc::new(Off));
    let facade = crate::testing::open();
    let status = get_link_status(&BTreeMap::new(), &facade).unwrap();
    assert_eq!(to_plain_string(&status["state"]), "notStarted");
    assert!(crate::edge::serving_link().is_none());
    crate::edge::install_edge_link(Arc::new(Fake));
    assert!(crate::edge::serving_link().is_some());
    crate::edge::clear_edge_link();
}

#[test]
fn the_front_pairing_is_a_row_per_front_link_or_one_from_what_pairing_wrote() {
    let field = |rows: &[Value], at: usize, name: &str| match &rows[at] {
        Value::Map(row) => row[name].clone(),
        other => panic!("{other:?}"),
    };
    // none without a front pairing
    assert!(front_rows(None, " ", "").is_empty());
    // a front pairing alone, by the properties pairing wrote
    assert!(front_only(" ", "https://front.test"));
    assert!(!front_only("https://edge.test", "https://front.test"));
    assert!(!front_only("", ""));
    // the link not running here: what pairing wrote, not started
    let rows = front_rows(None, " https://front.test ", "front-1");
    assert_eq!(rows.len(), 1);
    assert_eq!(to_plain_string(&field(&rows, 0, "url")), "https://front.test");
    assert_eq!(to_plain_string(&field(&rows, 0, "instanceId")), "front-1");
    assert_eq!(to_plain_string(&field(&rows, 0, "state")), "notStarted");
    assert_eq!(field(&rows, 0, "lastConnectDate"), Value::Null);
    // a link to each member of the front's cluster, each with the front's name and pinned fingerprint
    let mut front = LinkStatus {
        state: LinkState::Linked,
        url: "https://front.test".to_string(),
        ..LinkStatus::default()
    };
    front.peer.name = "front.test".to_string();
    front.peer.fingerprint = "ef".repeat(32);
    let one = front_rows(Some(&front), "https://ignored.test", "");
    assert_eq!(one.len(), 1, "a link with no member list is one row");
    assert_eq!(to_plain_string(&field(&one, 0, "state")), "linked");
    assert_eq!(to_plain_string(&field(&one, 0, "frontName")), "front.test");
    front.members = vec![
        crate::edge::EdgeMemberLink {
            instance_id: "front-1".to_string(),
            url: "https://front.test".to_string(),
            state: LinkState::Linked,
            rtt_ms: Some(1.5),
            ..Default::default()
        },
        crate::edge::EdgeMemberLink {
            instance_id: "front-2".to_string(),
            url: "https://front2.test".to_string(),
            state: LinkState::Retrying,
            last_error: "refused".to_string(),
            ..Default::default()
        },
    ];
    let rows = front_rows(Some(&front), "", "");
    assert_eq!(rows.len(), 2);
    assert_eq!(field(&rows, 0, "rttMs"), Value::F64(1.5));
    assert_eq!(to_plain_string(&field(&rows, 1, "instanceId")), "front-2");
    assert_eq!(to_plain_string(&field(&rows, 1, "state")), "retrying");
    assert_eq!(to_plain_string(&field(&rows, 1, "lastError")), "refused");
    assert_eq!(to_plain_string(&field(&rows, 1, "fingerprint")), "ef".repeat(32));
}
