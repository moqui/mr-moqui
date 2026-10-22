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

//! Draw a Gallery fixture.
//!
//! `gallery --size 72|100|180` prints one frame from an in-memory terminal
//! (72×24, 100×40, or 180×50). Without `--size` the screen runs on this terminal.

use moqui_tui::{gallery_link, run_local, snapshot, LocalOptions, Theme, GALLERY_LIST_PATH};

fn main() {
    let mut columns = None;
    let mut path = GALLERY_LIST_PATH.to_string();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--size" => {
                let raw = args.next().unwrap_or_else(|| usage());
                columns = Some(parse_columns(&raw));
            }
            "--screen" => path = args.next().unwrap_or_else(|| usage()),
            "--help" | "-h" => usage(),
            other => {
                eprintln!("unknown argument {other}");
                usage();
            }
        }
    }
    let link = gallery_link();
    let theme = Theme::detect(&|name| std::env::var(name).ok(), false, false);
    if let Some(columns) = columns {
        let (width, height) = frame(columns);
        println!("{}", snapshot(link, &path, width, height, theme));
        return;
    }
    let options = LocalOptions {
        path,
        theme,
        notes: None,
        log: None,
    };
    if let Err(err) = run_local(link, options) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn parse_columns(raw: &str) -> u16 {
    match raw {
        "72" | "100" | "180" => raw.parse().expect("size is a number"),
        _ => usage(),
    }
}

fn frame(columns: u16) -> (u16, u16) {
    match columns {
        72 => (72, 24),
        100 => (100, 40),
        180 => (180, 50),
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: gallery [--size 72|100|180] [--screen PATH]\n\
         \n\
         --size prints one frame. 72 is 72×24, 100 is 100×40, 180 is 180×50.\n\
         Without --size the screen runs on this terminal."
    );
    std::process::exit(2);
}
