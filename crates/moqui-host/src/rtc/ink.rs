//! Ephemeral drawing over one screen share. Geometry is measured in fractions
//! of the captured picture so zooming or letterboxing cannot change its position.
use serde_json::{json, Value as Json};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const MAX_POINTS: usize = 512;
const MAX_STROKES: usize = 128;
const FADE: Duration = Duration::from_secs(5);
const LASER: Duration = Duration::from_millis(600);

#[derive(Default)]
pub(super) struct Ink {
    strokes: VecDeque<(Instant, Json)>,
}

fn id(value: &Json) -> Option<&str> {
    value.as_str().filter(|s| {
        !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    })
}

impl Ink {
    fn prune(&mut self, now: Instant) {
        self.strokes.retain(|(at, frame)| {
            let stroke = &frame["stroke"];
            stroke["pinned"] == true
                || now.saturating_duration_since(*at) < if stroke["kind"] == "laser" { LASER } else { FADE }
        });
    }

    pub(super) fn visible(&mut self, now: Instant) -> Vec<Json> {
        self.prune(now);
        self.strokes
            .iter()
            .map(|(at, f)| {
                let mut frame = f.clone();
                frame["ageMs"] = json!(now.saturating_duration_since(*at).as_millis() as u64);
                frame
            })
            .collect()
    }

    /// accept only the dimensions and styles drawn by the application; never
    /// relay a client's markup, sender, or call address.
    pub(super) fn apply(
        &mut self,
        now: Instant,
        stage: &Json,
        host: bool,
        leg: &str,
        frame: &Json,
    ) -> Result<Json, &'static str> {
        self.prune(now);
        let owner = stage["callLegId"] == leg;
        let mode = stage["inkMode"].as_str().unwrap_or("off");
        let action = frame["action"].as_str().unwrap_or("");
        if stage["kind"] != "screen" {
            return Err("stageRequired");
        }
        if action == "clear" {
            if !host && !owner {
                return Err("notAllowed");
            }
            self.strokes.clear();
            return Ok(json!({"rtc":"ink","action":"clear","from":leg}));
        }
        if !(mode == "everyone" || (mode == "hosts" && host)) {
            return Err("notAllowed");
        }
        if action == "pin" {
            let target = id(&frame["strokeId"]).ok_or("badRequest")?;
            let (_, prior) = self
                .strokes
                .iter_mut()
                .find(|(_, prior)| prior["stroke"]["id"] == target)
                .ok_or("strokeExpired")?;
            if prior["stroke"]["kind"] == "laser" {
                return Err("badRequest");
            }
            prior["stroke"]["pinned"] = json!(true);
            return Ok(json!({"rtc":"ink","action":"pin","strokeId":target,"from":leg}));
        }
        if action != "stroke" {
            return Err("badRequest");
        }
        let s = &frame["stroke"];
        let stroke_id = id(&s["id"]).ok_or("badRequest")?;
        let kind = s["kind"]
            .as_str()
            .filter(|k| matches!(*k, "laser" | "path"))
            .ok_or("badRequest")?;
        let tool = s["tool"]
            .as_str()
            .filter(|t| matches!(*t, "draw" | "line" | "rectangle" | "ellipse"))
            .ok_or("badRequest")?;
        let color = s["color"]
            .as_str()
            .filter(|c| c.len() == 7 && c.starts_with('#') && c.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit))
            .ok_or("badRequest")?;
        let width = s["width"]
            .as_f64()
            .filter(|n| *n > 0. && *n <= 0.1)
            .ok_or("badRequest")?;
        let points = s["points"]
            .as_array()
            .filter(|p| !p.is_empty() && p.len() <= MAX_POINTS)
            .ok_or("badRequest")?;
        if !points.iter().all(|p| {
            p.as_array().is_some_and(|pair| {
                pair.len() == 2
                    && pair
                        .iter()
                        .all(|v| v.as_f64().is_some_and(|n| (0.0..=1.0).contains(&n)))
            })
        }) {
            return Err("badRequest");
        }
        let pinned = kind != "laser" && s["pinned"] == true;
        let event = json!({"rtc":"ink","action":"stroke","from":leg,"stroke":{
            "id":stroke_id,"kind":kind,"tool":tool,"color":color,"width":width,"points":points,"pinned":pinned}});
        if let Some(index) = self.strokes.iter().position(|(_, f)| f["stroke"]["id"] == stroke_id) {
            if self.strokes[index].1["from"] != leg {
                return Err("notAllowed");
            }
            self.strokes.remove(index);
        }
        if self.strokes.len() >= MAX_STROKES {
            return Err("tooManyStrokes");
        }
        self.strokes.push_back((now, event.clone()));
        Ok(event)
    }
}

#[test]
fn geometry_permissions_and_lifetime_apply_before_broadcast() {
    let now = Instant::now();
    let mut ink = Ink::default();
    let stage = json!({"kind":"screen","callLegId":"owner","inkMode":"hosts"});
    let mut request = json!({"action":"stroke","from":"owner","stroke":{"id":"one","kind":"path","color":"#112233","width":0.006,"points":[[0.2,0.3]],"tool":"draw"}});
    assert_eq!(ink.apply(now, &stage, false, "viewer", &request), Err("notAllowed"));
    request["stroke"]["points"] = json!([[3, 0]]);
    assert_eq!(ink.apply(now, &stage, true, "host", &request), Err("badRequest"));
    request["stroke"]["points"] = json!([[0.5, 0.2]]);
    assert_eq!(ink.apply(now, &stage, true, "host", &request).unwrap()["from"], "host");
    assert!(ink.visible(now + FADE).is_empty());
    request["stroke"]["pinned"] = json!(true);
    ink.apply(now, &stage, true, "host", &request).unwrap();
    assert_eq!(ink.visible(now + FADE).len(), 1);
    ink.apply(now, &stage, false, "owner", &json!({"action":"clear"}))
        .unwrap();
    assert!(ink.visible(now).is_empty());
}
