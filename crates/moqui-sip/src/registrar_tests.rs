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

//! The registrar's bindings on simulated time (V4: multiple bindings, expiry, unregister, ordering, rotation, flows).

use super::*;

const T0: u64 = 1_000_000;

fn flow(port: u16) -> Flow {
    Flow {
        transport: Transport::Udp,
        source: SocketAddr::from(([192, 0, 2, 10], port)),
    }
}

fn register(contacts: Contacts, expires: Option<u32>, call_id: &str, cseq: u32) -> Register {
    Register {
        device: "D1".to_string(),
        username: "p100".to_string(),
        revision: 1,
        contacts,
        expires,
        call_id: call_id.to_string(),
        cseq,
        flow: flow(5060),
        outbound: false,
        behind_nat: false,
        user_agent: Some("Phone/1".to_string()),
    }
}

fn contact(uri: &str, expires: Option<u32>) -> Contacts {
    Contacts::List(vec![ContactParam {
        uri: uri.to_string(),
        expires,
        ..Default::default()
    }])
}

#[test]
fn two_bindings_expire_on_their_own_and_only_live_ones_ring() {
    let mut r = Registrar::new(RegistrarLimits::default());
    let one = r
        .register(register(contact("sip:p100@10.0.0.5:5060", Some(120)), None, "a", 1), T0)
        .unwrap();
    assert_eq!(one.added.len(), 1);
    let two = r
        .register(register(contact("sip:p100@10.0.0.6:5062", Some(600)), None, "b", 1), T0)
        .unwrap();
    assert_eq!(two.current.len(), 2, "a device keeps several bindings");
    assert_eq!(r.live("p100", T0 + 60).len(), 2);
    // the first is past its expiry: it does not ring, and expire takes it
    let live = r.live("p100", T0 + 121);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].contact, "sip:p100@10.0.0.6:5062");
    let gone = r.expire(T0 + 121);
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].contact, "sip:p100@10.0.0.5:5060");
    assert_eq!(r.len(), 1);
    // a refresh keeps the binding's id
    let refreshed = r
        .register(
            register(contact("SIP:p100@10.0.0.6:5062", None), Some(300), "b", 2),
            T0 + 200,
        )
        .unwrap();
    assert_eq!(refreshed.refreshed.len(), 1);
    assert_eq!(refreshed.refreshed[0].id, two.added[0].id);
    assert_eq!(refreshed.refreshed[0].expires_at, T0 + 500);
}

#[test]
fn expiry_is_held_between_the_minimum_and_the_maximum_and_behind_a_nat_is_short() {
    let mut r = Registrar::new(RegistrarLimits::default());
    let brief = r.register(register(contact("sip:p100@10.0.0.5", Some(10)), None, "a", 1), T0);
    assert_eq!(brief.unwrap_err(), Refusal::TooBrief(60));
    assert_eq!(Refusal::TooBrief(60).status(), 423);
    assert!(r.is_empty(), "a refused request changes nothing");
    let long = r
        .register(register(contact("sip:p100@10.0.0.5", None), Some(86_400), "a", 2), T0)
        .unwrap();
    assert_eq!(long.added[0].remaining(T0), 3600);
    let default = r
        .register(register(contact("sip:p100@10.0.0.7", None), None, "c", 1), T0)
        .unwrap();
    assert_eq!(default.added[0].remaining(T0), 600);
    let mut nat = register(contact("sip:p100@192.168.1.20", Some(3600)), None, "n", 1);
    nat.behind_nat = true;
    let nat = r.register(nat, T0).unwrap();
    assert_eq!(
        nat.added[0].remaining(T0),
        60,
        "a NAT's mapping is kept by registering often"
    );
}

#[test]
fn unregister_one_contact_or_all_with_star() {
    let mut r = Registrar::new(RegistrarLimits::default());
    r.register(register(contact("sip:p100@10.0.0.5", None), None, "a", 1), T0)
        .unwrap();
    r.register(register(contact("sip:p100@10.0.0.6", None), None, "b", 1), T0)
        .unwrap();
    r.register(register(contact("sip:p100@10.0.0.7", None), None, "c", 1), T0)
        .unwrap();
    let removed = r
        .register(register(contact("sip:p100@10.0.0.5", Some(0)), None, "a", 2), T0)
        .unwrap();
    assert_eq!(removed.removed.len(), 1);
    assert_eq!(r.live("p100", T0).len(), 2);
    // * needs Expires: 0
    let bad = r.register(register(Contacts::Star, None, "d", 1), T0);
    assert_eq!(bad.unwrap_err(), Refusal::BadRequest);
    let all = r.register(register(Contacts::Star, Some(0), "d", 1), T0).unwrap();
    assert_eq!(all.removed.len(), 2);
    assert!(r.live("p100", T0).is_empty());
}

#[test]
fn a_request_out_of_order_for_its_call_id_changes_nothing() {
    let mut r = Registrar::new(RegistrarLimits::default());
    r.register(register(contact("sip:p100@10.0.0.5", None), None, "a", 5), T0)
        .unwrap();
    let stale = r.register(register(contact("sip:p100@10.0.0.5", Some(0)), None, "a", 5), T0);
    assert_eq!(stale.unwrap_err(), Refusal::OutOfOrder);
    let older = r.register(register(Contacts::Star, Some(0), "a", 4), T0);
    assert_eq!(older.unwrap_err(), Refusal::OutOfOrder);
    assert_eq!(r.live("p100", T0).len(), 1);
    // another Call-ID (a phone that restarted) may replace it
    let restarted = r
        .register(register(contact("sip:p100@10.0.0.5", None), None, "z", 1), T0)
        .unwrap();
    assert_eq!(restarted.refreshed.len(), 1);
}

#[test]
fn rotation_and_revocation_end_the_bindings() {
    let mut r = Registrar::new(RegistrarLimits::default());
    r.register(register(contact("sip:p100@10.0.0.5", None), None, "a", 1), T0)
        .unwrap();
    // a REGISTER of the new revision drops what the old one made
    let mut rotated = register(contact("sip:p100@10.0.0.6", None), None, "b", 1);
    rotated.revision = 2;
    let outcome = r.register(rotated, T0).unwrap();
    assert_eq!(outcome.removed.len(), 1);
    assert_eq!(outcome.current.len(), 1);
    assert_eq!(r.drop_device("p100").len(), 1);
    assert!(r.live("p100", T0).is_empty());
}

#[test]
fn outbound_bindings_are_keyed_by_instance_and_end_with_their_flow() {
    let mut r = Registrar::new(RegistrarLimits::default());
    let instance = "<urn:uuid:00000000-0000-1000-8000-000A95A0E128>".to_string();
    let mut request = register(
        Contacts::List(vec![ContactParam {
            uri: "sip:p100@192.168.1.20:5060;transport=tcp".to_string(),
            expires: Some(3600),
            instance: Some(instance.clone()),
            reg_id: Some(1),
        }]),
        None,
        "o",
        1,
    );
    request.outbound = true;
    request.behind_nat = true;
    request.flow = Flow {
        transport: Transport::Tcp,
        source: SocketAddr::from(([198, 51, 100, 7], 40_001)),
    };
    let outcome = r.register(request.clone(), T0).unwrap();
    assert_eq!(outcome.flow_timer, Some(120));
    assert!(outcome.added[0].outbound);
    assert_eq!(
        outcome.added[0].remaining(T0),
        3600,
        "an Outbound flow is kept alive by its keepalives"
    );
    let header = contact_header(&outcome.added[0], T0);
    assert!(header.contains("reg-id=1") && header.contains(&instance), "{header}");
    // the phone comes back on a new connection with a new contact: the same instance and reg-id is the same binding
    request.cseq = 2;
    request.flow.source = SocketAddr::from(([198, 51, 100, 7], 40_002));
    if let Contacts::List(list) = &mut request.contacts {
        list[0].uri = "sip:p100@192.168.1.20:5070;transport=tcp".to_string();
    }
    let again = r.register(request, T0 + 10).unwrap();
    assert_eq!(again.refreshed.len(), 1);
    assert_eq!(r.len(), 1);
    // the old connection closing ends nothing; the current one does
    assert!(r
        .drop_flow(Transport::Tcp, SocketAddr::from(([198, 51, 100, 7], 40_001)))
        .is_empty());
    assert_eq!(
        r.drop_flow(Transport::Tcp, SocketAddr::from(([198, 51, 100, 7], 40_002)))
            .len(),
        1
    );
    assert!(r.is_empty());
}

#[test]
fn a_device_has_at_most_its_limit_of_bindings() {
    let mut r = Registrar::new(RegistrarLimits {
        max_bindings: 2,
        ..Default::default()
    });
    r.register(register(contact("sip:p100@10.0.0.1", None), None, "a", 1), T0)
        .unwrap();
    r.register(register(contact("sip:p100@10.0.0.2", None), None, "b", 1), T0)
        .unwrap();
    let third = r.register(register(contact("sip:p100@10.0.0.3", None), None, "c", 1), T0);
    assert_eq!(third.unwrap_err(), Refusal::TooMany);
    assert_eq!(r.len(), 2);
}

#[test]
fn contacts_parse_with_their_parameters() {
    let parsed = parse_contacts(&[
        "\"Desk, front\" <sip:p100@10.0.0.5:5060;transport=udp>;expires=300;+sip.instance=\"<urn:uuid:abc>\";reg-id=2, \
         sip:p100@10.0.0.6;expires=0"
            .to_string(),
    ]);
    let Contacts::List(list) = parsed else {
        panic!("contacts")
    };
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].uri, "sip:p100@10.0.0.5:5060;transport=udp");
    assert_eq!(list[0].expires, Some(300));
    assert_eq!(list[0].instance.as_deref(), Some("<urn:uuid:abc>"));
    assert_eq!(list[0].reg_id, Some(2));
    assert_eq!(list[1].uri, "sip:p100@10.0.0.6");
    assert_eq!(list[1].expires, Some(0));
    assert_eq!(parse_contacts(&[" * ".to_string()]), Contacts::Star);
    assert_eq!(parse_contacts(&[]), Contacts::None);
}
