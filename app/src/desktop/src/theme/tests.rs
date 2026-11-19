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
