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

use std::time::Duration;

use super::pair::{parse_url, EdgeUrl};
use super::store::normalize_code;
use super::{check_edge_member, check_versions, edge_info, hello, LINK_VERSION};

#[test]
fn versions_one_apart_link_and_further_apart_are_refused_plainly() {
    assert!(check_versions(3, 3, "edge node").is_ok());
    assert!(check_versions(3, 2, "local node").is_ok());
    assert!(check_versions(3, 4, "local node").is_ok());
    let behind = check_versions(3, 1, "local node").unwrap_err();
    assert!(behind.contains("version 1") && behind.contains("version 3"), "{behind}");
    assert!(behind.contains("upgrade local node"), "{behind}");
    let ahead = check_versions(3, 5, "edge node").unwrap_err();
    assert!(ahead.contains("upgrade this node"), "{ahead}");
    assert!(check_versions(3, 0, "edge node")
        .unwrap_err()
        .contains("no link protocol version"));
    let mut far = hello("local-1", "Local", &[], &[]);
    far.link_version = LINK_VERSION + 2;
    let info = edge_info(&far, "ab");
    assert!(!info.compatible && info.message.contains("cannot link"), "{info:?}");
    assert!(edge_info(&hello("e", "Edge", &[], &[]), "ab").compatible);
}

#[test]
fn edge_node_urls_take_a_host_and_an_optional_port() {
    let at = |host: &str, port: u16| EdgeUrl {
        host: host.to_string(),
        port,
    };
    assert_eq!(
        parse_url("https://edge.example.com").unwrap(),
        at("edge.example.com", 443)
    );
    assert_eq!(
        parse_url(" https://edge.example.com:8443/x ").unwrap(),
        at("edge.example.com", 8443)
    );
    assert_eq!(
        parse_url("edge.example.com:9443").unwrap(),
        at("edge.example.com", 9443)
    );
    assert_eq!(parse_url("https://[::1]:8443").unwrap(), at("::1", 8443));
    assert!(parse_url("http://edge.example.com").is_err());
    assert!(parse_url("https://").is_err());
}

#[test]
fn a_pairing_code_matches_however_it_is_typed() {
    assert_eq!(normalize_code("abcd-EFGH-2345"), "ABCDEFGH2345");
    assert_eq!(normalize_code(" ABCD EFGH 2345 "), "ABCDEFGH2345");
}

#[test]
fn node_ids_have_no_spaces_or_control_characters_and_a_length_limit() {
    use super::valid_node_id;
    assert!(valid_node_id("local-1"));
    assert!(valid_node_id(&"a".repeat(40)));
    assert!(!valid_node_id(""));
    assert!(!valid_node_id(&"a".repeat(41)));
    assert!(!valid_node_id("local 1"));
    assert!(!valid_node_id("local\n1"));
    assert!(!valid_node_id("local\u{7}"));
}

#[test]
fn debug_output_redacts_keys_and_codes() {
    let key = super::new_key().unwrap();
    let local = super::LocalConfig::new("https://edge.test", &key, "ab", "local-1");
    let shown = format!("{local:?}");
    assert!(!shown.contains(&key) && shown.contains("<redacted>"), "{shown}");
    let pairing = super::PairConfig::new("https://edge.test", "SECRET-CODE", "local-1");
    let shown = format!("{pairing:?}");
    assert!(!shown.contains("SECRET-CODE"), "{shown}");
}

#[test]
fn the_calls_offer_reads_back_and_one_with_no_media_address_is_not_available() {
    let calls = super::proto::CallsOffer {
        available: true,
        media_address: "203.0.113.5:10000".to_string(),
        turn_urls: vec!["turn:203.0.113.5:3478?transport=udp".to_string()],
        other_nodes: true,
    };
    let offer = super::offer::offer_of(super::proto::Offer {
        calls: Some(calls.clone()),
        ..super::proto::Offer::default()
    });
    assert!(offer.calls.available && offer.calls.other_nodes, "{:?}", offer.calls);
    assert_eq!(offer.calls.media_address, "203.0.113.5:10000");
    assert_eq!(offer.calls.turn_urls, calls.turn_urls);
    let no_address = super::offer::offer_of(super::proto::Offer {
        calls: Some(super::proto::CallsOffer {
            media_address: String::new(),
            ..calls
        }),
        ..super::proto::Offer::default()
    });
    assert!(!no_address.calls.available);
    assert!(
        !super::offer::offer_of(super::proto::Offer::default()).calls.available,
        "an edge node from before the calls area"
    );
}

#[test]
fn the_sip_offer_reads_back_and_one_with_no_address_is_not_available() {
    let sip = super::proto::SipOffer {
        available: true,
        sip_address: "203.0.113.5:5060".to_string(),
        tls_address: "203.0.113.5:5061".to_string(),
        domain: "sip.example.com".to_string(),
        registrar: true,
        trunks: vec![super::proto::SipTrunk {
            voice_trunk_id: "T1".to_string(),
            name: "Main".to_string(),
            state: "failed".to_string(),
            error: "403 Forbidden".to_string(),
        }],
        fallbacks: 3,
        other_nodes: true,
    };
    let offer = super::offer::offer_of(super::proto::Offer {
        sip: Some(sip.clone()),
        ..super::proto::Offer::default()
    });
    assert!(
        offer.sip.available && offer.sip.registrar && offer.sip.other_nodes,
        "{:?}",
        offer.sip
    );
    assert_eq!(
        (
            offer.sip.sip_address.as_str(),
            offer.sip.tls_address.as_str(),
            offer.sip.domain.as_str()
        ),
        ("203.0.113.5:5060", "203.0.113.5:5061", "sip.example.com")
    );
    assert_eq!(offer.sip.fallbacks, 3);
    let trunk = &offer.sip.trunks[0];
    assert_eq!(
        (
            trunk.voice_trunk_id.as_str(),
            trunk.name.as_str(),
            trunk.state.as_str(),
            trunk.error.as_str()
        ),
        ("T1", "Main", "failed", "403 Forbidden")
    );
    let no_address = super::offer::offer_of(super::proto::Offer {
        sip: Some(super::proto::SipOffer {
            sip_address: String::new(),
            tls_address: String::new(),
            ..sip
        }),
        ..super::proto::Offer::default()
    });
    assert!(!no_address.sip.available);
    assert!(
        !super::offer::offer_of(super::proto::Offer::default()).sip.available,
        "an edge node from before the phones area"
    );
}

#[test]
fn an_edge_member_is_the_one_dialed_and_never_this_node() {
    // on a link to the URL this node paired with, any member of the edge cluster: they share the pinned link key
    assert!(check_edge_member(None, "edge-1", "local-1").is_ok());
    assert!(check_edge_member(None, "edge-2", "local-1").is_ok());
    // on a link dialed for a member of the list, that member alone (plans/Edge-Node2.md §4.6)
    assert!(check_edge_member(Some("edge-1"), "edge-1", "local-1").is_ok());
    assert!(check_edge_member(Some("edge-1"), "edge-2", "local-1").is_err_and(|err| err.contains("dialed as edge-1")));
    assert!(check_edge_member(None, "local-1", "local-1").is_err_and(|err| err.contains("own instance id")));
    assert!(check_edge_member(None, "", "local-1").is_err());
    assert!(check_edge_member(None, "edge 1", "local-1").is_err());
}

#[test]
fn each_role_offers_only_its_own_sets() {
    use moqui_core::edge::NodeRole;
    let full = || super::proto::Offer {
        web: Some(super::proto::WebOffer {
            public_names: vec!["edge.example.com".to_string()],
            modes: vec!["EdgeHostTerminate".to_string()],
        }),
        mail: Some(super::proto::MailOffer {
            available: true,
            ..Default::default()
        }),
        xmpp: Some(super::proto::XmppOffer {
            available: true,
            port: 5222,
            ..Default::default()
        }),
        calls: Some(super::proto::CallsOffer {
            available: true,
            ..Default::default()
        }),
        ssh: Some(super::proto::SshOffer {
            available: true,
            host: "edge.example.com".to_string(),
            port: 2222,
        }),
        ..Default::default()
    };
    // plans/Edge-Node2.md §4.3: a front node web access and ports only, an edge node behind a front none of them
    let mut front = full();
    super::offer::for_role(&mut front, NodeRole::Front);
    assert!(front.web.as_ref().is_some_and(|web| !web.modes.is_empty()));
    assert!(front.xmpp.as_ref().is_some_and(|xmpp| xmpp.available));
    assert!(front.ssh.as_ref().is_some_and(|ssh| ssh.available));
    assert!(!front.mail.as_ref().is_some_and(|mail| mail.available));
    assert!(!front.calls.as_ref().is_some_and(|calls| calls.available));
    assert!(front.not_available.iter().any(|area| area == "mail"));
    let mut edge = full();
    super::offer::for_role(&mut edge, NodeRole::Edge);
    assert!(edge
        .web
        .as_ref()
        .is_some_and(|web| web.modes.is_empty() && web.public_names.is_empty()));
    assert!(!edge.xmpp.as_ref().is_some_and(|xmpp| xmpp.available));
    assert!(!edge.ssh.as_ref().is_some_and(|ssh| ssh.available));
    assert!(edge.mail.as_ref().is_some_and(|mail| mail.available));
    assert!(edge.not_available.iter().any(|area| area == "web"));
    for role in [NodeRole::FrontEdge, NodeRole::AllInOne] {
        let mut both = full();
        super::offer::for_role(&mut both, role);
        assert_eq!(both, full(), "{role:?}");
    }
}

#[test]
fn a_member_status_crosses_the_link_and_back() {
    use moqui_core::edge::{ClusterMemberInfo, MemberState};
    let members = vec![ClusterMemberInfo {
        instance_id: "member-a".to_string(),
        heartbeat_age_ms: 1200,
        live: true,
        version: "0.1.0".to_string(),
        key_fingerprint: "ab".repeat(32),
    }];
    let status = super::status_to_proto(MemberState::Draining, &members);
    assert_eq!(status.state, "draining");
    assert_eq!(
        super::status_from_proto(&status),
        (Some(MemberState::Draining), members)
    );
    let unknown = super::proto::MemberStatus {
        state: "asleep".to_string(),
        members: Vec::new(),
    };
    assert_eq!(super::status_from_proto(&unknown).0, None);
}

#[test]
fn a_member_key_proves_one_connection_and_is_kept_by_its_instance() {
    use super::member_key::{fingerprint, verify, MemberKey};
    let key = MemberKey::generate().unwrap();
    let binding = [7_u8; 32];
    let proof = key.prove(&binding);
    assert!(verify(&key.public(), &binding, &proof));
    assert!(!verify(&key.public(), &[8_u8; 32], &proof), "another connection's");
    let other = MemberKey::generate().unwrap();
    assert!(!verify(&other.public(), &binding, &proof), "another key's");
    assert!(!verify(&[], &binding, &proof));
    assert_eq!(fingerprint(&key.public()), key.fingerprint());
    // kept under the runtime, the same each time it is read, never in .env
    let runtime = std::env::temp_dir().join(format!("moqui-member-key-{}", std::process::id()));
    let made = MemberKey::load_or_create(&runtime).unwrap();
    let read = MemberKey::load_or_create(&runtime).unwrap();
    assert_eq!(made.fingerprint(), read.fingerprint());
    assert!(runtime.join("db/edge_member_key").is_file());
    let _ = std::fs::remove_dir_all(&runtime);
}

#[test]
fn a_member_key_file_that_is_not_one_names_itself_and_the_remedy() {
    use super::member_key::MemberKey;
    let runtime = std::env::temp_dir().join(format!("moqui-member-key-bad-{}", std::process::id()));
    std::fs::create_dir_all(runtime.join("db")).unwrap();
    std::fs::write(runtime.join("db/edge_member_key"), "not a key\n").unwrap();
    let err = MemberKey::load_or_create(&runtime).unwrap_err();
    assert!(
        err.contains("edge_member_key") && err.contains("move it aside"),
        "{err}"
    );
    // a new one is written whole, owner only, and read back the same
    std::fs::remove_file(runtime.join("db/edge_member_key")).unwrap();
    let made = MemberKey::load_or_create(&runtime).unwrap();
    assert_eq!(
        MemberKey::load_or_create(&runtime).unwrap().fingerprint(),
        made.fingerprint()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(runtime.join("db/edge_member_key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }
    assert!(
        std::fs::read_dir(runtime.join("db")).unwrap().count() == 1,
        "no part file left"
    );
    // a part file planted as a link (its name is predictable) is replaced, never written through (SEC-OUT-85)
    #[cfg(unix)]
    {
        std::fs::remove_file(runtime.join("db/edge_member_key")).unwrap();
        let elsewhere = runtime.join("elsewhere");
        std::fs::write(&elsewhere, "").unwrap();
        let part = runtime.join(format!("db/edge_member_key.part-{}", std::process::id()));
        std::os::unix::fs::symlink(&elsewhere, &part).unwrap();
        MemberKey::load_or_create(&runtime).unwrap();
        assert_eq!(
            std::fs::read_to_string(&elsewhere).unwrap(),
            "",
            "the link's target is not written"
        );
    }
    let _ = std::fs::remove_dir_all(&runtime);
}

/// The SSH tunnel offered to a local node (E16) reaches its services as the host and port its clone URLs name; a port
/// that is not one, or none, is no tunnel.
#[test]
fn the_ssh_tunnel_of_an_offer_is_its_host_and_port() {
    let offer = |available: bool, port: u32| super::proto::Offer {
        ssh: Some(super::proto::SshOffer {
            available,
            host: "edge.example.com".to_string(),
            port,
        }),
        ..Default::default()
    };
    let ssh = super::offer::offer_of(offer(true, 2222)).ssh;
    assert!(ssh.available);
    assert_eq!((ssh.host.as_str(), ssh.port), ("edge.example.com", 2222));
    assert!(!super::offer::offer_of(offer(true, 70_000)).ssh.available, "not a port");
    assert!(!super::offer::offer_of(offer(false, 22)).ssh.available);
    assert!(!super::offer::offer_of(super::proto::Offer::default()).ssh.available);
}

/// When the pairing's ports cannot be read (here, no entity facade), the SSH offer names no port: the conf's tunnel
/// port may be another pairing's, so it is not offered as if this node were the one pairing.
#[test]
fn an_ssh_offer_whose_ports_were_not_read_names_no_port() {
    let context = moqui_core::context::ExecutionContextFactory::test_context();
    let mut ssh = super::proto::SshOffer {
        available: true,
        host: "edge.example.com".to_string(),
        port: 2222,
    };
    super::offer::ssh_node(context.factory(), "local-1", &mut ssh);
    assert_eq!(ssh.port, 0);
    assert!(!ssh.available);
}

/// SEC-CLU-02: a refusal because the receiver's pairing is paused at a cap is the receiver's pause to a delivery, tried
/// again uncounted, so the item is not failed for it; a request refused for itself is still a counted refusal.
#[test]
fn a_paused_pairing_s_refusal_is_a_pause_and_not_a_counted_failure() {
    use moqui_core::edge::queue::Applied;
    let paused = super::queue::paused_status("its replicated records would be 11, over its cap of 10");
    assert_eq!(
        paused.code(),
        tonic::Code::Unavailable,
        "a sender's Replicate tries it again"
    );
    assert_eq!(super::queue::delivery_error(&paused), Ok(Applied::Paused));
    let refused = tonic::Status::resource_exhausted("too large");
    assert!(matches!(super::queue::delivery_error(&refused), Ok(Applied::Failed(_))));
    assert!(
        super::queue::delivery_error(&tonic::Status::unavailable("the link")).is_err(),
        "no answer"
    );
}

/// SEC-CLU-83: a pairing has at most `edge@content-calls` content calls open at once, each holding its content in
/// memory; another is refused for now, and a permit given back makes room. Pairings are counted apart.
#[test]
fn a_pairings_content_calls_at_once_are_capped() {
    let first = super::queue::content_permit("cap-node-a", 2).unwrap();
    let _second = super::queue::content_permit("cap-node-a", 2).unwrap();
    let refused = super::queue::content_permit("cap-node-a", 2).unwrap_err();
    assert_eq!(refused.code(), tonic::Code::Unavailable, "{refused:?}");
    assert!(
        super::queue::delivery_error(&refused).is_err(),
        "no answer: the item waits uncounted"
    );
    let _other = super::queue::content_permit("cap-node-b", 2).expect("another pairing's calls are its own");
    drop(first);
    let _third = super::queue::content_permit("cap-node-a", 2).expect("room again");
}

/// SEC-CLU-82: a revoked id pairs again only with a code made for it, whatever key it brings, its own revoked one too;
/// an id paired before with another key likewise; a new id with any code, an active one with its own key.
#[test]
fn a_revoked_id_pairs_again_only_with_a_code_made_for_it() {
    use super::store::re_pair_refusal;
    let revoked = Some("EdgeClientRevoked");
    let active = Some("EdgeClientActive");
    assert!(
        re_pair_refusal("local-a", revoked, false, "").is_some(),
        "the same revoked key, any code"
    );
    assert!(
        re_pair_refusal("local-a", revoked, false, "local-b").is_some(),
        "a code for another node"
    );
    assert!(
        re_pair_refusal("local-a", revoked, true, "").is_some(),
        "a new key, any code"
    );
    assert!(re_pair_refusal("local-a", revoked, false, "local-a").is_none());
    assert!(re_pair_refusal("local-a", revoked, true, "local-a").is_none());
    assert!(
        re_pair_refusal("local-a", active, true, "local-a").is_some(),
        "revoke it first"
    );
    assert!(
        re_pair_refusal("local-a", active, false, "").is_none(),
        "its own key again"
    );
    assert!(re_pair_refusal("local-new", None, false, "").is_none(), "a new id");
}

fn chunk(item: &str, data: &[u8], total: u64) -> super::proto::ContentChunk {
    super::proto::ContentChunk {
        generation: 1,
        queue_item_id: item.to_string(),
        data: data.to_vec(),
        total_size: total,
    }
}

/// SEC-CLU-83, through `read_content`: the permit is taken before the stream is read (a call past them reads nothing,
/// uncounted), the named size is asked of the cap before the chunks are read, a call that sends more than it named is
/// refused (counted), and one whose next chunk does not come, or that takes too long in all, ends (uncounted).
#[tokio::test]
async fn a_content_call_is_bounded_in_its_order() {
    use super::queue::{read_content, ContentLimits};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    let limits = ContentLimits {
        calls: 1,
        idle: Duration::from_millis(200),
        whole: Duration::from_secs(5),
    };
    let ok = |_| Ok(());
    // the permit first: no chunk is read while the pairing's calls are taken
    let held = super::queue::content_permit("read-node-a", 1).unwrap();
    let polled = AtomicBool::new(false);
    let mut watched = futures_util::stream::poll_fn(|_| {
        polled.store(true, Ordering::SeqCst);
        std::task::Poll::Ready(None)
    });
    let refused = read_content("read-node-a", &mut watched, limits, ok, async |_| Ok(()))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), tonic::Code::Unavailable);
    assert!(!polled.load(Ordering::SeqCst), "nothing read past the permits");
    drop(held);
    // the named size is asked before the chunks; then one past it is refused
    let asked = AtomicU64::new(0);
    let mut over = futures_util::stream::iter(vec![Ok(chunk("item-1", b"abc", 4)), Ok(chunk("item-1", b"def", 0))]);
    let refused = read_content("read-node-a", &mut over, limits, ok, async |size| {
        asked.store(size, Ordering::SeqCst);
        Ok(())
    })
    .await
    .unwrap_err();
    assert_eq!(asked.load(Ordering::SeqCst), 4);
    assert_eq!(refused.code(), tonic::Code::InvalidArgument, "{refused:?}");
    assert!(matches!(
        super::queue::delivery_error(&refused),
        Ok(moqui_core::edge::queue::Applied::Failed(_))
    ));
    // a cap that refuses: nothing past the first chunk is read
    let mut paused = futures_util::stream::iter(vec![Ok(chunk("item-1", b"ab", 2))]);
    let refused = read_content("read-node-a", &mut paused, limits, ok, async |_| {
        Err(super::queue::paused_status("over its cap"))
    })
    .await
    .unwrap_err();
    assert!(matches!(
        super::queue::delivery_error(&refused),
        Ok(moqui_core::edge::queue::Applied::Paused)
    ));
    // a call whose next chunk does not come ends after the idle wait, uncounted
    use futures_util::StreamExt as _;
    let mut stalled =
        futures_util::stream::iter(vec![Ok(chunk("item-1", b"ab", 4))]).chain(futures_util::stream::pending());
    let started = std::time::Instant::now();
    let refused = read_content("read-node-a", &mut stalled, limits, ok, async |_| Ok(()))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), tonic::Code::DeadlineExceeded);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(super::queue::delivery_error(&refused).is_err());
    // one that sends a byte now and then, each within the idle wait, ends at the whole call's limit
    let slow_limits = ContentLimits {
        whole: Duration::from_millis(400),
        ..limits
    };
    let mut trickle = futures_util::stream::unfold(0_u64, |sent| async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Some((Ok(chunk("item-1", b"x", if sent == 0 { 1000 } else { 0 })), sent + 1))
    })
    .boxed();
    let refused = read_content("read-node-a", &mut trickle, slow_limits, ok, async |_| Ok(()))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), tonic::Code::DeadlineExceeded, "{refused:?}");
    assert!(refused.message().contains("content-call-seconds"), "{refused:?}");
    // and a whole call, within its size, is read
    let mut whole = futures_util::stream::iter(vec![Ok(chunk("item-1", b"ab", 4)), Ok(chunk("item-1", b"cd", 0))]);
    let (id, content) = read_content("read-node-a", &mut whole, limits, ok, async |_| Ok(()))
        .await
        .unwrap();
    assert_eq!((id.as_str(), content.as_slice()), ("item-1", b"abcd".as_slice()));
}
