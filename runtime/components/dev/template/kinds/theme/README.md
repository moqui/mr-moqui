# @component@

@Title@, a theme of the `/qapps` shell: a `ScreenTheme` built on the Default theme, with a style sheet and a color scheme
of its own. A user picks it in the account menu's Theme; a group gets it as its `UserGroupScreenTheme`.

| Path | What is here |
| --- | --- |
| `component.xml` | The component: its name and version. |
| `MoquiConf.xml` | Serves `screen/@component@/` at `/@component@/`. |
| `screen/@component@.xml`, `screen/@component@/@Class@.css` | The theme's files: its style sheet, scoped to its body class. |
| `data/@Class@ThemeData.xml` | Seed: the theme `@ID@_QUASAR` (parent `DEFAULT_QUASAR`), its style sheets, body class, Quasar brand and the color scheme `@ID@`. |
| `test/@Class@Theme.spec.js` | The regression suite: the theme's rows. |

A theme sets only what it changes: a resource type it has a row of replaces the parent's (so its style sheets list the
Default's first), and the rest comes from the parent. The rules and the theme values: `docs/ui/Qvt-Client.md`, Themes,
in mr-moqui; the framework's Green Screen (`runtime/components/base/webroot/data/WebrootQuasarThemeData.xml`) is a fuller
example.

## Run and test

- `./moq load` loads its seed data; the theme shows in the account menu after the next start.
- `./moq test @component@` runs its regression specs.
