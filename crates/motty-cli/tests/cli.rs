//! The configuration commands end to end: the built `motty` run with a
//! throwaway HOME, so nothing touches the real config.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "motty-cli-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn motty(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_motty"))
            .args(args)
            .env("HOME", &self.0)
            .env_remove("MOTTY_CONFIG_FILE")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("MOTTY_PANE_ID")
            // Never the running app.
            .env("MOTTY_SOCKET", self.0.join("no.sock"))
            .output()
            .unwrap()
    }

    /// stdout of a run that must succeed.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.motty(args);
        assert!(
            out.status.success(),
            "motty {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn config(&self) -> PathBuf {
        self.0.join(".config/motty")
    }

    fn read(&self, path: impl AsRef<Path>) -> String {
        std::fs::read_to_string(self.config().join(path)).unwrap_or_default()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn config_commands() {
    let home = Home::new("config");
    let path = home.ok(&["config", "path"]);
    assert_eq!(
        path.trim(),
        home.config().join("config.toml").to_str().unwrap()
    );
    home.ok(&["config", "set", "font-size", "14"]);
    // The effective value, as the app reads it.
    assert_eq!(home.ok(&["config", "get", "font-size"]), "14.0\n");
    assert!(home.read("config.toml").ends_with("font-size = 14\n"));

    // Rejected values exit non-zero and write nothing.
    let before = home.read("config.toml");
    for bad in [
        &["config", "set", "font-size", "big"][..],
        &["config", "set", "theme", "No Such Theme"],
        &["config", "set", "cursor-style", "triangle"],
        &["config", "set", "no-such-key", "1"],
        &["config", "set", "env", "+A=1", "B=2"],
        &["config", "set", "font-size", "15", "--transient"],
    ] {
        assert!(!home.motty(bad).status.success(), "{bad:?} passed");
    }
    assert_eq!(home.read("config.toml"), before);

    // Machine-local keys win and keep their writes.
    home.ok(&["config", "set", "--local", "command", "/bin/zsh"]);
    home.ok(&["config", "set", "shell", "/bin/bash"]);
    assert_eq!(home.read("config.local.toml"), "command = \"/bin/bash\"\n");
    let origin = home.ok(&["config", "get", "command", "--show-origin"]);
    assert!(
        origin.contains("config.local.toml:1\t/bin/bash"),
        "{origin}"
    );

    // Lists: whole, then signed entries; the result is printed.
    home.ok(&["config", "set", "env", "EDITOR=vim", "PAGER=less"]);
    let out = home.ok(&["config", "set", "env", "+EDITOR=nvim", "-PAGER"]);
    assert_eq!(out, "EDITOR=nvim\n");
    assert_eq!(home.ok(&["config", "get", "env"]), "EDITOR=nvim\n");

    // Unset: commented out in config.toml, gone from config.local.toml.
    home.ok(&["config", "unset", "font-size"]);
    assert!(
        home.read("config.toml")
            .contains("# font-size = 14 (reset to default)\n")
    );
    assert_eq!(home.ok(&["config", "get", "font-size"]), "13.0\n");
    home.ok(&["config", "unset", "command"]);
    assert_eq!(home.read("config.local.toml"), "");

    assert_eq!(home.ok(&["config", "validate"]), "No problems found.\n");
    std::fs::write(
        home.config().join("config.toml"),
        "font-sise = 1\ncursor-style = \"x\"\n",
    )
    .unwrap();
    let out = home.motty(&["config", "validate"]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(":1: unknown key \"font-sise\""), "{text}");
    assert!(text.contains(":2: cursor-style:"), "{text}");

    // The schema lists every key, as JSON under data.keys.
    let schema: serde_json::Value =
        serde_json::from_str(&home.ok(&["--json", "config", "schema"])).unwrap();
    let keys = schema["data"]["keys"].as_array().unwrap();
    assert_eq!(keys.len(), motty_theme::schema::KEYS.len());
    let theme = keys.iter().find(|k| k["key"] == "theme").unwrap();
    assert_eq!(theme["settings"], "Appearance › Theme");
    let show: serde_json::Value =
        serde_json::from_str(&home.ok(&["--json", "config", "show"])).unwrap();
    assert_eq!(show["data"]["cursor_style"], "x");
    assert_eq!(show["data"]["font_size"], 13.0);
}

const KITTY_THEME: &str =
    "foreground #c0caf5\nbackground #1a1b26\ncolor0 #15161e\ncolor1 #f7768e\n";

#[test]
fn theme_import_and_list() {
    let home = Home::new("theme");
    let file = home.0.join("My Night.conf");
    std::fs::write(&file, KITTY_THEME).unwrap();
    let out = home.ok(&["theme", "import", file.to_str().unwrap(), "--activate"]);
    assert!(
        out.contains("Imported My Night (dark theme from Kitty)"),
        "{out}"
    );
    let themes = home.config().join("themes");
    assert!(themes.join("my-night.ottytheme").is_file());
    assert!(home.read("config.toml").contains("theme = \"My Night\"\n"));
    // Again: a copy beside it; with --overwrite, in place.
    home.ok(&["theme", "import", file.to_str().unwrap()]);
    assert!(themes.join("my-night-1.ottytheme").is_file());
    std::fs::remove_file(themes.join("my-night-1.ottytheme")).unwrap();
    home.ok(&["theme", "import", file.to_str().unwrap(), "--overwrite"]);
    assert_eq!(std::fs::read_dir(&themes).unwrap().count(), 1);

    let list: serde_json::Value =
        serde_json::from_str(&home.ok(&["--json", "theme", "list", "--color", "dark"])).unwrap();
    let data = list["data"].as_array().unwrap();
    let mine = data.iter().find(|t| t["name"] == "My Night").unwrap();
    assert_eq!(mine["source"], "user");
    assert_eq!(mine["variant"], "dark");
    assert_eq!(mine["colors"]["background"], "#1a1b26");
    assert!(data.iter().all(|t| t["variant"] == "dark"));
    let nord = data.iter().find(|t| t["name"] == "Nord").unwrap();
    assert_eq!(nord["source"], "builtin");
    for key in [
        "background",
        "foreground",
        "surface",
        "accent",
        "secondary",
        "tertiary",
    ] {
        let c = nord["colors"][key].as_str().unwrap();
        assert!(c.len() == 7 && c.starts_with('#'), "{key}: {c}");
    }
    // `config set theme` now takes it.
    home.ok(&["config", "set", "theme-dark", "my night"]);

    // A bare `motty import` of a theme file.
    let light = home.0.join("Paperish.conf");
    std::fs::write(&light, "foreground #111111\nbackground #fafafa\n").unwrap();
    home.ok(&["import", light.to_str().unwrap()]);
    assert!(themes.join("paperish.ottytheme").is_file());
}

#[test]
fn font_import_and_apply() {
    let home = Home::new("font");
    let font = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../motty-app/assets/fonts/jetbrains-mono/JetBrainsMono-Bold.ttf");
    let out = home.ok(&["font", "import", font.to_str().unwrap(), "--apply"]);
    assert!(out.contains("Imported JetBrains Mono"), "{out}");
    assert!(home.config().join("fonts/JetBrainsMono-Bold.ttf").is_file());
    assert!(
        home.read("config.toml")
            .contains("font-family = \"JetBrains Mono\"\n")
    );
    // A bare `motty import` of a font file.
    std::fs::remove_dir_all(home.config().join("fonts")).unwrap();
    home.ok(&["import", font.to_str().unwrap()]);
    assert!(home.config().join("fonts/JetBrainsMono-Bold.ttf").is_file());
    // Not a font.
    assert!(
        !home
            .motty(&["font", "import", "/etc/hosts"])
            .status
            .success()
    );
    let list = home.ok(&["font", "list", "--family", "jetbrains mono", "--monospace"]);
    assert!(list.lines().any(|l| l == "JetBrains Mono"), "{list}");
    home.ok(&["font", "apply", "jetbrains mono"]);
    assert!(
        !home
            .motty(&["font", "apply", "No Such Font 123"])
            .status
            .success()
    );
}

#[test]
fn import_ghostty_from_its_usual_place() {
    let home = Home::new("ghostty");
    let ghostty = home.0.join(".config/ghostty");
    std::fs::create_dir_all(&ghostty).unwrap();
    std::fs::write(
        ghostty.join("config"),
        "font-size = 15\ncursor-style = bar\nbackground-opacity = 0.9\n",
    )
    .unwrap();
    std::fs::create_dir_all(home.config()).unwrap();
    std::fs::write(
        home.config().join("config.toml"),
        "# mine\nfont-size = 12\n",
    )
    .unwrap();
    // A preview writes nothing.
    let out = home.ok(&["import", "ghostty"]);
    assert!(out.contains("Conflicts (1"), "{out}");
    assert!(out.contains("Nothing written"), "{out}");
    assert_eq!(home.read("config.toml"), "# mine\nfont-size = 12\n");
    home.ok(&["import", "ghostty", "--keep"]);
    assert_eq!(
        home.read("config.toml"),
        "# mine\nfont-size = 12\ncursor-style = \"bar\"\n"
    );
    let json: serde_json::Value =
        serde_json::from_str(&home.ok(&["--json", "import", "ghostty", "--overwrite"])).unwrap();
    assert_eq!(json["data"]["written"], 1);
    assert_eq!(
        json["data"]["skipped"][0]["from"],
        "background-opacity = 0.9"
    );
    assert_eq!(
        home.read("config.toml"),
        "# mine\nfont-size = 15.0\ncursor-style = \"bar\"\n"
    );
    // And back out.
    let out = home.ok(&["export", "ghostty"]);
    assert!(
        out.contains("font-size = 15\n") && out.contains("cursor-style = bar\n"),
        "{out}"
    );
}
