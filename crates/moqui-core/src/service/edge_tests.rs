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
use crate::edge::{EdgeHostInfo, EdgeLink, LinkState, LinkStatus, PairResult, PairingCode};

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
    fn hosts(&self, paired_node_id: Option<&str>) -> Result<Vec<EdgeHostInfo>, LibError> {
        Ok(vec![EdgeHostInfo {
            host_name: "erp.example.com".to_string(),
            paired_node_id: paired_node_id.unwrap_or("local-1").to_string(),
            mode_enum_id: crate::edge::HOST_TERMINATE.to_string(),
        }])
    }
    fn add_host(&self, host: &EdgeHostInfo) -> Result<EdgeHostInfo, LibError> {
        Ok(EdgeHostInfo {
            host_name: host.host_name.to_ascii_lowercase(),
            ..host.clone()
        })
    }
    fn remove_host(&self, host_name: &str) -> Result<bool, LibError> {
        Ok(host_name == "erp.example.com")
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
    let hosts = get_edge_hosts(&params(&[("pairedNodeId", "local-2")]), &facade).unwrap();
    let Some(Value::List(list)) = hosts.get("hostList") else {
        panic!("hostList")
    };
    let Some(Value::Map(row)) = list.first() else {
        panic!("a host")
    };
    assert_eq!(to_plain_string(&row["pairedNodeId"]), "local-2");
    assert_eq!(to_plain_string(&row["modeEnumId"]), "EdgeHostTerminate");
    let added = add_edge_host(
        &params(&[("hostName", "ERP.example.com"), ("pairedNodeId", "local-1")]),
        &facade,
    )
    .unwrap();
    assert_eq!(to_plain_string(&added["hostName"]), "erp.example.com");
    assert_eq!(
        to_plain_string(&added["modeEnumId"]),
        "EdgeHostPassthrough",
        "passthrough by default"
    );
    let bad = add_edge_host(
        &params(&[
            ("hostName", "a.test"),
            ("pairedNodeId", "local-1"),
            ("modeEnumId", "Other"),
        ]),
        &facade,
    );
    assert!(bad.is_err());
    let removed = remove_edge_host(&params(&[("hostName", "erp.example.com")]), &facade).unwrap();
    assert_eq!(removed["removed"], Value::Bool(true));
    crate::edge::clear_edge_link();
}
