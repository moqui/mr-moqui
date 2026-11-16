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

//! Key names, the actions of plans/archive/Screen-TUI-Impl.md 1.5 and the keymaps of the key schemes: Modern (the
//! default: arrows move, Enter follows, Backspace goes back, desktop editor keys) and Classic (5250 function keys and
//! Lynx link travel). The user's `TUI_KEYS` preference, else the theme's `STRT_TUI_KEY_SCHEME`, picks the scheme, and a
//! theme's `STRT_TUI_KEYS` rows merge on top (D11). Esc, Enter and Tab stay bound to what they do whatever a theme says,
//! so no theme can leave a user stuck in a dialog. Single-letter keys act only when focus is not in a text field; the
//! session sends typing to the field before it looks here.

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Enter,
    Escape,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyPress {
    pub key: Key,
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
}

impl KeyPress {
    pub fn plain(key: Key) -> Self {
        Self {
            key,
            alt: false,
            ctrl: false,
            shift: false,
        }
    }

    pub fn with_alt(key: Key) -> Self {
        Self {
            alt: true,
            ..Self::plain(key)
        }
    }

    pub fn with_ctrl(key: Key) -> Self {
        Self {
            ctrl: true,
            ..Self::plain(key)
        }
    }

    /// No modifier, or only Shift on a character (which is in the character itself).
    pub fn is_plain(&self) -> bool {
        !self.alt && !self.ctrl
    }

    /// The chord as the key line and help show it: `F3`, `Alt+Left`, `Ctrl+S`, `Space`, `/`.
    pub fn label(&self) -> String {
        let mut out = String::new();
        if self.ctrl {
            out.push_str("Ctrl+");
        }
        if self.alt {
            out.push_str("Alt+");
        }
        if self.shift && !matches!(self.key, Key::Char(_) | Key::BackTab) {
            out.push_str("Shift+");
        }
        let name = match &self.key {
            Key::Char(' ') => "Space".to_string(),
            Key::Char(ch) if self.ctrl || self.alt => ch.to_ascii_uppercase().to_string(),
            Key::Char(ch) => ch.to_string(),
            Key::Enter => "Enter".to_string(),
            Key::Escape => "Esc".to_string(),
            Key::Tab => "Tab".to_string(),
            Key::BackTab => "Shift+Tab".to_string(),
            Key::Backspace => "Backspace".to_string(),
            Key::Delete => "Del".to_string(),
            Key::Up => "Up".to_string(),
            Key::Down => "Down".to_string(),
            Key::Left => "Left".to_string(),
            Key::Right => "Right".to_string(),
            Key::Home => "Home".to_string(),
            Key::End => "End".to_string(),
            Key::PageUp => "PgUp".to_string(),
            Key::PageDown => "PgDn".to_string(),
            Key::F(n) => format!("F{n}"),
        };
        out.push_str(&name);
        out
    }
}

/// A set of default keys, and the few behaviors outside the keymap that go with it (what Right and Esc do). A theme's
/// `STRT_TUI_KEYS` rows apply over either; a row named `modern.<action>` or `classic.<action>` applies to that scheme only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum KeyScheme {
    /// Today's TUI apps and desktop editors: Left and Right move as Up and Down do, Enter follows, Backspace goes back,
    /// Esc opens the palette when nothing is open, Ctrl+A selects all.
    #[default]
    Modern,
    /// The 5250 green screen and Lynx: F3 back, F12 cancel, Right follows a link and Left goes back.
    Classic,
}

/// Every scheme, in the order a choice of them lists them.
pub const KEY_SCHEMES: [KeyScheme; 2] = [KeyScheme::Modern, KeyScheme::Classic];

impl KeyScheme {
    /// The `TUI_KEYS` value, the `--keys` value and the theme row prefix.
    pub fn name(self) -> &'static str {
        match self {
            Self::Modern => "modern",
            Self::Classic => "classic",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Modern => "Modern",
            Self::Classic => "Classic (green screen)",
        }
    }

    /// A name, in any case, with spaces around it allowed.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim();
        KEY_SCHEMES
            .into_iter()
            .find(|scheme| scheme.name().eq_ignore_ascii_case(name))
    }

    /// The scheme a palette row switches to from this one.
    pub fn other(self) -> Self {
        match self {
            Self::Modern => Self::Classic,
            Self::Classic => Self::Modern,
        }
    }
}

/// What a key does. The theme names are in [`Action::name`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Next,
    Prev,
    Activate,
    Back,
    Cancel,
    Help,
    Prompt,
    Refresh,
    PagePrev,
    PageNext,
    Drawer,
    Submit,
    Select,
    SelectAll,
    Find,
    Sort,
    /// A list's next view: list, board, calendar.
    View,
    NumberLinks,
    Info,
    OpenBrowser,
    Redraw,
    /// The log pane of a local session.
    Log,
    Quit,
    /// Subscreen tab 1 to 9.
    Tab(u8),
    /// The palette: screens, records, saved finds, actions, recent screens, apps.
    Palette,
    /// Go to a path or a menu title.
    GoTo,
    QuickCreate,
    /// One menu of the screen's button menus, the focused list's row actions and its tools.
    Actions,
    /// Find text on the screen, then the next or previous match.
    FindText,
    FindNext,
    FindPrev,
    /// The last messages.
    Messages,
    /// Today, Chat and Notifications.
    Tray,
    /// Edit the focused row of a list.
    EditRow,
    /// The mouse on or off: off gives the terminal its own selection and menu back.
    Mouse,
    /// The Assist pane: show and focus it, or close it.
    Assist,
    /// Stop Assist's run.
    AssistStop,
    /// The first and last: a list's first and last row, else the first and last thing to focus.
    First,
    Last,
    /// The next and previous subscreen tab.
    TabNext,
    TabPrev,
    /// Copy the focused item's text to the clipboard; in a text field, the selection.
    Copy,
}

/// Every action, in the order the help lists them.
pub const ACTIONS: [Action; 41] = [
    Action::Help,
    Action::Next,
    Action::Prev,
    Action::First,
    Action::Last,
    Action::Activate,
    Action::Submit,
    Action::Back,
    Action::Cancel,
    Action::Prompt,
    Action::Refresh,
    Action::PagePrev,
    Action::PageNext,
    Action::TabPrev,
    Action::TabNext,
    Action::Drawer,
    Action::Find,
    Action::Sort,
    Action::View,
    Action::Select,
    Action::SelectAll,
    Action::NumberLinks,
    Action::Info,
    Action::OpenBrowser,
    Action::Redraw,
    Action::Log,
    Action::Palette,
    Action::GoTo,
    Action::QuickCreate,
    Action::Actions,
    Action::FindText,
    Action::FindNext,
    Action::FindPrev,
    Action::Messages,
    Action::Tray,
    Action::EditRow,
    Action::Copy,
    Action::Mouse,
    Action::Assist,
    Action::AssistStop,
    Action::Quit,
];

impl Action {
    /// The name a theme's `STRT_TUI_KEYS` row uses: `back = f3 | alt+left`.
    pub fn name(self) -> String {
        match self {
            Self::Next => "next".into(),
            Self::Prev => "prev".into(),
            Self::Activate => "activate".into(),
            Self::Back => "back".into(),
            Self::Cancel => "cancel".into(),
            Self::Help => "help".into(),
            Self::Prompt => "prompt".into(),
            Self::Refresh => "refresh".into(),
            Self::PagePrev => "page-prev".into(),
            Self::PageNext => "page-next".into(),
            Self::Drawer => "drawer".into(),
            Self::Submit => "submit".into(),
            Self::Select => "select".into(),
            Self::SelectAll => "select-all".into(),
            Self::Find => "find".into(),
            Self::Sort => "sort".into(),
            Self::View => "view".into(),
            Self::NumberLinks => "number-links".into(),
            Self::Info => "info".into(),
            Self::OpenBrowser => "open-browser".into(),
            Self::Redraw => "redraw".into(),
            Self::Log => "log".into(),
            Self::Quit => "quit".into(),
            Self::Tab(n) => format!("tab-{n}"),
            Self::Palette => "palette".into(),
            Self::GoTo => "go-to".into(),
            Self::QuickCreate => "quick-create".into(),
            Self::Actions => "actions".into(),
            Self::FindText => "find-text".into(),
            Self::FindNext => "find-next".into(),
            Self::FindPrev => "find-prev".into(),
            Self::Messages => "messages".into(),
            Self::Tray => "tray".into(),
            Self::EditRow => "edit-row".into(),
            Self::Mouse => "mouse".into(),
            Self::Assist => "assist".into(),
            Self::AssistStop => "assist-stop".into(),
            Self::First => "first".into(),
            Self::Last => "last".into(),
            Self::TabNext => "tab-next".into(),
            Self::TabPrev => "tab-prev".into(),
            Self::Copy => "copy".into(),
        }
    }

    /// The word on the key line and in the help.
    pub fn title(self) -> &'static str {
        match self {
            Self::Next => "Next",
            Self::Prev => "Previous",
            Self::Activate => "Follow",
            Self::Back => "Back",
            Self::Cancel => "Cancel",
            Self::Help => "Help",
            Self::Prompt => "Prompt",
            Self::Refresh => "Refresh",
            Self::PagePrev => "Page up",
            Self::PageNext => "Page down",
            Self::Drawer => "Menu",
            Self::Submit => "Save",
            Self::Select => "Select",
            Self::SelectAll => "Select all",
            Self::Find => "Find",
            Self::Sort => "Sort",
            Self::View => "View",
            Self::NumberLinks => "Number links",
            Self::Info => "Info",
            Self::OpenBrowser => "Open in browser",
            Self::Redraw => "Redraw",
            Self::Log => "Log",
            Self::Quit => "Quit",
            Self::Tab(_) => "Tab",
            Self::Palette => "Go to",
            Self::GoTo => "Path",
            Self::QuickCreate => "Create",
            Self::Actions => "Actions",
            Self::FindText => "Find text",
            Self::FindNext => "Next match",
            Self::FindPrev => "Previous match",
            Self::Messages => "Messages",
            Self::Tray => "Tray",
            Self::EditRow => "Edit row",
            Self::Mouse => "Mouse",
            Self::Assist => "Assist",
            Self::AssistStop => "Stop Assist",
            Self::First => "First",
            Self::Last => "Last",
            Self::TabNext => "Next tab",
            Self::TabPrev => "Previous tab",
            Self::Copy => "Copy",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        if let Some(n) = name.strip_prefix("tab-") {
            return n.parse::<u8>().ok().filter(|n| (1..=9).contains(n)).map(Self::Tab);
        }
        ACTIONS.into_iter().find(|action| action.name() == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyMap {
    bindings: Vec<(KeyPress, Action)>,
}

impl KeyMap {
    /// The keys of a scheme.
    pub fn for_scheme(scheme: KeyScheme) -> Self {
        match scheme {
            KeyScheme::Modern => Self::modern(),
            KeyScheme::Classic => Self::classic(),
        }
    }

    /// The scheme of section 1.5: 5250 function keys and Lynx link travel.
    pub fn classic() -> Self {
        let plain = KeyPress::plain;
        let ch = |c| KeyPress::plain(Key::Char(c));
        let mut bindings = vec![
            (plain(Key::Tab), Action::Next),
            (plain(Key::Down), Action::Next),
            (plain(Key::BackTab), Action::Prev),
            (plain(Key::Up), Action::Prev),
            (plain(Key::Enter), Action::Activate),
            (plain(Key::Right), Action::Activate),
            (plain(Key::F(3)), Action::Back),
            (KeyPress::with_alt(Key::Left), Action::Back),
            (plain(Key::Left), Action::Back),
            (plain(Key::Backspace), Action::Back),
            (plain(Key::Escape), Action::Cancel),
            (plain(Key::F(12)), Action::Cancel),
            (plain(Key::F(1)), Action::Help),
            (ch('?'), Action::Help),
            (plain(Key::F(4)), Action::Prompt),
            (plain(Key::F(5)), Action::Refresh),
            (KeyPress::with_ctrl(Key::Char('r')), Action::Refresh),
            (plain(Key::F(7)), Action::PagePrev),
            (plain(Key::PageUp), Action::PagePrev),
            (plain(Key::F(8)), Action::PageNext),
            (plain(Key::PageDown), Action::PageNext),
            (plain(Key::F(9)), Action::Drawer),
            (KeyPress::with_alt(Key::Enter), Action::Submit),
            (KeyPress::with_ctrl(Key::Char('s')), Action::Submit),
            (ch(' '), Action::Select),
            (ch('*'), Action::SelectAll),
            (ch('f'), Action::Find),
            (ch('s'), Action::Sort),
            (ch('v'), Action::View),
            (ch('#'), Action::NumberLinks),
            (ch('='), Action::Info),
            (ch('o'), Action::OpenBrowser),
            (KeyPress::with_ctrl(Key::Char('l')), Action::Redraw),
            (KeyPress::with_alt(Key::Char('l')), Action::Log),
            (KeyPress::with_alt(Key::Char('p')), Action::Mouse),
            (KeyPress::with_ctrl(Key::Char('q')), Action::Quit),
            (KeyPress::with_ctrl(Key::Char('k')), Action::Palette),
            (ch(':'), Action::Palette),
            (ch('g'), Action::GoTo),
            (plain(Key::F(6)), Action::QuickCreate),
            (ch('+'), Action::QuickCreate),
            (plain(Key::F(10)), Action::Actions),
            (ch('/'), Action::FindText),
            (ch('n'), Action::FindNext),
            (ch('N'), Action::FindPrev),
            (KeyPress::with_alt(Key::Char('m')), Action::Messages),
            (KeyPress::with_alt(Key::Char('t')), Action::Tray),
            (ch('e'), Action::EditRow),
            (plain(Key::F(2)), Action::Assist),
            (KeyPress::with_alt(Key::Char('a')), Action::Assist),
            (KeyPress::with_alt(Key::Char('x')), Action::AssistStop),
        ];
        bindings.extend(shared());
        Self { bindings }
    }

    /// Today's TUI apps and desktop editors, with the vi keys that clash with nothing: arrows and `j`/`k` move, Enter
    /// follows, Backspace goes back, Ctrl+F finds and F3 finds the next, Ctrl+K or Ctrl+P the palette. The first chord
    /// of an action is the one the key line shows.
    pub fn modern() -> Self {
        let plain = KeyPress::plain;
        let ch = |c| KeyPress::plain(Key::Char(c));
        let ctrl = |c| KeyPress::with_ctrl(Key::Char(c));
        let mut bindings = vec![
            (plain(Key::Tab), Action::Next),
            (plain(Key::Down), Action::Next),
            (plain(Key::Right), Action::Next),
            (ch('j'), Action::Next),
            (plain(Key::BackTab), Action::Prev),
            (plain(Key::Up), Action::Prev),
            (plain(Key::Left), Action::Prev),
            (ch('k'), Action::Prev),
            (plain(Key::Enter), Action::Activate),
            (plain(Key::Backspace), Action::Back),
            (KeyPress::with_alt(Key::Left), Action::Back),
            (plain(Key::Escape), Action::Cancel),
            (plain(Key::F(1)), Action::Help),
            (ch('?'), Action::Help),
            (plain(Key::F(4)), Action::Prompt),
            (KeyPress::with_alt(Key::Down), Action::Prompt),
            (plain(Key::F(5)), Action::Refresh),
            (ctrl('r'), Action::Refresh),
            (plain(Key::PageUp), Action::PagePrev),
            (plain(Key::PageDown), Action::PageNext),
            (plain(Key::F(9)), Action::Drawer),
            (ctrl('s'), Action::Submit),
            (KeyPress::with_alt(Key::Enter), Action::Submit),
            (ch(' '), Action::Select),
            (ctrl('a'), Action::SelectAll),
            (ch('*'), Action::SelectAll),
            (ch('f'), Action::Find),
            (ch('s'), Action::Sort),
            (ch('v'), Action::View),
            (ch('#'), Action::NumberLinks),
            (ch('='), Action::Info),
            (ch('o'), Action::OpenBrowser),
            (ctrl('l'), Action::Redraw),
            (KeyPress::with_alt(Key::Char('l')), Action::Log),
            (KeyPress::with_alt(Key::Char('p')), Action::Mouse),
            (ctrl('q'), Action::Quit),
            (ctrl('k'), Action::Palette),
            (ctrl('p'), Action::Palette),
            (ch(':'), Action::Palette),
            (ch('g'), Action::GoTo),
            (ctrl('g'), Action::GoTo),
            (ch('+'), Action::QuickCreate),
            (plain(Key::F(10)), Action::Actions),
            (ctrl('f'), Action::FindText),
            (ch('/'), Action::FindText),
            (plain(Key::F(3)), Action::FindNext),
            (ch('n'), Action::FindNext),
            (
                KeyPress {
                    shift: true,
                    ..plain(Key::F(3))
                },
                Action::FindPrev,
            ),
            (ch('N'), Action::FindPrev),
            (KeyPress::with_alt(Key::Char('m')), Action::Messages),
            (KeyPress::with_alt(Key::Char('t')), Action::Tray),
            (ch('e'), Action::EditRow),
            (plain(Key::F(2)), Action::Assist),
            (KeyPress::with_alt(Key::Char('a')), Action::Assist),
            (KeyPress::with_alt(Key::Char('x')), Action::AssistStop),
        ];
        bindings.extend(shared());
        Self { bindings }
    }

    /// The action of a key. A character matches without Shift, which is in the character (`N`, `?`).
    pub fn action(&self, press: &KeyPress) -> Option<Action> {
        let press = unshifted(press);
        self.bindings
            .iter()
            .find(|(bound, _)| unshifted(bound) == press)
            .map(|(_, action)| *action)
    }

    /// The chords of `action`, in the order they were bound.
    pub fn chords(&self, action: Action) -> Vec<&KeyPress> {
        self.bindings
            .iter()
            .filter(|(_, bound)| *bound == action)
            .map(|(press, _)| press)
            .collect()
    }

    /// `F3=Back`: the first chord of `action`, or nothing when it has none.
    pub fn hint(&self, action: Action) -> Option<String> {
        self.chords(action)
            .first()
            .map(|press| format!("{}={}", press.label(), action.title()))
    }

    /// Unbind `action` except on the fixed keys.
    fn retain_action(&mut self, action: Action) {
        self.bindings
            .retain(|(press, bound)| *bound != action || fixed_key(press));
    }

    fn bind(&mut self, press: KeyPress, action: Action) {
        self.bindings.retain(|(bound, _)| bound != &press);
        self.bindings.push((press, action));
    }
}

/// The keys both schemes bind the same way: Home and End, the tabs, Copy.
fn shared() -> Vec<(KeyPress, Action)> {
    let mut bindings = vec![
        (KeyPress::plain(Key::Home), Action::First),
        (KeyPress::plain(Key::End), Action::Last),
        (KeyPress::with_ctrl(Key::PageDown), Action::TabNext),
        (KeyPress::with_ctrl(Key::PageUp), Action::TabPrev),
        (KeyPress::with_ctrl(Key::Char('c')), Action::Copy),
    ];
    for n in 1..=9u8 {
        let digit = char::from(b'0' + n);
        bindings.push((KeyPress::with_alt(Key::Char(digit)), Action::Tab(n)));
        bindings.push((KeyPress::plain(Key::Char(digit)), Action::Tab(n)));
    }
    bindings
}

/// The keys of `scheme` with a theme's `STRT_TUI_KEYS` rows over them. A row named `<scheme>.<action>` applies to that
/// scheme only, after the rows without a scheme, so it wins over them; a row for another scheme is left out.
pub fn scheme_keys(scheme: KeyScheme, overrides: &[(&str, &str)]) -> (KeyMap, Vec<MergeSkip>) {
    let mut general = Vec::new();
    let mut own = Vec::new();
    for (name, chords) in overrides {
        match name.trim().split_once('.') {
            Some((prefix, action)) => match KeyScheme::parse(prefix) {
                Some(named) if named == scheme => own.push((action, *chords)),
                Some(_) => {}
                None => general.push((*name, *chords)),
            },
            None => general.push((*name, *chords)),
        }
    }
    general.extend(own);
    merge_keys(&KeyMap::for_scheme(scheme), &general)
}

/// Why one theme entry was not applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeSkip {
    pub action: String,
    pub chord: String,
    pub reason: &'static str,
}

impl std::fmt::Display for MergeSkip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "key binding {} = {} skipped: {}",
            self.action, self.chord, self.reason
        )
    }
}

/// Theme rows replace the default chords of a known action. A bad chord or an unknown action is skipped. A chord of Esc,
/// Enter or Tab is refused, and that action keeps its default; an action bound to one of those keys keeps it.
pub fn merge_keys(base: &KeyMap, overrides: &[(&str, &str)]) -> (KeyMap, Vec<MergeSkip>) {
    let mut map = base.clone();
    let mut skipped = Vec::new();
    for (action_name, chords) in overrides {
        let skip = |chord: &str, reason| MergeSkip {
            action: (*action_name).to_string(),
            chord: chord.to_string(),
            reason,
        };
        let Some(action) = Action::parse(action_name.trim()) else {
            skipped.push(skip(chords, "unknown action"));
            continue;
        };
        let mut parsed = Vec::new();
        let mut bad = false;
        for chord in chords.split('|').map(str::trim).filter(|chord| !chord.is_empty()) {
            match parse_chord(chord) {
                Some(press) if fixed_key(&press) => {
                    skipped.push(skip(chord, "fixed key"));
                    bad = true;
                }
                Some(press) => parsed.push(press),
                None => {
                    skipped.push(skip(chord, "bad chord"));
                    bad = true;
                }
            }
        }
        if bad || parsed.is_empty() {
            continue;
        }
        map.retain_action(action);
        for press in parsed {
            map.bind(press, action);
        }
    }
    (map, skipped)
}

fn unshifted(press: &KeyPress) -> KeyPress {
    let mut press = press.clone();
    if matches!(press.key, Key::Char(_)) {
        press.shift = false;
    }
    press
}

fn fixed_key(press: &KeyPress) -> bool {
    matches!(press.key, Key::Escape | Key::Enter | Key::Tab | Key::BackTab) && !press.alt && !press.ctrl
}

impl KeyPress {
    /// A chord as the theme's key bindings and the agent mode name it: `F10`, `Enter`, `Ctrl+K`, `Alt+M`, `Shift+Tab`,
    /// a single character. Case does not matter in the names.
    pub fn parse(chord: &str) -> Option<Self> {
        parse_chord(chord)
    }
}

fn parse_chord(chord: &str) -> Option<KeyPress> {
    let mut alt = false;
    let mut ctrl = false;
    let mut shift = false;
    let mut key = None;
    // `alt++` is Alt with the plus key; split on the `+` between names only.
    let mut parts: Vec<&str> = chord.split('+').collect();
    if chord.ends_with("++") {
        parts.truncate(parts.len().saturating_sub(2));
        parts.push("+");
    }
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        match part.to_ascii_lowercase().as_str() {
            "alt" => alt = true,
            "ctrl" => ctrl = true,
            "shift" => shift = true,
            other => {
                if key.is_some() {
                    return None;
                }
                key = Some(parse_key(other)?);
            }
        }
    }
    Some(KeyPress {
        key: key?,
        alt,
        ctrl,
        shift,
    })
}

fn parse_key(name: &str) -> Option<Key> {
    if name.chars().count() == 1 {
        return name.chars().next().map(Key::Char);
    }
    Some(match name {
        "esc" | "escape" => Key::Escape,
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "backtab" => Key::BackTab,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "space" => Key::Char(' '),
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pgup" | "pageup" => Key::PageUp,
        "pgdn" | "pagedown" => Key::PageDown,
        other => {
            let number = other.strip_prefix('f')?.parse::<u8>().ok()?;
            if (1..=12).contains(&number) {
                Key::F(number)
            } else {
                return None;
            }
        }
    })
}

/// What a click on a part of the frame acts on (plans/archive/Screen-TUI-Impl.md Phase 5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A focusable by its focus key: a link, a button, a field (its label and its value), a dialog or menu button, a
    /// box title that opens and closes, a component.
    Focus(String),
    /// A row of a list, and its selection box.
    ListRow {
        list: String,
        row: usize,
    },
    ListSelect {
        list: String,
        row: usize,
    },
    /// A column title: its sort. The index is the list's column.
    ListSort {
        list: String,
        column: usize,
    },
    /// A link in a list row's cell: the row becomes the current one, and the link is followed; an empty link is the
    /// row's own (its first column).
    RowLink {
        list: String,
        row: usize,
        link: String,
    },
    /// A subscreen tab: its url.
    Tab(String),
    TreeRow {
        tree: String,
        row: usize,
    },
    Drawer(usize),
    /// The `≡` of a hidden drawer.
    DrawerToggle,
    /// The crumbs of the title line: the palette.
    Title,
    /// An entry of the key line: its action.
    KeyLine(Action),
    /// The choices of an open overlay, by their index there.
    Choice(usize),
    MenuItem(usize),
    PaletteRow(usize),
    ConfirmChoice(usize),
    LoginField(usize),
    PickerDay(jiff::civil::Date),
    /// An overlay's area: a click there that is on nothing else does nothing, and never reaches the screen under it.
    Block,
}

/// A mouse report, as every channel gives it (plans/archive/Screen-TUI-Impl.md Phase 5): SGR or X10 bytes, or Revue's event in
/// local mode. Columns and rows count from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mouse {
    pub kind: MouseKind,
    /// 0 left, 1 middle, 2 right.
    pub button: u8,
    pub column: u16,
    pub row: u16,
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    /// Motion with a button held.
    Drag,
    Release,
    WheelUp,
    WheelDown,
}

impl Mouse {
    pub fn at(kind: MouseKind, column: u16, row: u16) -> Self {
        Self {
            kind,
            button: 0,
            column,
            row,
            alt: false,
            ctrl: false,
            shift: false,
        }
    }
}
