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

//! The driver's follow and stage rules, on a driver with no media: every leg's socket is on this node, and what
//! each would be sent is kept here ([`sent`]) instead of going to a socket.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use moqui_core::context::ExecutionContextFactory;
use moqui_rtc::{LegKey, Sfu, SfuConfig};
use serde_json::{json, Value as Json};

use super::{Driver, LegRoute, RelayBucket, RoomNotices, Route};
use crate::rtc::media::Media;

thread_local! {
    static SENT: RefCell<Vec<(u64, Json)>> = const { RefCell::new(Vec::new()) };
}

/// What the driver sends a socket on this node, kept for the test running on this thread.
pub(super) fn sent(socket: u64, frame: &Json) {
    SENT.with(|s| s.borrow_mut().push((socket, frame.clone())));
}

fn take_sent() -> Vec<(u64, Json)> {
    SENT.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

const ROOM: &str = "follow-room";
const OWN: &str = "node-1";

fn driver(factory: &Arc<ExecutionContextFactory>) -> Driver<'_> {
    Driver {
        factory,
        own: OWN.to_string(),
        sfu: Sfu::new(SfuConfig::default()),
        routes: HashMap::new(),
        tcp: None,
        revoke: None,
        media: Media::default(),
        notices: RoomNotices::default(),
        stages: HashMap::new(),
        follows: HashMap::new(),
        ink: HashMap::new(),
        origins: HashMap::new(),
        pairing_legs: 0,
    }
}

/// The socket id of each leg: its index, from 1.
fn route(socket: u64) -> Route {
    Route {
        node: OWN.to_string(),
        socket_id: socket,
    }
}

fn add_leg(d: &mut Driver<'_>, leg: &str, socket: u64, member_screen: bool, guest: bool) {
    let now = Instant::now();
    d.routes.insert(
        LegKey::new(ROOM, leg),
        LegRoute {
            host: false,
            guest,
            member_screen,
            generation: 1,
            route: route(socket),
            relayed: RelayBucket::new(now),
            direct: true,
            peer: RelayBucket::for_peer(now),
            noted: false,
        },
    );
}

fn follow_stage(revision: u64, leg: &str) -> Json {
    json!({"rtc":"stage","revision":revision,"stage":{"kind":"follow","callLegId":leg,"followEpoch":7,"requests":[]}})
}

fn navigate(sequence: u64) -> Json {
    json!({"rtc":"follow","action":"navigate","path":"/qapps/BasicComms/Home","params":{},"sequence":sequence})
}

fn to(sent: &[(u64, Json)], socket: u64, rtc: &str) -> Vec<Json> {
    sent.iter()
        .filter(|(s, f)| *s == socket && f["rtc"] == rtc)
        .map(|(_, f)| f.clone())
        .collect()
}

/// The presenter's screen goes to the other member pages only; anyone else's navigate is refused, a guest's page
/// may not even catch up, and a presenter past its frame budget gets its own code.
#[test]
fn only_the_presenter_navigates_and_only_member_pages_get_it() {
    let ctx = ExecutionContextFactory::test_context();
    let mut d = driver(ctx.factory());
    add_leg(&mut d, "P", 1, true, false);
    add_leg(&mut d, "M", 2, true, false);
    add_leg(&mut d, "G", 3, false, true);
    d.stage(ROOM, &follow_stage(2, "P"));
    take_sent();

    d.frame(route(1), ROOM, "P", 1, "", &navigate(1));
    let sent = take_sent();
    let followed = to(&sent, 2, "follow");
    assert_eq!(followed.len(), 1, "the member page follows");
    assert_eq!(followed[0]["from"], "P");
    assert_eq!(followed[0]["followEpoch"], 7);
    assert!(
        to(&sent, 3, "follow").is_empty(),
        "a guest's page gets no screen location"
    );
    assert!(to(&sent, 1, "follow").is_empty(), "nor the presenter itself");

    d.frame(route(2), ROOM, "M", 1, "", &navigate(2));
    let sent = take_sent();
    let refused = to(&sent, 2, "error");
    assert_eq!(refused.len(), 1);
    assert_eq!(
        (refused[0]["code"].as_str(), refused[0]["follow"].as_bool()),
        (Some("notAllowed"), Some(true))
    );
    assert!(
        to(&sent, 1, "follow").is_empty(),
        "a member who does not present moves no one"
    );

    d.frame(route(3), ROOM, "G", 1, "", &json!({"rtc":"follow","action":"catchUp"}));
    assert_eq!(to(&take_sent(), 3, "error")[0]["code"], "notAllowed");

    let mut codes = Vec::new();
    for sequence in 3..80 {
        d.frame(route(1), ROOM, "P", 1, "", &navigate(sequence));
        codes.extend(to(&take_sent(), 1, "error").into_iter().map(|f| f["code"].clone()));
    }
    assert_eq!(
        codes.first(),
        Some(&json!("followRate")),
        "the frame budget has its own code"
    );

    d.stage(
        ROOM,
        &json!({"rtc":"stage","revision":3,"stage":{"kind":null,"callLegId":null,"requests":[]}}),
    );
    take_sent();
    add_leg(&mut d, "P", 1, true, false);
    d.frame(route(1), ROOM, "P", 1, "", &navigate(100));
    assert_eq!(to(&take_sent(), 1, "error")[0]["code"], "stageUnavailable");
}

/// A leg that joins with a stage newer than the room's gets it once, with the rest of the room.
#[test]
fn a_joining_leg_gets_the_stage_once() {
    let ctx = ExecutionContextFactory::test_context();
    let mut d = driver(ctx.factory());
    add_leg(&mut d, "A", 1, true, false);
    d.stage(ROOM, &follow_stage(2, "A"));
    take_sent();
    let join = json!({"rtc":"join","direct":true,"boundMemberScreen":true,"boundGuest":false,
        "boundStage":follow_stage(3, "A")});
    d.frame(route(2), ROOM, "B", 1, "", &join);
    let sent = take_sent();
    assert_eq!(to(&sent, 2, "stage").len(), 1, "the joiner's stage, once");
    assert_eq!(to(&sent, 1, "stage").len(), 1, "the room hears the newer stage");
    // the same stage again is not news: the joiner still gets it, once
    let again = json!({"rtc":"join","direct":true,"boundMemberScreen":true,"boundGuest":false,
        "boundStage":follow_stage(3, "A")});
    d.frame(route(3), ROOM, "C", 1, "", &again);
    let sent = take_sent();
    assert_eq!(to(&sent, 3, "stage").len(), 1);
    assert!(to(&sent, 1, "stage").is_empty());
}

fn screen_stage(revision: u64, leg: &str, ink_mode: &str) -> Json {
    json!({"rtc":"stage","revision":revision,"stage":{"kind":"screen","callLegId":leg,"inkMode":ink_mode,
        "inkEmbedded":true,"inkEpoch":4,"requests":[]}})
}

/// A snapshot puts a board on the stage in place of the share, so it is asked only by one who may draw on the share:
/// the presenter whatever the ink mode, a host under Hosts, anyone under Everyone, never a guest. It goes to the
/// presenter's page alone, which asks its user when another asked.
#[test]
fn a_snapshot_is_asked_only_by_one_who_may_draw_and_goes_to_the_presenter() {
    let ctx = ExecutionContextFactory::test_context();
    let mut d = driver(ctx.factory());
    add_leg(&mut d, "P", 1, true, false);
    add_leg(&mut d, "M", 2, true, false);
    add_leg(&mut d, "H", 3, true, false);
    add_leg(&mut d, "G", 4, false, true);
    d.routes.get_mut(&LegKey::new(ROOM, "H")).expect("the host's leg").host = true;
    let snapshot = json!({"rtc":"ink","action":"snapshot","id":1});
    let asked = |d: &mut Driver<'_>, socket: u64, leg: &str| {
        d.frame(route(socket), ROOM, leg, 1, "", &snapshot);
        let sent = take_sent();
        let to_presenter = to(&sent, 1, "ink");
        let refused = to(&sent, socket, "error").first().map(|f| f["code"].clone());
        (to_presenter.first().map(|f| f["from"].clone()), refused)
    };

    d.stage(ROOM, &screen_stage(2, "P", "off"));
    take_sent();
    assert_eq!(
        asked(&mut d, 2, "M"),
        (None, Some(json!("notAllowed"))),
        "a member, annotation off"
    );
    assert_eq!(
        asked(&mut d, 3, "H"),
        (None, Some(json!("notAllowed"))),
        "a host, annotation off"
    );
    assert_eq!(asked(&mut d, 4, "G"), (None, Some(json!("notAllowed"))), "a guest");
    let (from, refused) = asked(&mut d, 1, "P");
    assert_eq!(
        (from, refused),
        (Some(json!("P")), None),
        "the presenter, whatever the mode"
    );

    d.stage(ROOM, &screen_stage(3, "P", "hosts"));
    take_sent();
    assert_eq!(asked(&mut d, 2, "M").1, Some(json!("notAllowed")));
    assert_eq!(asked(&mut d, 3, "H"), (Some(json!("H")), None));

    d.stage(ROOM, &screen_stage(4, "P", "everyone"));
    take_sent();
    d.frame(route(2), ROOM, "M", 1, "", &snapshot);
    let sent = take_sent();
    assert_eq!(to(&sent, 1, "ink").len(), 1, "the presenter's page is asked");
    assert_eq!(to(&sent, 1, "ink")[0]["inkEpoch"], 4);
    assert_eq!(asked(&mut d, 4, "G").1, Some(json!("notAllowed")), "never a guest");
}
