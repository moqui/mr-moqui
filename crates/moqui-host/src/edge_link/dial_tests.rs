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

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Weak};

use tonic::Request;

use super::{role_matches, work_factory, FixedMember, LocalService};
use crate::edge_link::proto::local_link_server::LocalLink as _;
use crate::edge_link::proto::{DelegateRequest, DeliverRequest};
use crate::edge_link::LocalConfig;

/// A front pairing reaches a front node and an edge pairing anything else; the other way round is refused, naming
/// the node's role, so the operator pairs again (plans/Edge-Node2.md §4.7).
#[test]
fn a_pairing_links_only_to_a_node_of_its_kind() {
    let mut config = LocalConfig::new("https://front.example.com", "key", "pin", "local-1");
    config.front = true;
    assert!(role_matches(&config, "front").is_ok());
    let refused = role_matches(&config, "front_edge").unwrap_err();
    assert!(
        refused.contains("not a front node (its role is front_edge)"),
        "{refused}"
    );
    assert!(role_matches(&config, "").unwrap_err().contains("not told"));
    config.front = false;
    for role in ["front_edge", "edge", "all-in-one", ""] {
        assert!(role_matches(&config, role).is_ok(), "{role}");
    }
    let refused = role_matches(&config, "front").unwrap_err();
    assert!(refused.contains("is a front node"), "{refused}");
}

/// A front pairing's down connection takes no queued item and no delegated call: it serves no server for them.
#[tokio::test]
async fn a_front_pairings_down_connection_takes_no_work() {
    assert!(work_factory(Some(Weak::new()), true).is_none());
    assert!(work_factory(Some(Weak::new()), false).is_some());
    let service = LocalService {
        current: Arc::new(AtomicU64::new(7)),
        hello: Default::default(),
        factory: work_factory(Some(Weak::new()), true),
        edge_node: "front-1".to_string(),
        member: FixedMember::new("local-1"),
        stopping: Arc::new(AtomicBool::new(false)),
    };
    let delivered = service
        .deliver(Request::new(DeliverRequest {
            generation: 7,
            item: Some(Default::default()),
        }))
        .await
        .unwrap_err();
    assert_eq!(delivered.code(), tonic::Code::Unavailable, "{delivered:?}");
    assert!(delivered.message().contains("no queue or delegated calls"));
    let delegated = service
        .delegate(Request::new(DelegateRequest {
            generation: 7,
            service: "moqui.test.Anything.run#Service".to_string(),
            ..Default::default()
        }))
        .await
        .unwrap_err();
    assert_eq!(delegated.code(), tonic::Code::Unavailable, "{delegated:?}");
}
