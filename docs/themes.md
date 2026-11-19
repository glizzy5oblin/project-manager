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
