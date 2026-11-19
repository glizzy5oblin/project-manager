//! Themes: a fixed set of color variables the UI styles with (`--bg`, `--accent`, ...).
//!
//! Two themes are built in (`dark`, the default, and `light`). User themes are JSON files in the
//! `themes` folder of the app config directory:
//!
//! ```json
//! { "name": "Midnight", "scheme": "dark", "colors": { "bg": "#0b0d12", "accent": "#7aa2f7" } }
//! ```
//!
//! Colors left out come from the built-in theme of the same scheme. Only known keys are allowed
//! and every value must be a plain color literal (`#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`,
//! `rgba()`, `hsl()`, `hsla()` or `transparent`), so a theme file cannot inject CSS. A file with
//! any problem is skipped and reported, and the rest still load. See `docs/themes.md`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Every variable a theme sets, in display order.
pub const KEYS: &[&str] = &[
    "bg",
    "surface",
    "surface-2",
    "border",
    "text",
    "text-muted",
    "accent",
    "accent-text",
    "focus",
    "danger",
    "warning",
    "success",
    "graph-1",
    "graph-2",
    "graph-3",
    "graph-4",
    "graph-5",
    "graph-6",
    "graph-7",
    "graph-8",
];

const DARK: &[&str] = &[
    "#15171c", "#1d2027", "#262a33", "#323743", "#e3e6ec", "#8a91a0", "#5b9cf5", "#0b1220",
    "#8fbaff", "#ef6b6b", "#e5b454", "#5cc98a", "#5b9cf5", "#e0789f", "#5cc98a", "#e5b454",
    "#a98cf0", "#4fc1c9", "#ef8f5b", "#9aa3b5",
];

const LIGHT: &[&str] = &[
    "#f6f7f9", "#ffffff", "#eef0f4", "#d5d9e0", "#1b1f27", "#5d6575", "#2f6fd6", "#ffffff",
    "#1d5bbf", "#c93c3c", "#a86f00", "#23874e", "#2f6fd6", "#c2457a", "#23874e", "#a86f00",
    "#7552c9", "#1d8a93", "#c9632b", "#6b7385",
];

const _: () = assert!(DARK.len() == KEYS.len() && LIGHT.len() == KEYS.len());

/// Largest theme file read.
const MAX_THEME_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum ColorScheme {
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    /// `dark`, `light`, or the user theme's file name without `.json`.
    pub id: String,
    pub name: String,
    pub scheme: ColorScheme,
    pub builtin: bool,
    /// Every key in [`KEYS`], each a validated color.
    pub colors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ThemeProblem {
    pub file: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ThemeList {
    pub themes: Vec<Theme>,
    /// User theme files that were skipped, and why.
    pub problems: Vec<ThemeProblem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: String,
    scheme: ColorScheme,
    #[serde(default)]
    colors: BTreeMap<String, String>,
}

pub fn builtin(scheme: ColorScheme) -> Theme {
    let (id, name, values) = match scheme {
        ColorScheme::Dark => ("dark", "Dark", DARK),
        ColorScheme::Light => ("light", "Light", LIGHT),
    };
    Theme {
        id: id.into(),
        name: name.into(),
        scheme,
        builtin: true,
        colors: KEYS
            .iter()
            .zip(values)
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

/// Whether `v` is a color literal this engine accepts.
pub fn is_color(v: &str) -> bool {
    if v.len() > 64 {
        return false;
    }
    if v == "transparent" {
        return true;
    }
    if let Some(hex) = v.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    for f in ["rgb(", "rgba(", "hsl(", "hsla("] {
        if let Some(inner) = v.strip_prefix(f).and_then(|r| r.strip_suffix(')')) {
            let inner = inner.replace("deg", "");
            return !inner.trim().is_empty()
                && inner
                    .chars()
                    .all(|c| c.is_ascii_digit() || " .,%/-".contains(c));
        }
    }
    false
}

/// Parse one user theme. `id` is the file stem.
pub fn parse_theme(id: &str, text: &str) -> Result<Theme, String> {
    if id.is_empty()
        || id.len() > 40
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(
            "the file name must be 1 to 40 lowercase letters, digits, - or _, then .json".into(),
        );
    }
    if id == "dark" || id == "light" {
        return Err(format!(
            "\"{id}\" is the name of a built-in theme; rename the file"
        ));
    }
    let file: ThemeFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let name = file.name.trim();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        return Err("\"name\" must be 1 to 60 characters".into());
    }
    let mut theme = builtin(file.scheme);
    for (k, v) in &file.colors {
        if !KEYS.contains(&k.as_str()) {
            return Err(format!(
                "unknown color \"{k}\" (known: {})",
                KEYS.join(", ")
            ));
        }
        let v = v.trim();
        if !is_color(v) {
            return Err(format!(
                "\"{k}\": {v:?} is not a color (use #rrggbb, rgb(), hsl() or similar)"
            ));
        }
        theme.colors.insert(k.clone(), v.to_string());
    }
    theme.id = id.to_string();
    theme.name = name.to_string();
    theme.builtin = false;
    Ok(theme)
}

/// The built-in themes followed by the valid user themes in `dir`, sorted by name.
pub fn load(dir: &Path) -> ThemeList {
    let mut themes = vec![builtin(ColorScheme::Dark), builtin(ColorScheme::Light)];
    let mut user = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return ThemeList { themes, problems };
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    for path in paths {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let id = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let text = match std::fs::metadata(&path) {
            Ok(m) if !m.is_file() => continue,
            Ok(m) if m.len() > MAX_THEME_BYTES => Err("the file is larger than 64 KB".to_string()),
            _ => std::fs::read_to_string(&path).map_err(|e| e.to_string()),
        };
        match text.and_then(|t| parse_theme(&id, &t)) {
            Ok(t) => user.push(t),
            Err(message) => problems.push(ThemeProblem { file, message }),
        }
    }
    user.sort_by_key(|t| t.name.to_lowercase());
    themes.extend(user);
    ThemeList { themes, problems }
}

#[cfg(test)]
mod tests;
