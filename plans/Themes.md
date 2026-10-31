# Themes

Built Oct 27–29, 2026 (commits c44d5f85, a5a6c555 and 06d5d5c9): the theme's default color scheme and the System app's Theme screen for corporate branding without code; light and dark accent colors, per-color scheme overrides and five new color schemes; and the scheme's colors inlined from the page config, so switching a scheme applies at once. The user guide is [docs/ui/Qvt-Client.md](../docs/ui/Qvt-Client.md) (Appearance); what is still open is in [Remaining-Work](Remaining-Work.md#screens-client-and-shell).

## Objective and scope

Two audiences for the look of the `/qapps` Quasar shell:

- **The administrator** sets the theme's branding without code: the shell's resources (the logo, the title, the footer items) and the theme's default color scheme, plus the color schemes and the fonts a user may choose.
- **The user** personalizes from the account menu's **Appearance…**: light, dark or auto, the color scheme, the accent color, the fonts, the text sizes, and the motion, corners and readability options.

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
| `ScreenThemeResource` | The theme's resources, one row per `STRT_*` type and value: the style sheets and scripts, the header logo, title and navbar components, the footer items, the Quasar config JSON (`STRT_QUASAR_CONFIG`), the shell's tray, quick-create, mail, chatter and record-files screens and counts service, the TUI widths, colors and keys, and the **`STRT_DEFAULT_COLOR_SCHEME`** (a `ScreenColorScheme` id of the theme) |
| `ScreenThemeIcon` | An icon class per text pattern |
| `ScreenColorScheme` | A scheme a user may choose: `colorSchemeId`, `screenThemeId`, description, sequence, and `extraCss` — CSS written after the colors for what is not a color; written by an administrator, never from what a user enters |
| `ScreenColorSchemeColor` | One color of a scheme: `colorModeEnumId` (`SCM_LIGHT`, `SCM_DARK`), `colorName` (a token name without its prefix), `colorValue` (a CSS color: `#hex`, `rgb()`, `hsl()`, `oklch()`, `color-mix()` or a `var()` of another token) |
| `ScreenFontOption` | A font a user may choose: `fontKindEnumId` (`SFK_SANS`, `SFK_SERIF`, `SFK_MONO`), a `font-family` stack with a generic family last, and an optional style sheet (a `/libs` path or an https URL; empty for a font the system has) |

The webroot seeds [`WebrootQuasarThemeData.xml`](../runtime/base-component/webroot/data/WebrootQuasarThemeData.xml) for `DEFAULT_QUASAR`:

- **Ten color schemes.** `DEFAULT` has no colors, so it is the theme's own (the style sheet's tokens and the `STRT_QUASAR_CONFIG` brand). The others set the standard colors for light and dark: Slate, Forest, Warm, High Contrast (black and white surfaces, darker brand steps, stronger borders and a wide focus ring in its extra CSS), Ocean, Indigo, Crimson, Teal and Graphite (a neutral, brand-free grey for a system that carries its own brand elsewhere).
- **Eleven fonts.** Five sans (System, Roboto, Inter, Source Sans 3, Atkinson Hyperlegible), three serif (System, Merriweather, Source Serif 4) and three mono (System, JetBrains Mono, IBM Plex Mono); the web fonts load from Google Fonts, as the theme's Roboto does.

**The tokens** are the names a scheme and a user's overrides address: the tokens of [`WebrootVue.qvt.css`](../runtime/base-component/webroot/screen/webroot/css/WebrootVue.qvt.css) without their prefix.

- the seven brand colors `primary`, `secondary`, `accent`, `positive`, `negative`, `info`, `warning` (Quasar's `--q-*`)
- `page-bg`, `panel-bg`, `header-bg`, `header-fg`, `drawer-bg`, `text`, `text-muted`, `link`, `negative-text`, `border`, `divider` (`--m-*`)

## 3. Tokens and cascade

The style sheet starts with the tokens: the fonts, text sizes and line height on `:root` (a user's preference sets them on the `html` element), the colors on the `body` per mode (`body:not(.body--dark)` and `body.body--dark`). The later sheet wins, in this order:

1. the Quasar brand the `STRT_QUASAR_CONFIG` JSON writes on the body
2. the theme's own defaults (the tokens above)
3. the scheme's sheet: a brand color as `--q-*` with `!important` (over the Quasar brand), the rest as `--m-*`; in dark mode `panel-bg` and `page-bg` also set Quasar's `--q-dark` and `--q-dark-page`, which Quasar's dark components use, and the link gives way to the user's dark accent (`var(--m-accent-dark-link, …)`)
4. the user's accent and overrides, in a sheet after the scheme link, so the user's colors win

A color name or value that could end its declaration (a name pattern, a value character set) is logged and left out, in both the transition and the page pre-action. The other borders and tints the style sheet keeps as black or white at a low alpha read on any surface and are not tokens.

## 4. Page load and instant switching

- The pre-actions of [`qapps.xml`](../runtime/base-component/webroot/screen/webroot/qapps.xml) fill `config.appearance`: the theme's color schemes (each with its light and dark colors, its generated CSS and a `version` hash of its colors and extra CSS), the fonts, the user's `THEME_*` values, the theme's `defaultSchemeId`, and the style sheets the page links.
- [`WebrootVue.qvt.html.fancy`](../runtime/base-component/webroot/screen/includes/WebrootVue.qvt.html.fancy) links the chosen scheme's and fonts' style sheets after the theme's, before the client starts, so the first paint is in the right colors.
- `moqui.applyAppearanceAll` sets the rest when the shell is created; each preference key has its applier in `moqui.preferenceAppliers`.
- **Switching a scheme applies at once without a fetch** (06d5d5c9): the scheme's colors are inlined from the page config the pre-action generated, and the linked sheet is replaced for the next load; the `version` hash busts the cache when an administrator changes a scheme.
- The `colorSchemeCss` transition of [`apps.xml`](../runtime/base-component/webroot/screen/webroot/apps.xml) (read-only, no session token; a scheme is not private) serves the same sheet as a URL for the page's link.

## 5. User preferences (Appearance)

The account menu's **Appearance…** opens `m-appearance-dialog`. Each choice applies at once (`moqui.applyPreference`) and is saved as the user's preference through `/apps/setPreference`; **Reset to defaults** clears the `THEME_*` keys. Browser zoom still scales the whole page; the sizes here are for the small and large text alone.

| Preference | Values | Applied as |
|---|---|---|
| `QUASAR_DARK` | `true`, `false`, `auto` | `$q.dark.set` |
| `THEME_COLOR_SCHEME` | a `ScreenColorScheme` id of the theme | the style sheet `/apps/colorSchemeCss?colorSchemeId=…&v=…` linked as `m-color-scheme`; none for a scheme without colors (the theme's own). Without the user's choice the theme's default scheme (`STRT_DEFAULT_COLOR_SCHEME`) is the effective one |
| `THEME_FONT_BODY`, `THEME_FONT_HEADING`, `THEME_FONT_MONO` | a `ScreenFontOption` id | `--m-font-body`, `--m-font-heading`, `--m-font-mono` on the html element, and the font's style sheet linked as `m-font-<role>` |
| `THEME_SIZE_SMALL`, `THEME_SIZE_LARGE` | `0.9`, `1`, `1.1`, `1.25`, `1.4` | `--m-fs-small-scale` (table text, captions, chips, code) and `--m-fs-large-scale` (headings) |
| `THEME_PRIMARY_COLOR`, `THEME_PRIMARY_COLOR_DARK` | a CSS color each (checked by `moqui.isCssColor`, else ignored), one for light and one for dark | `--q-primary` per mode (`!important`, over the scheme and the Quasar brand), in a style sheet so it switches with the mode; in dark mode the link color is mixed from the dark accent |
| `THEME_COLOR_OVERRIDES` | a JSON `{name: {light, dark}}` (each a CSS color), one entry per token of the scheme | the same tokens the scheme's style sheet uses, in a style sheet after the scheme link so the user's colors win |
| `THEME_MOTION` | `reduce`, `full`, empty (the system's reduced motion) | the body class `m-motion-reduce` or `m-motion-full` |
| `THEME_LINE_HEIGHT` | `relaxed` | `--m-line-height` 1.75 |
| `THEME_LINKS_UNDERLINE` | `true` | the body class `m-links-underline` |
| `THEME_RADIUS` | `square`, `round` | the body class `m-radius-square` or `m-radius-round` |
| `SHELL_DENSITY` | `comfortable`, `compact` | the `m-density-compact` body class |

The dialog: the mode toggle (light, dark, auto); the schemes as swatches, each previewing its light and dark `page-bg`, `panel-bg`, `text` and `primary`; the accent color as a color input for light and for dark, each with a "scheme color" button that clears it back to the scheme; the three font roles as selects that render each option in its own `font-family`; the small and large text sizes; and the density, corners, motion and readability options.

## 6. Branding (the administrator)

- **The theme's default scheme** (`STRT_DEFAULT_COLOR_SCHEME`): the scheme a user gets without an own `THEME_COLOR_SCHEME` preference. The page links it, the Appearance dialog shows it as chosen, and the user's own choice wins over it. This is how a corporate look becomes the theme's default.
- **The System app's Theme screen** ([`tools/screen/System/Theme.xml`](../runtime/base-component/tools/screen/System/Theme.xml), menu-index 15): three form-lists over the generic CRUD services — the theme's resources, its color schemes, and the schemes' colors (one row per mode and token, the token a drop-down of the style sheet's names). A change shows on the next page load.
- **Per-color overrides are the user's, not the administrator's.** `THEME_COLOR_OVERRIDES` is edited in the BOSS Preferences screen's Edit colors expander ([`ironstone-boss/screen/Commons/Settings/Preferences.xml`](../runtime/component/ironstone-boss/screen/Commons/Settings/Preferences.xml), the background `page-bg` and the menu bar `header-bg` today), applied in a sheet after the scheme's.

## 7. Tests

- The page test "the appearance applies at once" ([`WebrootVue.qvt.shell.page.js`](../runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.shell.page.js)): the scheme and font sheets, the tokens, a checked accent color and the body classes, and the dialog mounted on the account menu.

## Later

- A theme chooser with previews (the `STRT_SCREENSHOT` resource or a live swatch) and a screen that assigns a theme to a user or a group (`UserScreenTheme`, `UserGroupScreenTheme` are read, never written from a screen).
- A live preview on the Theme screen: the swatches of a scheme as its rows are edited, and a contrast check (the WCAG ratio) so a branding that does not read is flagged before it ships.
- The appearance for the public (`STT_PUBLIC`) and error (`STT_ERROR`) themes; the public shell takes the theme's Quasar config only.
- The terminal client's colors from the scheme's tokens where the roles match; today it reads the `STRT_TUI_COLORS` resources.
