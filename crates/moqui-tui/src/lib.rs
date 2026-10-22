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

//! The terminal client of the `tui` render mode (plans/archive/Screen-TUI-Impl.md section 3).
//!
//! It does no I/O of its own (D4): the session queues calls, and the host's [`ScreenLink`] answers them. The `model`
//! feature is everything but the Revue view: the model, the link, navigation, state, layout, keys and theme, which
//! `moqui-core`'s tests use without compiling Revue (D3). The `ui` feature adds the Revue view and the local loop.

mod assist;
mod components;
mod decode;
mod edit;
mod forms;
mod keys;
mod layout;
mod link;
mod markdown;
mod model;
mod nav;
mod session;
mod state;
mod theme;
#[cfg(feature = "ui")]
mod ui;

pub use assist::{state as assist_state, Act as AssistAct, Area as AssistArea, Assist, PANE_WIDTH};
pub use components::{
    find as find_component, line_text, names as component_names, Act, Component, ComponentState, Line, Registered, Span,
};
pub use decode::{Decoded, Decoder};
pub use edit::{row_col, Edited, Limits};
pub use forms::{
    field_keys, parameter_keys, ComponentInfo, DynamicInfo, Extra, FieldInfo, FocusItem, FocusKind, FormInfo, Index,
    LinkInfo, MenuInfo, RowInfo, Scope, TabInfo, TreeInfo, WizardInfo,
};
pub use keys::{merge_keys, Action, Key, KeyMap, KeyPress, MergeSkip, Mouse, MouseKind, Target, ACTIONS};
pub use layout::{
    display_width, dotted_label, frame, frame_with, label_column_width, layout, layout_levels, layout_nodes, outline,
    outline_of, ref_id, truncate_width, width_class, wizard_back, wizard_next, wrap, Breakpoints, Frame, LabelMode,
    LaidOut, Piece, Plain, Shown, TreeMark, TreeRow, WidthClass, DRAWER_DOCK_AT, DRAWER_WIDTH, PAGE_ROWS,
};
pub use link::{
    counts_of, gallery_link, row_link, CodeAsk, FakeLink, LinkCall, LinkError, MenuItem, Push, PushSink, RawBody,
    Reply, ScreenLink, Sendable, ShellAction, ShellInfo, Subscription, TransitionOutcome, GALLERY_DETAIL_JSON,
    GALLERY_DETAIL_PATH, GALLERY_LIST_JSON, GALLERY_LIST_PATH,
};
pub use markdown::blocks as markdown_blocks;
pub use model::{
    clean_text, find_mut, node_of, prefix_ids, walk, walk_mut, Cell, Choice, Column, ColumnChoice, Export, FormList,
    HiddenValue, HtmlBlock, HtmlLink, ListRow, ListViews, Messages, ModelError, Node, Paging, RowSelection, Rule,
    SavedFind, SavedFinds, ScreenMeta, ScreenModel, SelectColumns, StatusStage, SubscreenItem, ValidationError,
    VisibleWhen, Widget, MODEL_VERSION,
};
pub use nav::{decode as decode_query, level_path, query_parameters, Location, Nav, LEVEL_FLOOR};
pub use session::{
    drawer_rows, ColumnEntry, Columns, DropDown, Editor, Login, Menu, MenuAct, MenuEntry, Message, Overlay, Palette,
    PaletteRow, Picker, Prompt, PromptThen, Session, Timer, TreeNode, TreeState,
};
pub use state::{AssistApproval, Confirm, ConfirmKind, Edits, Finished, Lanes, Purpose, Request, Then};
pub use theme::{Depth, Glyph, Role, Theme};
#[cfg(feature = "ui")]
pub use ui::{
    from_revue, render_text, run_local, run_session, snapshot, Agent, AgentAnswer, AgentSnapshot, AppView,
    ChangedField, Hit, Hits, LocalOptions, Screen, Selection, SessionEvent, SessionOptions, AGENT_HELP, ENTER, LEAVE,
    MOUSE_OFF, MOUSE_ON,
};
