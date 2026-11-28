# Themes

Project Manager's UI is styled with a fixed set of CSS color variables. A theme sets those variables and
nothing else. The code is in `app/src/desktop/src/theme/` (loading and validation) and
`app/src/ui/lib/theme.ts` (applying).

## Built-in themes

- `dark` (Dark): the default.
- `light` (Light).

## User themes

A user theme is a JSON file in the `themes` folder of the app config directory (on Linux,
`~/.config/io.github.thepotato456.projectmanager/themes/`). The folder is created on first start. The
file name, without `.json`, is the theme's id.

```json
{
    "name": "Midnight",
    "scheme": "dark",
    "colors": {
        "bg": "#0b0d12",
        "surface": "#141824",
        "accent": "#7aa2f7"
    }
}
```

- `name`: shown in the theme picker; 1 to 60 characters.
- `scheme`: `dark` or `light`. Every color left out comes from the built-in theme of this scheme,
  so a theme can change just one or two colors.
- `colors`: any of the keys below.

The selected theme id is saved in `settings.json` in the app data directory. If the selected
theme disappears, the app falls back to `dark`.

## Color keys

| Key                     | Used for                                    |
| ----------------------- | ------------------------------------------- |
| `bg`                    | Window background                           |
| `surface`, `surface-2`  | Cards, panels, inputs; raised or hovered    |
| `border`                | Borders and separators                      |
| `text`, `text-muted`    | Main and secondary text                     |
| `accent`, `accent-text` | Primary buttons and active tabs; their text |
| `focus`                 | Keyboard focus outline                      |
| `danger`                | Errors and destructive actions              |
| `warning`               | Warnings and changed files                  |
| `success`               | Clean state and successful actions          |
| `graph-1` … `graph-8`   | Branch lane colors in the graph (Phase 2)   |

## Allowed values

Each value must be a plain color literal: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`,
`rgba()`, `hsl()`, `hsla()` (numbers, `%`, `deg`, `,`, `/` and spaces only) or `transparent`.
Named colors, `var()`, `url()` and anything else are refused. The frontend checks every value
again before setting it, so a theme file cannot inject CSS.

## Errors

A file is skipped, and listed under "theme files could not be loaded" on the hub, when:

- its name is not 1 to 40 lowercase letters, digits, `-` or `_` followed by `.json`, or it is
  named `dark.json` or `light.json`;
- it is larger than 64 KB or is not valid JSON;
- it has an unknown key or an invalid color.

The other themes still load.

## Hot reload

Project Manager watches the themes folder. Saving, adding or deleting a file reloads the list, and the
current theme updates in place without a restart.

## Saving, importing and exporting

The themes live in the app's themes folder and, as described above, reload in place. The
preferences dialog (**Edit…** next to the theme picker) opens a theme editor for the current
theme: every color is an editable field with a live swatch. Saving a user theme rewrites its
file; saving a built-in theme gives it a new name and id and writes a user file for it.

- **Save** (user theme): rewrites the file with only the colors that differ from the built-in
  theme of the same scheme — the merged theme still shows every color.
- **Save as new theme** (built-in): writes a new user file whose id is the slug of the name
  (lowercased, hyphen-joined, 1–40 chars; `-` and `_` kept); the theme is selected and applied
  immediately.
- **Export…**: writes a complete file (every color, not only the diff) to a chosen path,
  re-loadable with the normal loader.
- **Import theme…** (preferences dialog): validates a chosen `.json` file with the same rules
  as the loader and copies it into the themes folder; the new theme's id is the file name
  (without `.json`), it is selected and applied immediately. Export is the only way to get a
  theme into a different machine, import brings it back.
- Import/export/save-as errors (an unknown color key, a bad color, an existing id, an
  unreadable file) are reported in the dialog and change nothing.

`theme_save_user` (id, colors), `theme_save_as` (name, scheme, colors) and
`theme_import` (path) return the new theme id; `theme_export` takes (id, path). All four are
registered in `lib.rs` and wrap `theme/mod.rs` (`save_user`, `save_as`, `import_theme`,
`export_theme`).
