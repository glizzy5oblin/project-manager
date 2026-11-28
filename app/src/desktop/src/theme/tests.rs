use super::*;

#[test]
fn colors() {
    for ok in [
        "#abc",
        "#abcd",
        "#a1b2c3",
        "#A1B2C3ff",
        "rgb(1, 2, 3)",
        "rgba(1 2 3 / 50%)",
        "hsl(210deg 50% 40%)",
        "hsla(210, 50%, 40%, 0.5)",
        "transparent",
    ] {
        assert!(is_color(ok), "{ok}");
    }
    for bad in [
        "",
        "#ab",
        "#abcdefg",
        "red",
        "rgb()",
        "rgb(1,2,3); background: url(https://evil/x)",
        "url(https://evil/x)",
        "var(--bg)",
        "rgb(1,2,3) !important",
        "expression(alert(1))",
        "#fff;}body{display:none",
        &format!("rgb({})", "1,".repeat(40)),
    ] {
        assert!(!is_color(bad), "{bad}");
    }
}

#[test]
fn builtins_are_complete_and_valid() {
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        let t = builtin(scheme);
        assert_eq!(t.colors.len(), KEYS.len());
        assert!(t.colors.values().all(|v| is_color(v)));
    }
}

/// A relative-luminance / contrast helper, copied locally so the test reads plainly.
fn luminance(hex: &str) -> f64 {
    let hex = hex.trim_start_matches('#');
    let rgb = |i: usize| {
        let c = u8::from_str_radix(&hex[i..i + 2], 16).unwrap() as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * rgb(0) + 0.7152 * rgb(2) + 0.0722 * rgb(4)
}

fn contrast(a: &str, b: &str) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = (la.max(lb), la.min(lb));
    (hi + 0.05) / (lo + 0.05)
}

#[test]
fn builtin_themes_meet_wcag_contrast_ratios() {
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        let t = builtin(scheme);
        let bg = &t.colors["bg"];
        // Text pairs must clear 4.5:1 (WCAG AA).
        let text_pairs = [
            ("text", "bg", 4.5),
            ("text", "surface", 4.5),
            ("text", "surface-2", 4.5),
            ("text-muted", "bg", 4.5),
            ("text-muted", "surface-2", 4.5),
            ("accent", "bg", 4.5),
            ("accent-text", "accent", 4.5),
            ("focus", "bg", 4.5),
            ("danger", "bg", 4.5),
            ("warning", "bg", 4.5),
            ("success", "bg", 4.5),
        ];
        for (fg, bg_key, min) in text_pairs {
            let ratio = contrast(&t.colors[fg], &t.colors[bg_key]);
            assert!(
                ratio >= min,
                "{scheme:?}: {fg} on {bg_key} is {ratio:.2}, needs {min}"
            );
        }
        // Graph lanes double as decorative color: 3:1 against the background (WCAG non-text).
        for key in 1..=8 {
            let lane_key = format!("graph-{key}");
            let ratio = contrast(&t.colors[&lane_key], bg);
            assert!(
                ratio >= 3.0,
                "{scheme:?}: lane {lane_key} is {ratio:.2}, needs 3.0"
            );
        }
    }
}

/// `app.css` holds the dark theme so the first paint (before the theme loads) matches it.
#[test]
fn app_css_matches_builtin_dark() {
    let css = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/app.css"))
        .unwrap();
    let dark = builtin(ColorScheme::Dark);
    for (k, v) in &dark.colors {
        let decl = format!("--{k}: {v};");
        assert!(css.contains(&decl), "app.css is missing `{decl}`");
    }
}

#[test]
fn user_theme_fills_missing_colors_from_its_scheme() {
    let t = parse_theme(
        "paper",
        r##"{ "name": "Paper", "scheme": "light", "colors": { "accent": "#ff0000" } }"##,
    )
    .unwrap();
    assert_eq!(t.colors["accent"], "#ff0000");
    assert_eq!(t.colors["bg"], builtin(ColorScheme::Light).colors["bg"]);
    assert_eq!(t.colors.len(), KEYS.len());
    assert!(!t.builtin);
}

#[test]
fn bad_themes_are_rejected_with_a_reason() {
    let cases = [
        (
            "x",
            r##"{"name":"X","scheme":"dark","colors":{"bgg":"#000"}}"##,
            "unknown color",
        ),
        (
            "x",
            r#"{"name":"X","scheme":"dark","colors":{"bg":"red"}}"#,
            "not a color",
        ),
        ("x", r#"{"name":"X","scheme":"dim"}"#, "unknown variant"),
        (
            "x",
            r#"{"name":"X","scheme":"dark","extra":1}"#,
            "unknown field",
        ),
        ("x", r#"{"name":"","scheme":"dark"}"#, "name"),
        ("Bad Name", r#"{"name":"X","scheme":"dark"}"#, "file name"),
        ("dark", r#"{"name":"X","scheme":"dark"}"#, "built-in"),
        ("x", "not json", "expected"),
    ];
    for (id, text, want) in cases {
        let err = parse_theme(id, text).unwrap_err();
        assert!(err.contains(want), "{id} {text}: {err}");
    }
}

#[test]
fn load_folder() {
    let dir = tempfile::tempdir().unwrap();
    let w = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).unwrap();
    w("zeta.json", r#"{"name":"Zeta","scheme":"dark"}"#);
    w("alpha.json", r#"{"name":"alpha","scheme":"light"}"#);
    w("broken.json", "{");
    w("notes.txt", "ignored");
    std::fs::write(dir.path().join("huge.json"), vec![b' '; 70 * 1024]).unwrap();
    std::fs::create_dir(dir.path().join("folder.json")).unwrap();

    let list = load(dir.path());
    let ids: Vec<_> = list.themes.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["dark", "light", "alpha", "zeta"]);
    let bad: Vec<_> = list.problems.iter().map(|p| p.file.as_str()).collect();
    assert_eq!(bad, ["broken.json", "huge.json"]);

    let missing = load(&dir.path().join("none"));
    assert_eq!(missing.themes.len(), 2);
    assert!(missing.problems.is_empty());
}

fn temp_dir(tag: &str) -> tempfile::TempDir {
    let mut p = std::env::temp_dir();
    p.push(format!("pm-theme-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    tempfile::Builder::new()
        .prefix("pm-theme-")
        .tempdir()
        .unwrap()
}

fn merged(scheme: ColorScheme) -> BTreeMap<String, String> {
    builtin(scheme).colors.clone()
}

#[test]
fn slug_sanitizes_names() {
    assert_eq!(slug("Midnight Blue"), Some("midnight-blue".into()));
    assert_eq!(
        slug("  Spaced_Out--Theme! "),
        Some("spaced_out-theme".into())
    );
    assert_eq!(slug("dark"), Some("dark".into()));
    assert_eq!(slug("---"), None);
    assert_eq!(slug("..."), None);
    assert!(slug("a".repeat(60).as_str()).unwrap().len() <= 40);
}

#[test]
fn save_as_writes_only_the_diff_and_returns_the_id() {
    let dir = temp_dir("save-as");
    let mut colors = merged(ColorScheme::Light);
    colors.insert("accent".into(), "#ff0000".into());

    let id = save_as(dir.path(), "Sunset Theme", ColorScheme::Light, &colors).unwrap();
    assert_eq!(id, "sunset-theme");
    let text = std::fs::read_to_string(dir.path().join("sunset-theme.json")).unwrap();
    let theme = parse_theme(&id, &text).unwrap();
    assert_eq!(theme.name, "Sunset Theme");
    assert_eq!(theme.scheme, ColorScheme::Light);
    assert_eq!(theme.colors.get("accent").unwrap(), "#ff0000");
    // The untouched keys come from the built-in theme of the same scheme.
    assert_eq!(theme.colors["bg"], builtin(ColorScheme::Light).colors["bg"]);
    // Only the diff is stored, not every key.
    assert_eq!(text.matches("#").count(), 1);
}

#[test]
fn save_as_refuses_duplicates_and_unusable_names() {
    let dir = temp_dir("save-as-dup");
    std::fs::write(
        dir.path().join("midnight.json"),
        r#"{"name":"Midnight","scheme":"dark"}"#,
    )
    .unwrap();
    let colors = merged(ColorScheme::Dark);
    let e = save_as(dir.path(), "midnight", ColorScheme::Dark, &colors).unwrap_err();
    assert!(e.contains("already exists"), "{e}");
    let e = save_as(dir.path(), "!!!", ColorScheme::Dark, &colors).unwrap_err();
    assert!(e.contains("letter or digit"), "{e}");
    let e = save_as(dir.path(), "", ColorScheme::Dark, &colors).unwrap_err();
    assert!(e.contains("name is required"), "{e}");
}

#[test]
fn save_user_rewrites_the_existing_file() {
    let dir = temp_dir("save-user");
    std::fs::write(
        dir.path().join("midnight.json"),
        r##"{"name":"Midnight","scheme":"dark","colors":{"bg":"#101010"}}"##,
    )
    .unwrap();
    let initial = std::fs::read_to_string(dir.path().join("midnight.json")).unwrap();
    let mut colors = parse_theme("midnight", &initial).unwrap().colors;
    colors.insert("accent".into(), "#123456".into());

    save_user(dir.path(), "midnight", &colors).unwrap();
    let text = std::fs::read_to_string(dir.path().join("midnight.json")).unwrap();
    let theme = parse_theme("midnight", &text).unwrap();
    assert_eq!(theme.name, "Midnight");
    assert_eq!(theme.colors.get("accent").unwrap(), "#123456");
    // The bg override is kept (not rewritten to the built-in default).
    assert_eq!(theme.colors["bg"], "#101010");
}

#[test]
fn save_user_rejects_unknown_keys_and_missing_themes() {
    let dir = temp_dir("save-user-bad");
    let mut colors = merged(ColorScheme::Dark);
    colors.insert("nope".into(), "#101010".into());
    let e = save_user(dir.path(), "midnight", &colors).unwrap_err();
    assert!(e.contains("unknown color"), "{e}");

    let colors = merged(ColorScheme::Dark);
    let e = save_user(dir.path(), "ghost", &colors).unwrap_err();
    assert!(e.contains("no user theme"), "{e}");
}

#[test]
fn export_writes_a_complete_file() {
    let dir = temp_dir("export");
    let mut colors = merged(ColorScheme::Dark);
    colors.insert("accent".into(), "#123456".into());
    let id = save_as(dir.path(), "Sunset Theme", ColorScheme::Dark, &colors).unwrap();
    let theme = load(dir.path())
        .themes
        .into_iter()
        .find(|t| t.id == id)
        .unwrap();

    let out = dir.path().join("exported.json");
    export_theme(&theme, &out).unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    let reparsed = parse_theme(&id, &text).unwrap();
    assert_eq!(reparsed.name, "Sunset Theme");
    assert_eq!(reparsed.colors, theme.colors);
}

#[test]
fn import_validates_and_copies_the_file() {
    let themes_dir = temp_dir("import-themes");
    let source_dir = temp_dir("import-source");
    let src = source_dir.path().join("source.json");
    std::fs::write(
        &src,
        r##"{"name":"Quiet Blue","scheme":"dark","colors":{"bg":"#101010"}}"##,
    )
    .unwrap();
    let id = import_theme(themes_dir.path(), &src).unwrap();
    assert_eq!(id, "source");
    let copied = std::fs::read_to_string(themes_dir.path().join("source.json")).unwrap();
    assert_eq!(copied, std::fs::read_to_string(&src).unwrap());

    let e = import_theme(themes_dir.path(), &src).unwrap_err();
    assert!(e.contains("already exists"), "{e}");
}

#[test]
fn import_rejects_bad_files() {
    let themes_dir = temp_dir("import-bad-themes");
    let source_dir = temp_dir("import-bad-source");

    let src = source_dir.path().join("bad.json");
    std::fs::write(&src, "{").unwrap();
    assert!(import_theme(themes_dir.path(), &src).is_err());

    // A stem with no letter or digit cannot name a theme.
    let src = source_dir.path().join("---.json");
    std::fs::write(&src, r#"{"name":"x","scheme":"dark"}"#).unwrap();
    let e = import_theme(themes_dir.path(), &src).unwrap_err();
    assert!(e.contains("letter or digit"), "{e}");
}
