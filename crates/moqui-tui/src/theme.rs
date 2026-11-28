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

//! The client's color roles and terminal capabilities (plans/archive/Screen-TUI-Impl.md 1.4). Revue's widgets use fixed colors,
//! so the client keeps its own roles and the draw layer applies them. Color is never the only signal: every cue also has
//! a glyph or text, so monochrome (`NO_COLOR`) is reverse and underline only, and the ASCII mode swaps box drawing and
//! symbols for plain characters.

use std::collections::BTreeMap;

/// What a color means, not which color it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Text,
    Primary,
    Secondary,
    Positive,
    Negative,
    Warning,
    Info,
    Muted,
    Field,
    Focus,
    Changed,
    Selection,
    Border,
}

impl Role {
    pub const ALL: [Self; 13] = [
        Self::Text,
        Self::Primary,
        Self::Secondary,
        Self::Positive,
        Self::Negative,
        Self::Warning,
        Self::Info,
        Self::Muted,
        Self::Field,
        Self::Focus,
        Self::Changed,
        Self::Selection,
        Self::Border,
    ];

    /// The name a theme's `STRT_TUI_COLORS` row uses.
    pub fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::Positive => "positive",
            Self::Negative => "negative",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Muted => "muted",
            Self::Field => "field",
            Self::Focus => "focus",
            Self::Changed => "changed",
            Self::Selection => "selection",
            Self::Border => "border",
        }
    }

    /// A Quasar `btn-type`, `type` or status color name, as a role.
    pub fn of_type(name: &str) -> Option<Self> {
        Some(match name {
            "primary" => Self::Primary,
            "secondary" | "default" => Self::Secondary,
            "success" | "positive" => Self::Positive,
            "danger" | "negative" | "error" => Self::Negative,
            "warning" => Self::Warning,
            "info" => Self::Info,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// How many colors the terminal shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// `NO_COLOR`, or a terminal without color: reverse and underline only.
    Mono,
    Ansi16,
    Ansi256,
    TrueColor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub depth: Depth,
    /// Box drawing and symbol glyphs are swapped for `+ - |`, `v`, `*`, `>`.
    pub ascii: bool,
    pub dark: bool,
    colors: BTreeMap<Role, Rgb>,
}

impl Default for Theme {
    fn default() -> Self {
        Self::built_in(true)
    }
}

impl Theme {
    /// The built-in dark or light pair.
    pub fn built_in(dark: bool) -> Self {
        let pairs: [(Role, Rgb, Rgb); 13] = [
            (Role::Text, Rgb(220, 220, 220), Rgb(30, 30, 30)),
            (Role::Primary, Rgb(100, 160, 255), Rgb(25, 118, 210)),
            (Role::Secondary, Rgb(160, 160, 200), Rgb(90, 90, 140)),
            (Role::Positive, Rgb(110, 200, 110), Rgb(33, 140, 60)),
            (Role::Negative, Rgb(240, 100, 100), Rgb(193, 0, 21)),
            (Role::Warning, Rgb(240, 190, 80), Rgb(170, 110, 0)),
            (Role::Info, Rgb(90, 200, 220), Rgb(0, 120, 150)),
            (Role::Muted, Rgb(130, 130, 130), Rgb(120, 120, 120)),
            (Role::Field, Rgb(235, 235, 160), Rgb(60, 60, 140)),
            (Role::Focus, Rgb(255, 255, 255), Rgb(0, 0, 0)),
            (Role::Changed, Rgb(120, 170, 255), Rgb(20, 90, 200)),
            (Role::Selection, Rgb(60, 80, 120), Rgb(200, 220, 250)),
            (Role::Border, Rgb(110, 110, 110), Rgb(150, 150, 150)),
        ];
        Self {
            depth: Depth::TrueColor,
            ascii: false,
            dark,
            colors: pairs
                .into_iter()
                .map(|(role, on_dark, on_light)| (role, if dark { on_dark } else { on_light }))
                .collect(),
        }
    }

    /// The built-in pair for a dark or light background, keeping this theme's depth and ASCII choice.
    pub fn with_background(&self, dark: bool) -> Self {
        Self {
            depth: self.depth,
            ascii: self.ascii,
            ..Self::built_in(dark)
        }
    }

    /// The theme for this terminal: `NO_COLOR` is monochrome, `COLORTERM` says true color, `TERM` 256 colors; `linux`
    /// and `vt100` (or `--ascii`) draw in ASCII.
    pub fn detect(env: &dyn Fn(&str) -> Option<String>, ascii: bool, no_color: bool) -> Self {
        let mut theme = Self::built_in(!env("COLORFGBG").is_some_and(|value| light_background(&value)));
        let term = env("TERM").unwrap_or_default();
        theme.depth = if no_color || env("NO_COLOR").is_some_and(|value| !value.is_empty()) || term == "dumb" {
            Depth::Mono
        } else if env("COLORTERM").is_some_and(|value| value == "truecolor" || value == "24bit") {
            Depth::TrueColor
        } else if term.contains("256color") {
            Depth::Ansi256
        } else {
            Depth::Ansi16
        };
        theme.ascii = ascii || term == "linux" || term.starts_with("vt");
        theme
    }

    /// The server theme's colors over the built-in ones. A name may start `dark.` or `light.` to apply on that background
    /// only, and wins over the same role without one. A value that is not a color is left out, with the reason.
    pub fn merge_colors(&mut self, colors: &BTreeMap<String, String>) -> Vec<String> {
        let mut skipped = Vec::new();
        let this = if self.dark { "dark." } else { "light." };
        let plain = colors
            .iter()
            .filter(|(name, _)| !name.starts_with("dark.") && !name.starts_with("light."));
        let chosen = colors
            .iter()
            .filter_map(|(name, value)| name.strip_prefix(this).map(|bare| (bare, value)));
        for (name, value) in plain.map(|(name, value)| (name.as_str(), value)).chain(chosen) {
            let role = Role::ALL.into_iter().find(|role| role.name() == name);
            match (role, parse_color(value)) {
                (Some(role), Some(rgb)) => {
                    self.colors.insert(role, rgb);
                }
                (None, _) => skipped.push(format!("color role {name} is not known")),
                (_, None) => skipped.push(format!("color {name} = {value} is not #rrggbb")),
            }
        }
        skipped
    }

    /// The color of a role, or None in monochrome.
    pub fn color(&self, role: Role) -> Option<Rgb> {
        self.fit(self.colors.get(&role).copied()?)
    }

    /// A color as this terminal shows it: the nearest of its 16 or 256, as [`sgr_color`] writes it, or None in
    /// monochrome.
    pub fn fit(&self, rgb: Rgb) -> Option<Rgb> {
        match self.depth {
            Depth::Mono => None,
            Depth::Ansi16 => Some(nearest_16(rgb)),
            Depth::Ansi256 => Some(nearest_256(rgb)),
            Depth::TrueColor => Some(rgb),
        }
    }

    /// The background the theme's colors are drawn on, as near as the client knows it: the terminal says only whether
    /// its own is dark (`COLORFGBG`, OSC 11), so a near black or white.
    pub fn background(&self) -> Rgb {
        if self.dark {
            BACKGROUND_DARK
        } else {
            BACKGROUND_LIGHT
        }
    }

    /// A document's own colors (a sheet's font color and fill, plans/IronCalc-DB2.md CG3.1) as this terminal draws
    /// them in place of `role`'s: the text color, and the fill (None for none), at the terminal's depth; (None, None)
    /// in monochrome. The rule, so a sheet made on white paper reads on a dark terminal and the other way:
    ///
    /// - A fill is drawn, unless it is within [`SAME_AS_PAPER`] of the theme's background, where it would show nothing.
    /// - The text color (the document's, else the role's) is drawn when it has [`READABLE`] contrast with what is behind
    ///   it (the fill, else the theme's background). Else the role's color is, when it has; else black or white,
    ///   whichever has more contrast with the fill.
    ///
    /// The contrast is WCAG 2's ratio of relative luminance, taken on the colors as the terminal shows them (after
    /// [`Self::fit`]). Monochrome draws none.
    pub fn readable(&self, color: Option<Rgb>, fill: Option<Rgb>, role: Option<Role>) -> (Option<Rgb>, Option<Rgb>) {
        if self.mono() {
            return (None, None);
        }
        let paper = self.background();
        let fill = fill
            .and_then(|fill| self.fit(fill))
            .filter(|fill| contrast(*fill, paper) >= SAME_AS_PAPER);
        let behind = fill.unwrap_or(paper);
        let theirs = role.and_then(|role| self.color(role));
        let reads = |text: Option<Rgb>| text.filter(|text| contrast(*text, behind) >= READABLE);
        let text = reads(color.and_then(|color| self.fit(color)))
            .or_else(|| reads(theirs))
            .unwrap_or_else(|| {
                let (black, white) = (Rgb(0, 0, 0), Rgb(255, 255, 255));
                if contrast(black, behind) >= contrast(white, behind) {
                    black
                } else {
                    white
                }
            });
        (Some(text), fill)
    }

    pub fn mono(&self) -> bool {
        self.depth == Depth::Mono
    }

    /// A glyph, or its ASCII stand-in.
    pub fn glyph(&self, glyph: Glyph) -> &'static str {
        match (glyph, self.ascii) {
            (Glyph::DropDown, false) => "▾",
            (Glyph::DropDown, true) => "v",
            (Glyph::Changed, false) => "•",
            (Glyph::Changed, true) => "*",
            (Glyph::Error, _) => "!",
            (Glyph::Crumb, false) => "▸",
            (Glyph::Crumb, true) => ">",
            (Glyph::Menu, false) => "≡",
            (Glyph::Menu, true) => "=",
            (Glyph::Open, false) => "▾",
            (Glyph::Open, true) => "v",
            (Glyph::Closed, false) => "▸",
            (Glyph::Closed, true) => ">",
            (Glyph::Active, false) => "•",
            (Glyph::Active, true) => "*",
            (Glyph::Apps, false) => "◂",
            (Glyph::Apps, true) => "<",
            (Glyph::Row, false) => "▌",
            (Glyph::Row, true) => ">",
            (Glyph::Up, false) => "▲",
            (Glyph::Up, true) => "^",
            (Glyph::Down, false) => "▼",
            (Glyph::Down, true) => "v",
            (Glyph::ScrollLeft, false) => "◂",
            (Glyph::ScrollLeft, true) => "<",
            (Glyph::ScrollRight, false) => "▸",
            (Glyph::ScrollRight, true) => ">",
            (Glyph::Image, false) => "▣",
            (Glyph::Image, true) => "#",
            (Glyph::Ellipsis, false) => "…",
            (Glyph::Ellipsis, true) => "~",
            (Glyph::Bullet, false) => "•",
            (Glyph::Bullet, true) => "*",
            (Glyph::Rule, false) => "─",
            (Glyph::Rule, true) => "-",
            (Glyph::Done, false) => "✓",
            (Glyph::Done, true) => "x",
            (Glyph::Current, false) => "●",
            (Glyph::Current, true) => "*",
            (Glyph::Todo, false) => "○",
            (Glyph::Todo, true) => "o",
            (Glyph::Leaf, false) => "·",
            (Glyph::Leaf, true) => "-",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    DropDown,
    Changed,
    Error,
    Crumb,
    Menu,
    Open,
    Closed,
    /// The drawer's item for the screen shown: not a disclosure mark, so it does not read as a closed parent.
    Active,
    /// The drawer's Home, the link back to all the apps.
    Apps,
    Row,
    Up,
    Down,
    ScrollLeft,
    ScrollRight,
    Image,
    Ellipsis,
    Bullet,
    Rule,
    /// A status stage passed, a step done.
    Done,
    /// The current stage or step.
    Current,
    /// A stage or step to come.
    Todo,
    /// A tree node with no children.
    Leaf,
}

/// What [`Theme::background`] takes a dark and a light terminal's background to be.
const BACKGROUND_DARK: Rgb = Rgb(24, 24, 24);
const BACKGROUND_LIGHT: Rgb = Rgb(255, 255, 255);
/// The contrast a document's text color needs to be drawn ([`Theme::readable`]): WCAG 2's least for large text and
/// the parts of an interface, which keeps most of a sheet's colors; below it a color is hard to read.
const READABLE: f64 = 3.0;
/// The contrast below which a fill is taken for the paper itself and not drawn.
const SAME_AS_PAPER: f64 = 1.1;

/// WCAG 2's relative luminance of a color.
fn luminance(rgb: Rgb) -> f64 {
    let linear = |value: u8| {
        let value = f64::from(value) / 255.0;
        if value <= 0.039_28 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(rgb.0) + 0.7152 * linear(rgb.1) + 0.0722 * linear(rgb.2)
}

/// WCAG 2's contrast ratio of two colors, 1 (the same) to 21 (black and white).
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `COLORFGBG` is `fg;bg` with ANSI numbers; a background of 7 or 15 is light.
fn light_background(value: &str) -> bool {
    value
        .rsplit(';')
        .next()
        .and_then(|bg| bg.parse::<u8>().ok())
        .is_some_and(|bg| bg == 7 || bg == 15)
}

/// `#rrggbb`, `#rgb`, or `rgb(r, g, b)`.
pub fn parse_color(text: &str) -> Option<Rgb> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .map(|ch| ch.to_digit(16).and_then(|d| u8::try_from(d).ok()))
            .collect::<Option<_>>()?;
        return match digits.as_slice() {
            [r, g, b] => Some(Rgb(r * 17, g * 17, b * 17)),
            [r1, r2, g1, g2, b1, b2] => Some(Rgb(r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2)),
            _ => None,
        };
    }
    let inner = text.strip_prefix("rgb(")?.strip_suffix(')')?;
    let parts: Vec<u8> = inner
        .split(',')
        .map(|part| part.trim().parse().ok())
        .collect::<Option<_>>()?;
    match parts.as_slice() {
        [r, g, b] => Some(Rgb(*r, *g, *b)),
        _ => None,
    }
}

/// The 16 ANSI colors as xterm draws them.
const ANSI_16: [Rgb; 16] = [
    Rgb(0, 0, 0),
    Rgb(205, 0, 0),
    Rgb(0, 205, 0),
    Rgb(205, 205, 0),
    Rgb(0, 0, 238),
    Rgb(205, 0, 205),
    Rgb(0, 205, 205),
    Rgb(229, 229, 229),
    Rgb(127, 127, 127),
    Rgb(255, 0, 0),
    Rgb(0, 255, 0),
    Rgb(255, 255, 0),
    Rgb(92, 92, 255),
    Rgb(255, 0, 255),
    Rgb(0, 255, 255),
    Rgb(255, 255, 255),
];

fn distance(a: Rgb, b: Rgb) -> u32 {
    let d = |x: u8, y: u8| u32::from(x.abs_diff(y)).pow(2);
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

fn nearest_16(rgb: Rgb) -> Rgb {
    ANSI_16
        .into_iter()
        .min_by_key(|candidate| distance(rgb, *candidate))
        .unwrap_or(rgb)
}

/// The levels of each axis of the 6x6x6 cube of the 256-color palette.
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The index of the cube level nearest `value`.
fn cube_step(value: u8) -> u8 {
    (0u8..6)
        .min_by_key(|at| CUBE[usize::from(*at)].abs_diff(value))
        .unwrap_or(0)
}

/// The 6x6x6 cube of the 256-color palette.
fn nearest_256(rgb: Rgb) -> Rgb {
    let level = |value: u8| CUBE[usize::from(cube_step(value))];
    Rgb(level(rgb.0), level(rgb.1), level(rgb.2))
}

/// The SGR parameters that set a color at this depth: `38;2;r;g;b`, `38;5;n` from the cube, or `3n`/`9n` from the 16
/// (`4n`/`10n` for a background). None in monochrome. A session over SSH writes these itself (`ui/encode.rs`).
#[cfg_attr(not(feature = "ui"), allow(dead_code))] // the encoder of the session loop is its only caller
pub fn sgr_color(depth: Depth, rgb: Rgb, background: bool) -> Option<String> {
    let (true_color, base) = if background { (48, 40) } else { (38, 30) };
    match depth {
        Depth::Mono => None,
        Depth::TrueColor => Some(format!("{true_color};2;{};{};{}", rgb.0, rgb.1, rgb.2)),
        Depth::Ansi256 => {
            let index =
                16 + 36 * u16::from(cube_step(rgb.0)) + 6 * u16::from(cube_step(rgb.1)) + u16::from(cube_step(rgb.2));
            Some(format!("{true_color};5;{index}"))
        }
        Depth::Ansi16 => {
            let index = (0u8..16)
                .min_by_key(|at| distance(rgb, ANSI_16[usize::from(*at)]))
                .unwrap_or(7);
            Some(if index < 8 {
                (base + index).to_string()
            } else {
                (base + 60 + index - 8).to_string()
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_colors_by_depth() {
        let primary = Rgb(100, 160, 255);
        assert_eq!(
            sgr_color(Depth::TrueColor, primary, false).as_deref(),
            Some("38;2;100;160;255")
        );
        // 100 -> 95 (1), 160 -> 175 (3), 255 -> 255 (5): 16 + 36 + 18 + 5.
        assert_eq!(sgr_color(Depth::Ansi256, primary, true).as_deref(), Some("48;5;75"));
        assert_eq!(sgr_color(Depth::Ansi16, Rgb(240, 60, 60), false).as_deref(), Some("91"));
        assert_eq!(sgr_color(Depth::Ansi16, Rgb(0, 0, 0), true).as_deref(), Some("40"));
        assert_eq!(sgr_color(Depth::Mono, primary, false), None);
    }

    /// CG3.1: a document's colors through the contrast rule. A dark text on a dark terminal is drawn in the role's
    /// color, a light one on a light terminal too; a text on a fill it does not read on in black or white, whichever
    /// reads; a fill as near as the paper is not drawn; monochrome draws none, and 16 colors are judged as shown.
    #[test]
    fn a_documents_colors_are_drawn_where_they_read() {
        let dark = Theme::built_in(true);
        let light = Theme::built_in(false);
        let text = |theme: &Theme| theme.color(Role::Text);
        let navy = Rgb(0, 0, 128);
        let red = Rgb(230, 0, 0);
        let yellow = Rgb(255, 255, 0);
        // a red text reads on both, navy on light only, white text on light not at all
        assert_eq!(dark.readable(Some(red), None, Some(Role::Text)), (Some(red), None));
        assert_eq!(dark.readable(Some(navy), None, Some(Role::Text)), (text(&dark), None));
        assert_eq!(light.readable(Some(navy), None, Some(Role::Text)), (Some(navy), None));
        assert_eq!(
            light.readable(Some(Rgb(255, 255, 255)), None, Some(Role::Text)),
            (text(&light), None)
        );
        // on a yellow fill: the dark theme's light text does not read, nor does a white font; black does
        assert_eq!(
            dark.readable(None, Some(yellow), Some(Role::Text)),
            (Some(Rgb(0, 0, 0)), Some(yellow))
        );
        assert_eq!(
            dark.readable(Some(Rgb(255, 255, 255)), Some(yellow), Some(Role::Text)),
            (Some(Rgb(0, 0, 0)), Some(yellow))
        );
        // navy on yellow reads as it is, on a dark terminal too
        assert_eq!(
            dark.readable(Some(navy), Some(yellow), Some(Role::Text)),
            (Some(navy), Some(yellow))
        );
        // a white fill on a light terminal is the paper: not drawn
        assert_eq!(
            light.readable(None, Some(Rgb(255, 255, 255)), Some(Role::Text)),
            (text(&light), None)
        );
        let mut mono = Theme::built_in(true);
        mono.depth = Depth::Mono;
        assert_eq!(mono.readable(Some(red), Some(yellow), Some(Role::Text)), (None, None));
        let mut ansi = Theme::built_in(true);
        ansi.depth = Depth::Ansi16;
        assert_eq!(ansi.readable(Some(red), None, Some(Role::Text)).0, Some(Rgb(205, 0, 0)));
        assert!((contrast(Rgb(0, 0, 0), Rgb(255, 255, 255)) - 21.0).abs() < 1e-9);
        assert!((contrast(red, red) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn no_color_is_mono_and_linux_is_ascii() {
        let env = |name: &str| match name {
            "NO_COLOR" => Some("1".to_string()),
            "TERM" => Some("linux".to_string()),
            _ => None,
        };
        let theme = Theme::detect(&env, false, false);
        assert!(theme.mono());
        assert!(theme.ascii);
        assert_eq!(theme.color(Role::Primary), None);
        assert_eq!(theme.glyph(Glyph::DropDown), "v");
        let mut theme = Theme::detect(
            &|name| (name == "COLORTERM").then(|| "truecolor".to_string()),
            false,
            false,
        );
        let skipped = theme.merge_colors(&BTreeMap::from([
            ("primary".to_string(), "#123456".to_string()),
            ("bogus".to_string(), "#fff".to_string()),
            ("negative".to_string(), "red".to_string()),
        ]));
        assert_eq!(theme.color(Role::Primary), Some(Rgb(0x12, 0x34, 0x56)));
        assert_eq!(skipped.len(), 2, "{skipped:?}");
    }

    #[test]
    fn a_dark_or_light_color_wins_on_its_background_only() {
        let colors = BTreeMap::from([
            ("primary".to_string(), "#111111".to_string()),
            ("dark.primary".to_string(), "#222222".to_string()),
            ("light.info".to_string(), "#333333".to_string()),
        ]);
        let mut dark = Theme::built_in(true);
        assert!(dark.merge_colors(&colors).is_empty());
        assert_eq!(dark.color(Role::Primary), Some(Rgb(0x22, 0x22, 0x22)));
        assert_eq!(dark.color(Role::Info), Theme::built_in(true).color(Role::Info));
        let mut light = Theme::built_in(false);
        assert!(light.merge_colors(&colors).is_empty());
        assert_eq!(light.color(Role::Primary), Some(Rgb(0x11, 0x11, 0x11)));
        assert_eq!(light.color(Role::Info), Some(Rgb(0x33, 0x33, 0x33)));
        let mut mono = Theme::built_in(true).with_background(false);
        mono.depth = Depth::Mono;
        assert!(!mono.dark && mono.color(Role::Primary).is_none());
    }
}
