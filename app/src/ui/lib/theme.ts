// Applies a theme (validated by the backend, src/desktop/src/theme) as CSS variables.

import type { Theme } from "./bindings/Theme";
import type { ThemeList } from "./bindings/ThemeList";

/** Same check as the backend's `is_color`; a second guard so no theme value can become CSS. */
export function isColor(v: string): boolean {
    if (v.length > 64) return false;
    if (v === "transparent") return true;
    if (/^#([0-9a-f]{3}|[0-9a-f]{4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.test(v))
        return true;
    const m = /^(rgb|rgba|hsl|hsla)\((.*)\)$/.exec(v);
    if (!m) return false;
    const inner = m[2].replaceAll("deg", "");
    return inner.trim() !== "" && /^[0-9 .,%/-]*$/.test(inner);
}

/** CSS variable name and value pairs for a theme. Keys must look like `bg` or `graph-1`. */
export function themeVars(theme: Theme): [string, string][] {
    return Object.entries(theme.colors)
        .filter(
            (e): e is [string, string] =>
                /^[a-z][a-z0-9-]*$/.test(e[0]) &&
                typeof e[1] === "string" &&
                isColor(e[1]),
        )
        .map(([k, v]) => [`--${k}`, v]);
}

export function pickTheme(list: ThemeList, id: string): Theme {
    return list.themes.find((t) => t.id === id) ?? list.themes[0];
}

export function applyTheme(
    theme: Theme,
    root: HTMLElement = document.documentElement,
) {
    for (const [name, value] of themeVars(theme)) {
        root.style.setProperty(name, value);
    }
    root.style.colorScheme = theme.scheme;
}
