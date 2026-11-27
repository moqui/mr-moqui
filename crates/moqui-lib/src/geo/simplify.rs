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

//! Line and ring generalization for drawing a large overlay at a zoom (plans/archive/Maps.md §7 "Geometry/import jobs"): the
//! Douglas–Peucker simplification of a line or a ring within a tolerance, and the clipping of a ring (Sutherland–Hodgman)
//! or a line (Liang–Barsky, split where it leaves) to a tile's rectangle. Plane coordinates, the caller's units (the
//! tiler's are Web Mercator's unit square). Enough for display: a simplified ring may touch itself, and a clipped ring
//! runs along the rectangle's edge, which a renderer draws as the polygon's inside; neither is kept as data. Written
//! here rather than taken from the `geo` crate (MIT or Apache-2.0), whose algorithms are complete but bring a dozen
//! crates (`geo-types`, `robust`, `rstar`, `i_overlay`, `earcutr` and theirs) into every build for these few functions.

/// A plane position.
pub type Xy = [f64; 2];

/// The squared distance of `p` from the segment `a`–`b`.
fn segment_distance_squared(p: Xy, a: Xy, b: Xy) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (x, y) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    x * x + y * y
}

/// `points` simplified by Douglas–Peucker: the first and last kept, and every point farther than `tolerance` from the
/// line of those kept around it. A ring (first equal to last) stays closed. Iterative, so a long line takes no deep
/// recursion.
#[must_use]
pub fn douglas_peucker(points: &[Xy], tolerance: f64) -> Vec<Xy> {
    if points.len() <= 2 || tolerance <= 0.0 {
        return points.to_vec();
    }
    let limit = tolerance * tolerance;
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut spans = vec![(0usize, points.len() - 1)];
    while let Some((first, last)) = spans.pop() {
        if last <= first + 1 {
            continue;
        }
        let (mut farthest, mut at) = (0.0f64, first);
        for index in first + 1..last {
            let distance = segment_distance_squared(points[index], points[first], points[last]);
            if distance > farthest {
                farthest = distance;
                at = index;
            }
        }
        if farthest > limit {
            keep[at] = true;
            spans.push((first, at));
            spans.push((at, last));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(point, kept)| kept.then_some(*point))
        .collect()
}

/// A ring simplified for drawing: [`douglas_peucker`] of the closed ring split at its farthest point from its start
/// (so a ring's start does not decide what is kept), or `None` when fewer than three distinct points are left.
#[must_use]
pub fn simplify_ring(ring: &[Xy], tolerance: f64) -> Option<Vec<Xy>> {
    let open = if ring.len() > 1 && ring[0] == ring[ring.len() - 1] {
        &ring[..ring.len() - 1]
    } else {
        ring
    };
    if open.len() < 3 {
        return None;
    }
    let start = open[0];
    let far = (1..open.len())
        .max_by(|a, b| {
            let da = (open[*a][0] - start[0]).powi(2) + (open[*a][1] - start[1]).powi(2);
            let db = (open[*b][0] - start[0]).powi(2) + (open[*b][1] - start[1]).powi(2);
            da.total_cmp(&db)
        })
        .unwrap_or(1);
    let mut first: Vec<Xy> = douglas_peucker(&open[..=far], tolerance);
    let mut second: Vec<Xy> = open[far..].to_vec();
    second.push(start);
    let second = douglas_peucker(&second, tolerance);
    first.extend_from_slice(&second[1..]);
    // first now ends with the start again: a closed ring
    (first.len() >= 4).then_some(first)
}

/// A rectangle: west, north (the lesser y), east, south.
pub type Rect = [f64; 4];

/// `ring` (closed or not) clipped to `rect` by Sutherland–Hodgman: a closed ring, or `None` when fewer than three
/// points are left. A ring that covers the rectangle comes back as the rectangle.
#[must_use]
pub fn clip_ring(ring: &[Xy], rect: Rect) -> Option<Vec<Xy>> {
    let [x0, y0, x1, y1] = rect;
    let mut points: Vec<Xy> = if ring.len() > 1 && ring[0] == ring[ring.len() - 1] {
        ring[..ring.len() - 1].to_vec()
    } else {
        ring.to_vec()
    };
    // inside each edge in turn (its axis, its value, whether the inside is above it), and where a segment crosses it
    for (axis, edge, above) in [(0, x0, true), (0, x1, false), (1, y0, true), (1, y1, false)] {
        if points.is_empty() {
            break;
        }
        let inside = |p: Xy| if above { p[axis] >= edge } else { p[axis] <= edge };
        let cross = |a: Xy, b: Xy| {
            if axis == 0 {
                cross_x(a, b, edge)
            } else {
                cross_y(a, b, edge)
            }
        };
        let mut out = Vec::with_capacity(points.len() + 4);
        let mut previous = points[points.len() - 1];
        for &point in &points {
            match (inside(point), inside(previous)) {
                (true, true) => out.push(point),
                (true, false) => {
                    out.push(cross(previous, point));
                    out.push(point);
                }
                (false, true) => out.push(cross(previous, point)),
                (false, false) => {}
            }
            previous = point;
        }
        points = out;
    }
    points.dedup();
    if points.len() > 1 && points[0] == points[points.len() - 1] {
        points.pop();
    }
    if points.len() < 3 {
        return None;
    }
    points.push(points[0]);
    Some(points)
}

fn cross_x(a: Xy, b: Xy, x: f64) -> Xy {
    let t = (x - a[0]) / (b[0] - a[0]);
    [x, a[1] + t * (b[1] - a[1])]
}

fn cross_y(a: Xy, b: Xy, y: f64) -> Xy {
    let t = (y - a[1]) / (b[1] - a[1]);
    [a[0] + t * (b[0] - a[0]), y]
}

/// The part of segment `a`–`b` inside `rect` (Liang–Barsky): its two ends and whether each was cut (the segment
/// enters or leaves the rectangle there), or `None` when it misses the rectangle.
fn clip_segment(a: Xy, b: Xy, rect: Rect) -> Option<(Xy, bool, Xy, bool)> {
    let [x0, y0, x1, y1] = rect;
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let (mut low, mut high) = (0.0f64, 1.0f64);
    for (p, q) in [(-dx, a[0] - x0), (dx, x1 - a[0]), (-dy, a[1] - y0), (dy, y1 - a[1])] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                low = low.max(t);
            } else {
                high = high.min(t);
            }
            if low > high {
                return None;
            }
        }
    }
    let start = if low > 0.0 {
        [a[0] + low * dx, a[1] + low * dy]
    } else {
        a
    };
    let end = if high < 1.0 {
        [a[0] + high * dx, a[1] + high * dy]
    } else {
        b
    };
    Some((start, low > 0.0, end, high < 1.0))
}

/// `line` clipped to `rect`: each run of it inside, as its own line of two points or more.
#[must_use]
pub fn clip_line(line: &[Xy], rect: Rect) -> Vec<Vec<Xy>> {
    let mut out: Vec<Vec<Xy>> = Vec::new();
    let mut current: Vec<Xy> = Vec::new();
    let close = |current: &mut Vec<Xy>, out: &mut Vec<Vec<Xy>>| {
        if current.len() >= 2 {
            out.push(std::mem::take(current));
        }
        current.clear();
    };
    for pair in line.windows(2) {
        match clip_segment(pair[0], pair[1], rect) {
            Some((start, entered, end, left)) => {
                // a run goes on only from the end of the segment before, inside
                if entered || current.is_empty() {
                    close(&mut current, &mut out);
                    current.push(start);
                }
                current.push(end);
                if left {
                    close(&mut current, &mut out);
                }
            }
            None => close(&mut current, &mut out),
        }
    }
    close(&mut current, &mut out);
    out
}

#[cfg(test)]
#[path = "simplify_tests.rs"]
mod tests;
