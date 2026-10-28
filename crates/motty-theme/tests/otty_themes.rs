//! Parses every theme in the user's local Otty install, if present. Those
//! files are not redistributed with Motty, so this test is skipped when
//! `~/.config/otty/themes` does not exist.

#[test]
fn local_otty_themes_parse_without_warnings() {
    let dir = motty_theme::config::otty_dir().join("themes");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("skipping: {} not found", dir.display());
        return;
    };
    let mut failures = Vec::new();
    let mut count = 0;
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().and_then(|e| e.to_str()) != Some("ottytheme") {
            continue;
        }
        count += 1;
        let src = std::fs::read_to_string(&path).unwrap();
        match motty_theme::theme::parse(&src) {
            Ok(p) if p.warnings.is_empty() => {}
            Ok(p) => failures.push(format!("{}: {:?}", path.display(), p.warnings)),
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    eprintln!("parsed {count} themes");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
