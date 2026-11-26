/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
//! The world in braille: a land mask of the Natural Earth outline and points over it, for the geomap panel of a report and
//! the overview of the map component. Longitude and latitude are WGS84 degrees; the drawing spans 90°N to 60°S.

use crate::components::{Line, Span};
use crate::theme::Role;

/// Land at 2° cells, from 90°N (first row) to 60°S, 180°W to 180°E (first column the high bit): the country polygons of
/// `libs/naturalearth/countries-110m.json` (Natural Earth 110m, public domain), each cell set when its centre is on land,
/// with each ring made continuous across the antimeridian first (Russia, Fiji) and a ring around a pole left as it is.
const LAND: [&str; 75] = [
    "000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000",
    "0000000000007f800ffc0000000000000000000000000",
    "000000000017ff3fffffe000000000000040000000000",
    "00000000061df0ffffff0000f8000000003c000000000",
    "000000030027c3ffffff8000200000000006000000000",
    "0000000038afc007ffff00000000030007ffc001d8000",
    "0000000e88b7b003fffe000000000c003ffffec000000",
    "80180009fc33fc00fffa00000300041dffffffeffc801",
    "00fffbfe725d8f00fff800001ff000edfffffffffffdf",
    "e0ffffffffff87c1ff8000007ffebffdfffffffffffff",
    "31fffffffffd1f80fc01e000f9ebfffffffffffffffff",
    "00fffffffff8034078000003e7fffffffffffffffffff",
    "01fdffffffe01e0038000007e7ffffffffffffffff2f0",
    "007807ffffe01e4000000007e3ffffffffffffff82200",
    "001001fffff80fe000000182c7fffffffffffffe00f00",
    "0080007fffff9ff8000003820ffffffffffffffc00e00",
    "0000007fffff9ffc000006cfffffffffffffffffc0800",
    "0000003ffffffffc000000dfffffffffffffffff40000",
    "00000017ffffff460000007fffffffffffffffff40000",
    "0000000fffffffc10000007ffffffffffffffffe40000",
    "0000000fffffff600000007f7f97cffffffffffc00000",
    "0000000ffffffe00000007f19f03cffffffffff8c0000",
    "0000000ffffffc00000003c26f7fe7ffffffffe080000",
    "0000000ffffff000000007c02dffe7fffffff84080000",
    "00000007fffff0000000018740ffe7fffffffe6300000",
    "00000003fffff000000001fe022ffffffffffc4f00000",
    "00000001ffffc000000003ff000ffffffffffc1800000",
    "00000000ffff8000000007ffef7ffffffffffe0000000",
    "000000003fc04000000007ffffffdffffffffe0000000",
    "000000005f80400000001fffffefe7fffffffc0000000",
    "000000002f80000000001fffffe7f41ffffffc0000000",
    "000000000780c00000003ffffff7ff07fffff20000000",
    "000000000786100000007ffffffbfe07fcff000000000",
    "0000000003cc020000003ffffff9fe03f07e800000000",
    "0000000000fc000000003ffffff9f801e07f020000000",
    "00000000000f000000007ffffffde001c01f020000000",
    "000000000003000000003ffffffe8001c01f820000000",
    "0000000000010f0000003fffffff3000c013008000000",
    "000000000000aff000001fffffffe000a012000000000",
    "0000000000001ff800000fffffffe0002000008000000",
    "0000000000001fff00000687ffffc000002c180000000",
    "0000000000001fff80000001ffff80000014380000000",
    "0000000000003fff80000001ffff000000187a0000000",
    "0000000000003fffe0000003fffe0000000c781800000",
    "0000000000003ffffc000001fffc00000006762b80000",
    "0000000000007fffff000000fffc000000020101e0000",
    "0000000000003fffff800000fffc00000001c001f0800",
    "0000000000001fffff000000fffc000000000880d0200",
    "0000000000001ffffe0000007ffc00000000000000000",
    "0000000000000ffffe000000fffc20000000001c40000",
    "0000000000000ffffe000000fffc2000000000fc60040",
    "00000000000003fffc000000fff8e000000001fe60000",
    "00000000000001fffc000000fff0e000000001ffe0000",
    "00000000000001fffc0000007ff0c00000000ffff8080",
    "00000000000001fff00000007ff0c00000001ffff8000",
    "00000000000001ffc00000007fe0800000001ffffc000",
    "00000000000001ffc00000003fc0000000001ffffe000",
    "00000000000003ff800000003fc0000000001ffffe000",
    "00000000000003ff800000001f80000000000ffffe000",
    "00000000000003ff000000001f00000000000f07fc000",
    "00000000000003f8000000000000000000000801f8008",
    "00000000000007fc000000000000000000000000f8004",
    "00000000000007e000000000000000000000000000006",
    "00000000000007a00000000000000000000000003000c",
    "00000000000003c000000000000000000000000010018",
    "000000000000078000000000000000000000000000030",
    "000000000000078000000000000000000000000000000",
    "0000000000000f0000000000000000080000000000000",
    "0000000000000e0000000000000000000000000000000",
    "000000000000070000000000000000000000000000000",
    "000000000000030000000000000000000000000000000",
    "000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000",
];

pub(crate) fn land(lon: f64, lat: f64) -> bool {
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (column, row) = (
        ((lon + 180.0) / 2.0).floor() as usize,
        ((90.0 - lat) / 2.0).floor() as usize,
    );
    let Some(hex) = LAND.get(row) else {
        return false;
    };
    let column = column.min(179);
    hex.as_bytes()
        .get(column / 4)
        .and_then(|digit| (*digit as char).to_digit(16))
        .is_some_and(|digit| digit & (8 >> (column % 4)) != 0)
}

/// The southern edge of the drawing, in degrees of latitude.
pub(crate) const SOUTH_EDGE: f64 = -60.0;

/// The world at `width` columns (at most 120) and a quarter as many rows (at least 6): land as dim dots, each of `points`
/// (longitude, latitude) a raised dot in the point colour and each of `marked` in the marked colour, a cell with a point
/// showing it over the land.
pub(crate) fn braille_map(points: &[(f64, f64)], marked: &[(f64, f64)], width: usize) -> Vec<Line> {
    let columns = width.clamp(1, 120);
    let rows = (columns / 4).max(6);
    let (dots_x, dots_y) = (columns * 2, rows * 4);
    let mut grid = vec![vec![0u8; columns]; rows];
    // 0 nothing, 1 a point, 2 a marked point
    let mut kinds = vec![vec![0u8; columns]; rows];
    let mut dot = |lon: f64, lat: f64, kind: u8| {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let (x, y) = (
            (((lon + 180.0) / 360.0) * dots_x as f64)
                .floor()
                .clamp(0.0, (dots_x - 1) as f64) as usize,
            (((90.0 - lat) / 150.0) * dots_y as f64)
                .floor()
                .clamp(0.0, (dots_y - 1) as f64) as usize,
        );
        const BITS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
        grid[y / 4][x / 2] |= BITS[y % 4][x % 2];
        let cell = &mut kinds[y / 4][x / 2];
        *cell = (*cell).max(kind);
    };
    for (lon, lat) in points {
        dot(*lon, *lat, 1);
    }
    for (lon, lat) in marked {
        dot(*lon, *lat, 2);
    }
    // Land dots go under the points; a cell with a point shows it in the point colour.
    #[expect(clippy::cast_precision_loss)]
    for (r, line) in grid.iter_mut().enumerate() {
        for (c, cell) in line.iter_mut().enumerate() {
            if kinds[r][c] != 0 {
                continue;
            }
            let lon = -180.0 + (c as f64 + 0.5) * 360.0 / columns as f64;
            let lat = 90.0 - (r as f64 + 0.5) * 150.0 / rows as f64;
            if land(lon, lat) {
                *cell = 0x24;
            }
        }
    }
    grid.iter()
        .zip(&kinds)
        .map(|(line, kinds)| {
            line.iter()
                .zip(kinds)
                .map(|(bits, kind)| {
                    let glyph = char::from_u32(0x2800 + u32::from(*bits)).unwrap_or(' ');
                    let role = match kind {
                        2 => Role::Primary,
                        1 => Role::Negative,
                        _ => Role::Muted,
                    };
                    Span::new(glyph.to_string(), role)
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn land_is_the_outline_and_points_go_over_it() {
        assert!(land(2.35, 48.85), "Paris is on land");
        assert!(!land(-30.0, 30.0), "the mid Atlantic is not");
        // the rings across the antimeridian are not filled across the world
        assert!(land(100.0, 69.0), "the Siberian coast is land");
        assert!(!land(0.0, 69.0), "the Norwegian Sea is not");
        assert!(land(-30.0, 71.0), "Greenland is land at 71°N");
        assert!(!land(-5.0, 71.0), "the Greenland Sea is not");
        assert!(
            land(178.0, 67.0),
            "Chukotka, past the antimeridian of its ring, is land"
        );
        let lines = braille_map(&[(2.35, 48.85)], &[(151.21, -33.87)], 60);
        assert_eq!(lines.len(), 15);
        assert!(lines.iter().all(|line| line.len() == 60));
        let roles = |role| lines.iter().flatten().filter(|span| span.role == role).count();
        assert_eq!(roles(Role::Negative), 1, "the point");
        assert_eq!(roles(Role::Primary), 1, "the marked point");
        assert!(roles(Role::Muted) > 0, "the land and sea");
        assert!(lines.iter().flatten().any(|span| span.text == "\u{2824}"), "land dots");
    }
}
