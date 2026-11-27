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

use super::*;

#[test]
fn douglas_peucker_keeps_what_passes_the_tolerance() {
    // a zig-zag of 0.1 along a line: gone at 0.5, kept at 0.05
    let line: Vec<Xy> = (0..=10)
        .map(|i| [f64::from(i), if i % 2 == 0 { 0.0 } else { 0.1 }])
        .collect();
    assert_eq!(douglas_peucker(&line, 0.5), vec![[0.0, 0.0], [10.0, 0.0]]);
    assert_eq!(douglas_peucker(&line, 0.05).len(), 11);
    // a corner stays
    let corner = vec![[0.0, 0.0], [5.0, 0.01], [10.0, 0.0], [10.0, 10.0]];
    assert_eq!(
        douglas_peucker(&corner, 1.0),
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]
    );
    assert_eq!(douglas_peucker(&corner[..2], 1.0).len(), 2);
}

#[test]
fn a_ring_stays_closed_or_goes() {
    let mut square: Vec<Xy> = vec![
        [0.0, 0.0],
        [5.0, 0.001],
        [10.0, 0.0],
        [10.0, 10.0],
        [0.0, 10.0],
        [0.0, 0.0],
    ];
    let simplified = simplify_ring(&square, 0.5).unwrap();
    assert_eq!(simplified.first(), simplified.last());
    assert_eq!(simplified.len(), 5, "{simplified:?}");
    // a sliver smaller than the tolerance has nothing left to draw
    square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 0.01], [0.0, 0.0]];
    assert!(simplify_ring(&square, 1.0).is_none());
}

#[test]
fn clips_a_ring_to_a_rectangle() {
    let rect = [0.0, 0.0, 10.0, 10.0];
    // a square over the rectangle's corner: the quarter inside
    let ring = vec![[5.0, 5.0], [15.0, 5.0], [15.0, 15.0], [5.0, 15.0], [5.0, 5.0]];
    let clipped = clip_ring(&ring, rect).unwrap();
    assert_eq!(clipped.len(), 5);
    assert_eq!(clipped.first(), clipped.last());
    for corner in [[5.0, 5.0], [10.0, 5.0], [10.0, 10.0], [5.0, 10.0]] {
        assert!(clipped.contains(&corner), "{clipped:?}");
    }
    // one over the whole rectangle is the rectangle; one outside is nothing
    let cover = vec![[-5.0, -5.0], [20.0, -5.0], [20.0, 20.0], [-5.0, 20.0]];
    assert_eq!(clip_ring(&cover, rect).unwrap().len(), 5);
    let outside = vec![[20.0, 20.0], [30.0, 20.0], [30.0, 30.0]];
    assert!(clip_ring(&outside, rect).is_none());
}

#[test]
fn clips_a_line_into_its_runs_inside() {
    let rect = [0.0, 0.0, 10.0, 10.0];
    // in, out across the top, and in again
    let line = vec![[2.0, 5.0], [5.0, 5.0], [5.0, 15.0], [8.0, 15.0], [8.0, 5.0]];
    let runs = clip_line(&line, rect);
    assert_eq!(
        runs,
        vec![vec![[2.0, 5.0], [5.0, 5.0], [5.0, 10.0]], vec![[8.0, 10.0], [8.0, 5.0]]]
    );
    assert!(clip_line(&[[20.0, 0.0], [30.0, 0.0]], rect).is_empty());
    // a segment crossing the whole rectangle
    assert_eq!(
        clip_line(&[[-5.0, 5.0], [15.0, 5.0]], rect),
        vec![vec![[0.0, 5.0], [10.0, 5.0]]]
    );
}
