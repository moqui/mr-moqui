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

//! Cells in theme roles. Revue's widgets draw in fixed colors, so the client draws its own cells, and Revue's `Border`
//! where a frame is wanted, all through [`Painter`].

use std::collections::BTreeMap;

use revue::layout::Rect;
use revue::render::{Cell, Modifier};
use revue::style::Color;
use revue::widget::{Border, RenderContext, View};

pub use crate::keys::Target;
use crate::layout::{char_width, display_width};
use crate::theme::{Role, Theme};

/// How a run of text looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub role: Option<Role>,
    pub bg: Option<Role>,
    pub bold: bool,
    pub dim: bool,
    pub underline: bool,
    pub reverse: bool,
    pub italic: bool,
}

impl Look {
    pub fn role(role: Role) -> Self {
        Self {
            role: Some(role),
            ..Self::default()
        }
    }

    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    pub fn reverse(mut self) -> Self {
        self.reverse = true;
        self
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub fn on(mut self, bg: Role) -> Self {
        self.bg = Some(bg);
        self
    }

    /// Reverse when `focused`: the focused field, button, row or link.
    pub fn focused(self, focused: bool) -> Self {
        if focused {
            self.reverse()
        } else {
            self
        }
    }
}

/// Where the view drew a field or a button on screen, so a drop-down opens under it.
pub type Spots = BTreeMap<String, (u16, u16)>;

/// One clickable area of a frame, in screen cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub rect: Rect,
    pub target: Target,
}

/// The areas of a frame, in drawing order: the last one drawn at a cell is on top.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hits(pub Vec<Hit>);

impl Hits {
    /// What is under the cell at `column`, `row`: the top one.
    pub fn at(&self, column: u16, row: u16) -> Option<&Target> {
        self.0
            .iter()
            .rev()
            .find(|hit| {
                column >= hit.rect.x
                    && column < hit.rect.x.saturating_add(hit.rect.width)
                    && row >= hit.rect.y
                    && row < hit.rect.y.saturating_add(hit.rect.height)
            })
            .map(|hit| &hit.target)
    }
}

pub struct Painter<'a, 'b> {
    pub ctx: &'a mut RenderContext<'b>,
    pub theme: &'a Theme,
    /// The screen rows the painter may draw on, `[top, bottom)`, and the canvas row at `top`.
    pub top: u16,
    pub bottom: u16,
    pub scroll: usize,
    /// The columns it may draw on, `[left, right)`.
    pub left: u16,
    pub right: u16,
    pub spots: &'a mut Spots,
    pub hits: &'a mut Hits,
    /// The target being drawn, and the screen cells its drawing covered so far: [`Self::target`].
    pub aim: Option<(Target, Option<Rect>)>,
}

impl Painter<'_, '_> {
    fn color(&self, role: Option<Role>) -> Option<Color> {
        let rgb = self.theme.color(role?)?;
        Some(Color::rgb(rgb.0, rgb.1, rgb.2))
    }

    fn cell(&self, ch: char, look: Look) -> Cell {
        let mut cell = Cell::new(ch);
        cell.fg = self.color(look.role);
        cell.bg = self.color(look.bg);
        let mut modifier = Modifier::empty();
        if look.bold {
            modifier |= Modifier::BOLD;
        }
        if look.dim && !self.theme.mono() {
            modifier |= Modifier::DIM;
        }
        if look.underline {
            modifier |= Modifier::UNDERLINE;
        }
        if look.reverse {
            modifier |= Modifier::REVERSE;
        }
        if look.italic {
            modifier |= Modifier::ITALIC;
        }
        cell.modifier = modifier;
        cell
    }

    /// The screen row of canvas row `y`, or None when it is scrolled out.
    pub fn row(&self, y: usize) -> Option<u16> {
        let rel = y.checked_sub(self.scroll)?;
        let row = self.top.checked_add(u16::try_from(rel).ok()?)?;
        (row < self.bottom).then_some(row)
    }

    /// Text at canvas row `y`, screen column `x`, cut at `max` columns and at the painter's right edge. Returns the
    /// columns used.
    pub fn text(&mut self, x: u16, y: usize, text: &str, look: Look, max: usize) -> usize {
        let Some(row) = self.row(y) else {
            return display_width(text).min(max);
        };
        self.put(x, row, text, look, max, None)
    }

    /// As [`Self::text`], with a terminal hyperlink (OSC 8) on the cells.
    pub fn link_text(&mut self, x: u16, y: usize, text: &str, look: Look, max: usize, url: &str) -> usize {
        let Some(row) = self.row(y) else {
            return display_width(text).min(max);
        };
        let id = safe_link(url).then(|| self.ctx.buffer.register_hyperlink(url));
        self.put(x, row, text, look, max, id)
    }

    /// Text on a screen row, outside the scrolled canvas: the title, message and key lines, the overlays.
    pub fn fixed(&mut self, x: u16, row: u16, text: &str, look: Look, max: usize) -> usize {
        self.put(x, row, text, look, max, None)
    }

    fn put(&mut self, x: u16, row: u16, text: &str, look: Look, max: usize, link: Option<u16>) -> usize {
        let right = usize::from(self.right);
        let mut col = usize::from(x);
        let stop = (usize::from(x) + max).min(right);
        let mut used = 0;
        for ch in text.chars() {
            let width = char_width(ch);
            if width == 0 {
                continue;
            }
            let ch = if self.theme.ascii { ascii_stand_in(ch) } else { ch };
            if col + width > stop {
                break;
            }
            let mut cell = self.cell(ch, look);
            cell.hyperlink_id = link;
            let Ok(cx) = u16::try_from(col) else {
                break;
            };
            self.ctx.set(cx, row, cell);
            self.cover(Rect::new(cx, row, u16::try_from(width).unwrap_or(1), 1));
            if width == 2 {
                let mut spacer = Cell::continuation();
                spacer.bg = self.color(look.bg);
                if let Ok(next) = u16::try_from(col + 1) {
                    self.ctx.set(next, row, spacer);
                }
            }
            col += width;
            used += width;
        }
        used
    }

    pub fn fill_row(&mut self, x: u16, row: u16, width: usize, look: Look) {
        let stop = (usize::from(x) + width).min(usize::from(self.right));
        for col in usize::from(x)..stop {
            if let Ok(cx) = u16::try_from(col) {
                let cell = self.cell(' ', look);
                self.ctx.set(cx, row, cell);
            }
        }
    }

    /// A frame of `w` by `h` at canvas row `y`, its title in the top edge, drawn by Revue's `Border`. A `type` gives a
    /// heavy left edge in its color.
    pub fn frame(&mut self, x: u16, y: usize, w: u16, h: usize, title: &str, edge: Option<Role>) {
        let first = (y..y + h).find_map(|row| self.row(row).map(|screen| (row, screen)));
        let Some((first_y, screen_top)) = first else {
            return;
        };
        let mut border = if self.theme.ascii {
            Border::ascii()
        } else {
            Border::single()
        };
        if !title.is_empty() {
            border = border.title(title.to_string());
        }
        if let Some(color) = self.color(Some(Role::Border)) {
            border = border.fg(color);
        }
        if first_y == y {
            // Full height, clipped to the canvas, so a frame running off the bottom has no bottom edge yet.
            let rect = Rect::new(x, screen_top, w, u16::try_from(h).unwrap_or(u16::MAX));
            let clip = Rect::new(self.left, self.top, self.right - self.left, self.bottom - self.top);
            border.render_child_into(self.ctx, rect, Some(clip));
        } else {
            // Scrolled past its top edge: only the sides and the bottom, so the title does not move down.
            for row in first_y..y + h {
                if let Some(screen) = self.row(row) {
                    let last = row + 1 == y + h;
                    let side = if self.theme.ascii { "|" } else { "│" };
                    if last {
                        let line = if self.theme.ascii {
                            format!("+{}+", "-".repeat(usize::from(w.saturating_sub(2))))
                        } else {
                            format!("└{}┘", "─".repeat(usize::from(w.saturating_sub(2))))
                        };
                        self.put(x, screen, &line, Look::role(Role::Border), usize::from(w), None);
                    } else {
                        self.put(x, screen, side, Look::role(Role::Border), 1, None);
                        self.put(x + w.saturating_sub(1), screen, side, Look::role(Role::Border), 1, None);
                    }
                }
            }
        }
        if let Some(role) = edge {
            let mark = if self.theme.ascii { "#" } else { "┃" };
            for row in y + 1..y + h.saturating_sub(1) {
                if let Some(screen) = self.row(row) {
                    self.put(x, screen, mark, Look::role(role), 1, None);
                }
            }
        }
    }

    /// A frame on screen rows, for the overlays.
    pub fn fixed_frame(&mut self, rect: Rect, title: &str) {
        let mut border = if self.theme.ascii {
            Border::ascii()
        } else {
            Border::single()
        };
        if !title.is_empty() {
            border = border.title(title.to_string());
        }
        if let Some(color) = self.color(Some(Role::Border)) {
            border = border.fg(color);
        }
        border.render_child_into(self.ctx, rect, None);
    }

    /// Blank the screen rows of `rect`, under an overlay; a click there no longer reaches what is under it.
    pub fn clear(&mut self, rect: Rect) {
        self.area(rect, Target::Block);
        for row in rect.y..rect.y.saturating_add(rect.height) {
            self.fill_row(rect.x, row, usize::from(rect.width), Look::default());
        }
    }

    /// Draw with `draw`, and record the cells it wrote as a click area of `target`. A target drawn inside another is
    /// its own area, on top.
    pub fn target<R>(&mut self, target: Target, draw: impl FnOnce(&mut Self) -> R) -> R {
        let outer = self.aim.replace((target, None));
        let result = draw(self);
        if let Some((target, Some(rect))) = std::mem::replace(&mut self.aim, outer) {
            self.hits.0.push(Hit { rect, target });
        }
        result
    }

    /// An area of `target` that is not only drawn text: an overlay's whole box, a field's width.
    pub fn area(&mut self, rect: Rect, target: Target) {
        if rect.width > 0 && rect.height > 0 {
            self.hits.0.push(Hit { rect, target });
        }
    }

    /// An area of `target` on the canvas: `width` columns at column `x`, canvas rows `y` to `y + height`, the rows
    /// scrolled out left off.
    pub fn canvas_area(&mut self, x: u16, y: usize, width: usize, height: usize, target: Target) {
        let rows: Vec<u16> = (y..y + height).filter_map(|row| self.row(row)).collect();
        if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
            let width = u16::try_from(width)
                .unwrap_or(u16::MAX)
                .min(self.right.saturating_sub(x));
            self.area(Rect::new(x, *first, width, last - first + 1), target);
        }
    }

    /// Grow the area of the target being drawn by `rect`.
    fn cover(&mut self, rect: Rect) {
        if let Some((_, area)) = &mut self.aim {
            *area = Some(match area {
                Some(area) => area.union(&rect),
                None => rect,
            });
        }
    }

    pub fn spot(&mut self, id: &str, x: u16, y: usize) {
        if let Some(row) = self.row(y) {
            self.spots.insert(id.to_string(), (x, row));
        }
    }
}

/// Draw a Revue view into a rect of the context, given in the context's coordinates, and clipped to `clip` when set.
trait RenderInto {
    fn render_child_into(&self, ctx: &mut RenderContext, rect: Rect, clip: Option<Rect>);
}

impl<V: View> RenderInto for V {
    fn render_child_into(&self, ctx: &mut RenderContext, rect: Rect, clip: Option<Rect>) {
        let area = ctx.area;
        let absolute = Rect::new(area.x + rect.x, area.y + rect.y, rect.width, rect.height);
        let bounds = Rect::new(area.x, area.y, area.width, area.height);
        let clip = clip.map_or(bounds, |clip| {
            Rect::new(area.x + clip.x, area.y + clip.y, clip.width, clip.height)
        });
        ctx.render_child_with_overflow(self, absolute, false, clip.intersection(&bounds));
    }
}

/// OSC 8 links are written only for these schemes (section 2.7).
/// In ASCII mode a mark of one column has a stand-in of one column, so nothing moves: the ellipsis of a cut text, the
/// dashes and dots of a footer, the arrows, bullets and bars. A letter of another script is left as it is.
fn ascii_stand_in(ch: char) -> char {
    match ch {
        '…' => '.',
        '–' | '—' | '─' | '·' => '-',
        '•' | '●' => '*',
        '○' => 'o',
        '▸' | '▶' | '›' => '>',
        '◂' | '‹' => '<',
        '▾' | '▼' => 'v',
        '▲' => '^',
        '▌' | '│' | '┃' => '|',
        '“' | '”' => '"',
        '‘' | '’' => '\'',
        '×' => 'x',
        '→' => '>',
        '✓' => '+',
        '≡' => '=',
        '▣' => '#',
        other => other,
    }
}

pub fn safe_link(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")
}
