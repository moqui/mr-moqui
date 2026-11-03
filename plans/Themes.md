# Themes

Built Oct 27–Nov 2, 2026 (commits c44d5f85, a5a6c555, 06d5d5c9 and 578f119a): the theme's default color scheme and the System app's Theme screen for corporate branding without code; light and dark accent colors, per-color scheme overrides and five new color schemes; the scheme's colors inlined from the page config, so switching a scheme applies at once; the theme's user appearance policy, so a theme decides what a user may override and the scheme's primary is the theme's by default; and OpenStep, the NeXTSTEP/OpenSTEP look. The user guide is [docs/ui/Qvt-Client.md](../docs/ui/Qvt-Client.md) (Appearance); what is still open is in [Remaining-Work](Remaining-Work.md#screens-client-and-shell).

## Objective and scope

Two audiences for the look of the `/qapps` Quasar shell:

- **The administrator** sets the theme's branding without code: the shell's resources (the logo, the title, the footer items) and the theme's default color scheme, plus the color schemes and the fonts a user may choose.
- **The user** personalizes from the account menu's **Appearance…**: light, dark or auto, the color scheme, the fonts, the text sizes, and the motion, corners and readability options; the accent color too, when the theme allows it.

The framework stays free of any data model: a theme is configuration (`ScreenTheme` rows and their resources), a color scheme is a set of color tokens, and a user's choices are preferences. The scope is the internal Quasar shell (`STT_INTERNAL_QUASAR`); the public and error themes and the terminal client are outside it (Later).

## 1. Theme resolution

[`screen/sri/theme.rs`](../crates/moqui-core/src/screen/sri/theme.rs) finds the theme of a render, once, in this order:

1. the user's `UserScreenTheme` for the screen theme type
2. one of the user's groups' `UserGroupScreenTheme`, highest sequence first, `ALL_USERS` last
3. the type's `enumCode` as a theme id
4. a theme of the type with `DEFAULT` in its id

The shell's type is `STT_INTERNAL_QUASAR` (`QUASAR_THEME_TYPE`), falling back to `DEFAULT_QUASAR`; `user_theme_id` answers the same question outside a render for a service that reads the shell's theme values (the counts service of `ShellServices`). The System app's Theme screen resolves the current user's Quasar theme the same way in its actions, so the branding is kept on the theme the shell renders.

## 2. Data model

All in [`ScreenEntities.xml`](../runtime/base-component/framework/entity/ScreenEntities.xml), `use="configuration"`, cached:

| Entity | What it holds |
|---|---|
| `ScreenTheme` | A theme: `screenThemeId`, its type (`STT_INTERNAL`, `STT_PUBLIC`, `STT_ERROR`; the webroot seeds `STT_INTERNAL_QUASAR`), description |
| `ScreenThemeResource` | The theme's resources, one row per `STRT_*` type and value: the style sheets and scripts, the header logo, title and navbar components, the footer items, the Quasar config JSON (`STRT_QUASAR_CONFIG`), the shell's tray, quick-create, mail, chatter and record-files screens and counts service, the TUI widths, colors and keys, the **`STRT_DEFAULT_COLOR_SCHEME`** (a `ScreenColorScheme` id of the theme) and the **`STRT_APPEARANCE_POLICY`** (the user appearance policy; [Appearance policy](#appearance-policy)) |
| `ScreenThemeIcon` | An icon class per text pattern |
| `ScreenColorScheme` | A scheme a user may choose: `colorSchemeId`, `screenThemeId`, description, sequence, and `extraCss` — CSS written after the colors for what is not a color; written by an administrator, never from what a user enters |
| `ScreenColorSchemeColor` | One color of a scheme: `colorModeEnumId` (`SCM_LIGHT`, `SCM_DARK`), `colorName` (a token name without its prefix), `colorValue` (a CSS color: `#hex`, `rgb()`, `hsl()`, `oklch()`, `color-mix()` or a `var()` of another token) |
| `ScreenFontOption` | A font a user may choose: `fontKindEnumId` (`SFK_SANS`, `SFK_SERIF`, `SFK_MONO`), a `font-family` stack with a generic family last, and an optional style sheet (a `/libs` path or an https URL; empty for a font the system has) |

The webroot seeds [`WebrootQuasarThemeData.xml`](../runtime/base-component/webroot/data/WebrootQuasarThemeData.xml) for `DEFAULT_QUASAR`:

- **Fifteen color schemes.** `DEFAULT` has no colors, so it is the theme's own (the style sheet's tokens and the `STRT_QUASAR_CONFIG` brand). The others set the standard colors for light and dark: Slate, Forest, Warm, High Contrast (black and white surfaces, darker brand steps, stronger borders and a wide focus ring in its extra CSS), Ocean, Indigo, Crimson, Teal, Graphite (a neutral, brand-free grey for a system that carries its own brand elsewhere), OpenStep (a NeXTSTEP/OpenStep cool blue-grey desktop, white document panels, charcoal title bar, a grey primary, white selected rows and active menu rows with black text, square beveled buttons and dark text on grey buttons; the main header bar stays flat except Search, which keeps its grey box and black text; its dark mode is a charcoal variant, since NeXTSTEP had none) Windows 11 (the Fluent look: the Mica grey desktop, white surfaces, the accent blue, 8px-rounded cards and 4px-rounded controls, flat grey buttons with a thin border, the subtle card depth, the accent tint on the selected rows and the focus ring, and Segoe UI as the default font; its dark mode is the dark Mica) and Plasma (the KDE Plasma/Breeze look: the light grey desktop, white surfaces, the Breeze blue, 3px-rounded cards and 2px-rounded controls, flat buttons with a thin border, the solid accent selection with white text, and Noto Sans as the default font; its dark mode is the Breeze dark). Rainbow is the pride flag, unapologetic: the flag across the header, the primary buttons and the drawer's current app, a pastel rainbow wash on the page, rainbow-tinted selection and card edges, purple focus; its dark mode is the flag over charcoal with brighter flag steps. Lab is the Moqui Labs look (moquilabs.com): the near-black surfaces with the hot pink and cyan neon, the dark header with its pink-to-cyan underline and glow, the neon wash on the page, the glowing primary buttons, the pink selection with the cyan focus, and Orbitron headings with Barlow body text; the light mode keeps the brand's dark header. The corporate schemes (Slate, Forest, Warm, Ocean, Indigo, Crimson, Teal) also carry extra CSS: the brand-gradient header (dark with white text in light mode; the dev and test deployments mark the window with a corner square, the `webapp_dev_marker_color` conf property, never the header's color), a page wash toward the brand, the gradient primary buttons (primary into the secondary or the accent), and the complementary accent on the drawer's current app (Crimson's is the teal secondary).
- **Thirteen fonts.** Five sans (System, Roboto, Inter, Source Sans 3, Atkinson Hyperlegible), three serif (System, Merriweather, Source Serif 4) and three mono (System, JetBrains Mono, IBM Plex Mono); the web fonts load from Google Fonts, as the theme's Roboto does. Orbitron and Barlow (the Lab scheme's heading and body fonts) are two more sans.

**The tokens** are the names a scheme and a user's overrides address: the tokens of [`WebrootVue.qvt.css`](../runtime/base-component/webroot/screen/webroot/css/WebrootVue.qvt.css) without their prefix.

- the seven brand colors `primary`, `secondary`, `accent`, `positive`, `negative`, `info`, `warning` (Quasar's `--q-*`)
- `page-bg`, `panel-bg`, `header-bg`, `header-fg`, `drawer-bg`, `text`, `text-muted`, `link`, `negative-text`, `border`, `divider` (`--m-*`)

## 3. Tokens and cascade

The style sheet starts with the tokens: the fonts, text sizes and line height on `:root` (a user's preference sets them on the `html` element), the colors on the `body` per mode (`body:not(.body--dark)` and `body.body--dark`). The later sheet wins, in this order:

1. the Quasar brand the `STRT_QUASAR_CONFIG` JSON writes on the body
2. the theme's own defaults (the tokens above)
3. the scheme's sheet: a brand color as `--q-*` with `!important` (over the Quasar brand), the rest as `--m-*`; in dark mode `panel-bg` and `page-bg` also set Quasar's `--q-dark` and `--q-dark-page`, which Quasar's dark components use, and the link gives way to the user's dark accent when the theme allows it (`var(--m-accent-dark-link, …)`)
4. the user's accent, when the theme allows it, and the user's overrides, in a sheet after the scheme's style, so the user's colors win over the scheme's

A color name or value that could end its declaration (a name pattern, a value character set) is logged and left out by the transition, and left out by the page pre-action too. The other borders and tints the style sheet keeps as black or white at a low alpha read on any surface and are not tokens.

## 4. Page load

- `org.moqui.impl.ShellServices.get#Appearance` (Rust, `crates/moqui-core/src/screen/sri/appearance.rs`; called by [`includes/QvtConfig.js`](../runtime/base-component/webroot/screen/includes/QvtConfig.js), which builds the page config for `qapps.xml` and the desktop client) fills `config.appearance`: the theme's color schemes (each with its light and dark colors, its default fonts and a `version` hash of its sheet), the fonts, the user's `THEME_*` values (a locked one left out), the theme's `defaultSchemeId`, the `schemeId` shown, the style sheets the page links and the normalized appearance `policy`. The theme's `STRT_APPEARANCE_SERVICE` services may change any of it first (section 5, Appearance policy).
- [`WebrootVue.qvt.html.fancy`](../runtime/base-component/webroot/screen/includes/WebrootVue.qvt.html.fancy) links the chosen scheme's and fonts' style sheets (`appearance.links`; a scheme's own font when the user has none) after the theme's, before the client starts, so the first paint is in the right colors.
- `moqui.applyAppearanceAll` sets the rest when the shell is created; each preference key has its applier in `moqui.preferenceAppliers`.
- Switching a scheme links its sheet (`/apps/colorSchemeCss?colorSchemeId=…&v=…`, served by `get#ColorSchemeCss`; read-only, no session token, a scheme is not private); the `version` hash busts the cache when an administrator changes a scheme. The user's accent and overrides are styles with `html body` selectors, so they win over the scheme's whatever the order of the sheets.

## 5. User preferences (Appearance)

The account menu's **Appearance…** opens `m-appearance-dialog`. Each choice applies at once (`moqui.applyPreference`) and is saved as the user's preference through `/apps/setPreference`; **Reset to defaults** clears the `THEME_*` keys. Browser zoom still scales the whole page; the sizes here are for the small and large text alone.

| Preference | Values | Applied as |
|---|---|---|
| `QUASAR_DARK` | `true`, `false`, `auto` | `$q.dark.set` |
| `THEME_COLOR_SCHEME` | a `ScreenColorScheme` id of the theme | the `/apps/colorSchemeCss?colorSchemeId=…&v=…` sheet linked as `m-color-scheme`; none for a scheme without colors (the theme's own). Without the user's choice the theme's default scheme (`STRT_DEFAULT_COLOR_SCHEME`) is the effective one |
| `THEME_FONT_BODY`, `THEME_FONT_HEADING`, `THEME_FONT_MONO` | a `ScreenFontOption` id; without one the scheme's own font of the role (`defaultFontBodyId`, `defaultFontHeadingId`, `defaultFontMonoId`) | `--m-font-body`, `--m-font-heading`, `--m-font-mono` on the html element, and the font's style sheet linked as `m-font-<role>` |
| `THEME_SIZE_SMALL`, `THEME_SIZE_LARGE` | `0.9`, `1`, `1.1`, `1.25`, `1.4` | `--m-fs-small-scale` (table text, captions, chips, code) and `--m-fs-large-scale` (headings) |
| `THEME_PRIMARY_COLOR`, `THEME_PRIMARY_COLOR_DARK` | a CSS color each (checked by `moqui.isCssColor`, else ignored), one for light and one for dark (the light one in both modes while the dark one is empty); not when the theme's appearance policy sets `accent: false`, as `DEFAULT_QUASAR` does, so the scheme's `primary` is the theme's | `--q-primary` per mode (`!important`, over the scheme and the Quasar brand), in the `m-accent` style so it switches with the mode; in dark mode the link color is mixed from the dark accent |
| `THEME_COLOR_OVERRIDES` | a JSON `{name: {light, dark}}` (each a CSS color), one entry per token of the scheme; only when the theme's policy allows the colors | the same tokens the scheme's style sheet uses, in a style sheet after the scheme's style so the user's colors win |
| `THEME_MOTION` | `reduce`, `full`, empty (the system's reduced motion) | the body class `m-motion-reduce` or `m-motion-full` |
| `THEME_LINE_HEIGHT` | `relaxed` | `--m-line-height` 1.75 |
| `THEME_LINKS_UNDERLINE` | `true` | the body class `m-links-underline` |
| `THEME_RADIUS` | `square`, `round` | the body class `m-radius-square` or `m-radius-round` |
| `SHELL_DENSITY` | `comfortable`, `compact` | the `m-density-compact` body class |

The dialog: the mode toggle (light, dark, auto); the schemes as swatches, each previewing its light and dark `page-bg`, `panel-bg`, `text` and `primary`; the accent color as a color input for light and for dark, each with a "scheme color" button that clears it back to the scheme, shown only when the theme allows the user's accent; the three font roles as selects that render each option in its own `font-family`; the small and large text sizes; and the density, corners, motion and readability options.

### Appearance policy

The theme may decide what the user may override, and force the mode: a `STRT_APPEARANCE_POLICY` `ScreenThemeResource` holds JSON. Absent allows everything. `DEFAULT_QUASAR` sets `{"accent": false}`, so the primary light and dark colors follow the scheme. A POS or another authoritarian station theme can set:

```json
{ "appearance": false, "mode": "dark" }
```

- `appearance: false` hides the account menu's **Appearance…** and its dark/light toggle, and the Appearance box of BOSS Preferences; it also refuses every `THEME_*`, `QUASAR_DARK` and `SHELL_DENSITY` write. A `mode` of `light`, `dark` or `auto` forces the mode even when Appearance remains.
- The groups (`mode`, `density`, `scheme`, `accent`, `colors`, `fonts`, `sizes`, `motion`) each lock their controls in the Appearance dialog and BOSS Preferences when `false`; `lockedKeys` names single preferences. A locked preference leaves the theme's value in place and is refused.
- **Per user, station or tenant:** each `STRT_APPEARANCE_SERVICE` resource of the theme names a service called with `userId`, `screenThemeId` and the `appearance` so far, answering `appearance`. It may change the policy, the default scheme, or the schemes and fonts offered, so a station's day/night look or a tenant's brand needs no theme of its own. Each runs in its own transaction with authorization disabled; one that fails is logged and skipped.
- The shell sends the result as `config.appearance.policy`, with `lockedKeys` every key it locks; `moqui.appearanceAllowed(key)` hides the control and ignores a stale value. BOSS Preferences and `set#Appearance` read it through `org.moqui.impl.ShellServices.get#AppearancePolicy` (JSON text), with artifact authorization disabled because it is theme data.
- The server is the authority: `ec.user.setPreference` resolves the same policy and refuses a locked key, so `/apps/setPreference` (403), the generic preference service and BOSS `set#Appearance` cannot bypass the hidden controls. BOSS `set#Appearance` keeps a locked preference as it is rather than clearing it.
- A policy that is not a JSON object is logged and locks every appearance preference.

The policy is per theme, and a theme is assigned per user or group (`UserScreenTheme`, `UserGroupScreenTheme`), not per device. A station theme works for a shared station login or a station group with no personal theme; a user's own theme wins over a group's. Per-station day/night looks need separate station users/groups, or an `STRT_APPEARANCE_SERVICE` that decides by station.

## 6. Branding (the administrator)

- **The theme's default scheme** (`STRT_DEFAULT_COLOR_SCHEME`): the scheme a user gets without an own `THEME_COLOR_SCHEME` preference. The page links it, the Appearance dialog shows it as chosen, and the user's own choice wins over it. This is how a corporate look becomes the theme's default.
- **The System app's Theme screen** ([`tools/screen/System/Theme.xml`](../runtime/base-component/tools/screen/System/Theme.xml), menu-index 15): three form-lists over the generic CRUD services — the theme's resources, its color schemes (with their default fonts), and the schemes' colors (one row per mode and token, the token a drop-down of the style sheet's names and the shape tokens). It finds the theme through `get#Appearance` and shows the default scheme and the locked preferences in effect. A #hex color value is a swatch that opens the browser's color picker; another CSS value (rgb(), oklch(), var()) is a CSS value text field. A change shows on the next page load.
- **Per-color overrides are the user's, not the administrator's.** `THEME_COLOR_OVERRIDES` is edited in the BOSS Preferences screen's Appearance form ([`ironstone-boss/screen/Commons/Settings/Preferences.xml`](../runtime/component/ironstone-boss/screen/Commons/Settings/Preferences.xml)): the Accent (only when the theme allows the user's accent), Surfaces, Text, Lines and Brand colors sections, one light and one dark value for every token of the scheme, applied in a sheet after the scheme's. `set#Appearance` takes the overrides as the JSON `{name: {light, dark}}`, validates each name and CSS color, and keeps the rest when one is not valid.

## 7. Tests

- The page test "the appearance applies at once" ([`WebrootVue.qvt.shell.page.js`](../runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.shell.page.js)): the scheme style and font sheets, the tokens, a checked accent color and the body classes, and the dialog mounted on the account menu.
- The page test "the theme's appearance": the default scheme and its fonts, the user's color overrides, and the keys the policy locks; `crates/moqui-core/src/screen/sri/appearance_tests.rs` covers the policy parse, lock and denial rules, and the shell test of `crates/moqui-core/src/screen/tests/shell.rs` the 403 of a locked preference and the appearance services.
- OpenStep, Windows 11 and Plasma have no separate spec: their colors and `extraCss` are seed data in `WebrootQuasarThemeData.xml`; the white selection, flat header with the Search box, square buttons and dark button text of OpenStep, the rounded corners, card depth and accent-tinted selection of Windows 11, the solid accent selection, the thin-bordered flat buttons and the 3px-rounded cards of Plasma, and the gradient headers, page washes and complementary drawer accents of the corporate schemes (Slate in both modes, Crimson dark) were checked in the browser.

## Later

- A theme chooser with previews (the `STRT_SCREENSHOT` resource or a live swatch) and a screen that assigns a theme to a user or a group (`UserScreenTheme`, `UserGroupScreenTheme` are read, never written from a screen).
- A live preview on the Theme screen: the swatches of a scheme as its rows are edited, and a contrast check (the WCAG ratio) so a branding that does not read is flagged before it ships.
- An original OpenStep icon set for the shell (folder, disk, app, trash, document and toolbar icons). The real NeXTSTEP artwork is copyrighted; the current scheme uses the shell's Material Symbols.
- The appearance for the public (`STT_PUBLIC`) and error (`STT_ERROR`) themes; the public shell takes the theme's Quasar config only.
- The terminal client's colors from the scheme's tokens where the roles match; today it reads the `STRT_TUI_COLORS` resources.
