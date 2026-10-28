//! The welcome guide: a card shown over the first window on first launch
//! (and from the palette later) that walks through what to set up: what
//! opens at launch, a theme, agents and the `motty` command, and the
//! shortcuts worth knowing. Choices are written to Motty's config.toml as
//! they're made, so the window behind shows them right away.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    Action, AnyElement, ClipboardItem, Context, Entity, FocusHandle, Focusable, Hsla, Image,
    ImageFormat, KeyDownEvent, MouseButton, ScrollHandle, SharedString, Window, div, img,
    prelude::*, px,
};
use motty_render::hsla;
use motty_theme::{Mode, Rgba};

use super::Workspace;
use super::palette::keycaps;
use super::picker::{self, Colors};
use crate::actions::*;
use crate::terminal_view::TerminalView;

const WIDTH: f32 = 600.;
const BODY_HEIGHT: f32 = 404.;
const FOOTER_HEIGHT: f32 = 56.;
const PAD: f32 = 28.;
const SWATCH_WIDTH: f32 = 169.;
const SWATCH_PREVIEW: f32 = 62.;
/// Used when there's no theme setting: Motty's own, following the system.
const AUTO_LIGHT: &str = "Motty Light";
const AUTO_DARK: &str = "Motty Dark";
const ICON: &[u8] = include_bytes!("../../assets/icon/motty-256.png");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Hello,
    Launch,
    Theme,
    Agents,
    Shortcuts,
}

const STEPS: [Step; 5] = [
    Step::Hello,
    Step::Launch,
    Step::Theme,
    Step::Agents,
    Step::Shortcuts,
];

pub(super) struct Welcome {
    focus: FocusHandle,
    step: usize,
    /// Where focus goes back to, and whose key bindings the shortcuts list.
    target: Option<Entity<TerminalView>>,
    icon: Arc<Image>,
    themes: Vec<ThemeChoice>,
    scroll: ScrollHandle,
    /// The command line last copied.
    copied: Option<usize>,
    install: Install,
    /// Why the last choice couldn't be saved.
    error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Install {
    Idle,
    Running,
    Done(String),
    Failed(String),
}

struct ThemeChoice {
    name: String,
    mode: Mode,
    background: Rgba,
    foreground: Rgba,
    accents: [Rgba; 6],
}

/// Where finishing (or skipping) the guide is remembered.
fn done_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Motty/onboarding.json")
}

/// Whether to show the guide at launch: once, and never for scripted or
/// secondary instances (the same ones that don't keep a session).
pub fn first_run() -> bool {
    crate::recovery::enabled() && !done_path().exists()
}

fn mark_done() {
    let path = done_path();
    let result = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
        .and_then(|_| std::fs::write(&path, "{\"version\":1}\n"));
    if let Err(e) = result {
        tracing::warn!("couldn't save {}: {e}", path.display());
    }
}

fn load_themes() -> Vec<ThemeChoice> {
    motty_theme::config::list_themes()
        .into_iter()
        .filter_map(|(name, mode, _)| {
            let theme = motty_theme::config::find_theme(&name)?.theme;
            let t = &theme.terminal;
            // A see-through terminal shows the window or panel behind it.
            let background = [
                t.background,
                theme.window.background,
                theme.panel.background,
            ]
            .into_iter()
            .find(|c| c.a > 0.5)
            .unwrap_or(if mode == Mode::Dark {
                Rgba::rgb(0x1e1e1e)
            } else {
                Rgba::rgb(0xf5f5f5)
            })
            .with_alpha(1.);
            let p = &t.palette;
            Some(ThemeChoice {
                name,
                mode,
                background,
                foreground: t.foreground.with_alpha(1.),
                accents: [p[1], p[3], p[2], p[6], p[4], p[5]],
            })
        })
        .collect()
}

/// The `motty` command as other programs should run it: by name when it's
/// on the usual PATH, else by its place in the app.
pub(crate) fn cli_command() -> String {
    let Some(cli) = super::cli_dir().map(|d| d.join("motty")) else {
        return "motty".into();
    };
    let linked = std::fs::canonicalize("/usr/local/bin/motty").ok();
    if linked.as_deref() == Some(cli.as_path()) {
        return "motty".into();
    }
    let path = cli.to_string_lossy().into_owned();
    if path.contains(|c: char| c.is_whitespace() || "'\"$`\\".contains(c)) {
        format!("'{}'", path.replace('\'', r"'\''"))
    } else {
        path
    }
}

/// Links /usr/local/bin/motty to the app's CLI, asking for an administrator
/// password when the folder isn't writable.
pub(crate) fn install_cli(cli: &Path) -> Result<String, String> {
    let link = Path::new("/usr/local/bin/motty");
    let direct = std::fs::create_dir_all("/usr/local/bin").and_then(|_| {
        match std::fs::remove_file(link) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        std::os::unix::fs::symlink(cli, link)
    });
    if direct.is_ok() {
        return Ok("Installed at /usr/local/bin/motty.".into());
    }
    let quoted = format!("'{}'", cli.to_string_lossy().replace('\'', r"'\''"));
    let script = format!(
        "do shell script \"mkdir -p /usr/local/bin && ln -sf {} /usr/local/bin/motty\" \
         with administrator privileges",
        quoted.replace('\\', r"\\").replace('"', "\\\"")
    );
    let out = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok("Installed at /usr/local/bin/motty.".into())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(if err.contains("-128") {
            "Cancelled.".into()
        } else {
            err.trim().to_owned()
        })
    }
}

impl Workspace {
    /// Opens the guide at its first step (or brings it back).
    pub fn show_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.open_quickly = None;
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.welcome = Some(Welcome {
            focus,
            step: 0,
            target: self.focused_pane(),
            icon: Arc::new(Image::from_bytes(ImageFormat::Png, ICON.to_vec())),
            themes: load_themes(),
            scroll: ScrollHandle::new(),
            copied: None,
            install: Install::Idle,
            error: None,
        });
        cx.notify();
    }

    fn close_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(welcome) = self.welcome.take() {
            mark_done();
            if let Some(target) = welcome.target {
                window.focus(&target.focus_handle(cx), cx);
            }
        }
        cx.notify();
    }

    fn welcome_step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(w) = self.welcome.as_mut() else {
            return;
        };
        let next = w.step as isize + delta;
        if next >= STEPS.len() as isize {
            return self.close_welcome(window, cx);
        }
        w.step = next.max(0) as usize;
        w.error = None;
        cx.notify();
    }

    /// Writes `pairs` to the config and applies them to this window now
    /// (others notice the file change).
    fn welcome_save(&mut self, pairs: &[(&str, &str)], cx: &mut Context<Self>) {
        let values: Vec<(&str, toml::Value)> = pairs
            .iter()
            .map(|(k, v)| (*k, toml::Value::String((*v).into())))
            .collect();
        let result = motty_theme::config::save_values(&values);
        if let Some(w) = self.welcome.as_mut() {
            w.error = result
                .as_ref()
                .err()
                .map(|e| format!("Couldn't save config.toml: {e}"));
        }
        if result.is_ok() {
            self.reload_config(cx);
        }
        cx.notify();
    }

    fn on_welcome_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        if m.platform || m.control || m.alt {
            return;
        }
        match ks.key.as_str() {
            "enter" | "right" => self.welcome_step(1, window, cx),
            "left" => self.welcome_step(-1, window, cx),
            "escape" => self.close_welcome(window, cx),
            _ => {}
        }
        cx.stop_propagation();
    }

    pub(super) fn render_welcome(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let w = self.welcome.as_ref()?;
        let c = Colors::new(&self.theme);
        let step = STEPS[w.step];
        let body = match step {
            Step::Hello => self.welcome_hello(w, &c),
            Step::Launch => self.welcome_launch(&c, cx),
            Step::Theme => self.welcome_theme(w, &c, cx),
            Step::Agents => self.welcome_agents(w, &c, cx),
            Step::Shortcuts => self.welcome_shortcuts(w, &c, window, cx),
        };
        let last = w.step + 1 == STEPS.len();
        let dots = STEPS.iter().enumerate().map(|(ix, _)| {
            div()
                .id(("welcome-dot", ix))
                .size(px(7.))
                .rounded_full()
                .bg(if ix == w.step { c.text } else { c.selected })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(w) = this.welcome.as_mut() {
                        w.step = ix;
                        cx.notify();
                    }
                }))
        });
        let footer = div()
            .flex_none()
            .h(px(FOOTER_HEIGHT))
            .px(px(PAD - 8.))
            .flex()
            .items_center()
            .border_t(px(1.))
            .border_color(c.separator)
            .child(div().pl(px(8.)).flex().gap(px(8.)).children(dots))
            .child(div().flex_1())
            .when(!last, |d| {
                d.child(
                    button("welcome-skip", "Skip", false, &self.theme, &c).on_click(
                        cx.listener(|this, _, window, cx| this.close_welcome(window, cx)),
                    ),
                )
            })
            .when(w.step > 0, |d| {
                d.child(
                    button("welcome-back", "Back", false, &self.theme, &c).on_click(
                        cx.listener(|this, _, window, cx| this.welcome_step(-1, window, cx)),
                    ),
                )
            })
            .child(
                button(
                    "welcome-next",
                    match step {
                        Step::Hello => "Get Started",
                        Step::Shortcuts => "Start Using Motty",
                        _ => "Continue",
                    },
                    true,
                    &self.theme,
                    &c,
                )
                .on_click(cx.listener(|this, _, window, cx| this.welcome_step(1, window, cx))),
            );

        let card = picker::card(WIDTH, &c)
            .id("welcome")
            .key_context("Welcome")
            .track_focus(&w.focus)
            .on_key_down(cx.listener(Self::on_welcome_key))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .h(px(BODY_HEIGHT))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(body),
            )
            .child(footer);

        // Unlike the palette, a click outside doesn't dismiss it.
        Some(
            div()
                .id("welcome-scrim")
                .absolute()
                .inset_0()
                .px(px(16.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .bg(hsla(Rgba::rgb(0).with_alpha(0.25)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(card),
        )
    }

    fn welcome_hello(&self, w: &Welcome, c: &Colors) -> AnyElement {
        let feature = |title: &'static str, text: &'static str| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_color(c.text)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(div().text_color(c.placeholder).child(text))
        };
        div()
            .size_full()
            .px(px(PAD + 24.))
            .pt(px(26.))
            .flex()
            .flex_col()
            .items_center()
            .child(img(w.icon.clone()).size(px(96.)))
            .child(
                div()
                    .mt(px(10.))
                    .text_size(px(22.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(c.text)
                    .child("Welcome to Motty"),
            )
            .child(
                div()
                    .mt(px(4.))
                    .text_color(c.placeholder)
                    .child("A fast, native terminal that works alongside your coding agents."),
            )
            .child(
                div()
                    .mt(px(24.))
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(feature(
                        "Tabs, splits and session recovery",
                        "Windows, panes and scrollback come back the way you left them.",
                    ))
                    .child(feature(
                        "Made for agents",
                        "Claude Code, Codex and others can read and drive your panes.",
                    ))
                    .child(feature(
                        "Otty-compatible",
                        "Reads .ottytheme files and the keys of Otty's config.toml.",
                    )),
            )
            .into_any_element()
    }

    fn welcome_launch(&self, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let restore = self.config.restore_session;
        let option = |id: &'static str, title: &'static str, text: &'static str, on: bool| {
            div()
                .id(id)
                .p(px(14.))
                .flex()
                .gap(px(12.))
                .rounded(px(10.))
                .border(px(if on { 2. } else { 1. }))
                .border_color(if on { accent(&self.theme) } else { c.border })
                .when(!on, |d| d.m(px(1.)))
                .child(radio(on, &self.theme, c))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_color(c.text)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(div().text_color(c.placeholder).child(text)),
                )
        };
        step_body(
            "When Motty starts",
            "What a new launch opens. This is on-launch in config.toml.",
            c,
        )
        .child(
            div()
                .mt(px(18.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    option(
                        "launch-restore",
                        "Restore my last session",
                        "Reopen your windows, tabs and splits, with their folders and scrollback.",
                        restore,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.welcome_save(&[("on-launch", "restore_session")], cx)
                    })),
                )
                .child(
                    option(
                        "launch-new",
                        "Start with a new window",
                        "Begin fresh each time. After a crash, Motty still restores.",
                        !restore,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.welcome_save(&[("on-launch", "new_window")], cx)
                    })),
                ),
        )
        .children(self.welcome_error())
        .into_any_element()
    }

    fn welcome_theme(&self, w: &Welcome, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let (light, dark) = (
            self.config.theme.as_deref(),
            self.config.theme_dark.as_deref(),
        );
        let automatic = matches!(
            (light, dark),
            (None, None) | (Some(AUTO_LIGHT), Some(AUTO_DARK))
        );
        let current = (!automatic)
            .then(|| self.config.theme_name(self.mode))
            .flatten();
        let auto_tile = {
            let pick = |n: &str| w.themes.iter().find(|t| t.name == n);
            let preview = match (pick(AUTO_LIGHT), pick(AUTO_DARK)) {
                (Some(l), Some(d)) => div()
                    .size_full()
                    .flex()
                    .child(self.preview(l).w_1_2().h_full())
                    .child(self.preview(d).w_1_2().h_full())
                    .into_any_element(),
                _ => div().into_any_element(),
            };
            swatch(
                "theme-auto",
                "Match System",
                preview,
                automatic,
                &self.theme,
                c,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.welcome_save(&[("theme", AUTO_LIGHT), ("theme-dark", AUTO_DARK)], cx)
            }))
        };
        let tiles = w.themes.iter().enumerate().map(|(ix, t)| {
            let selected = current.is_some_and(|n| n.eq_ignore_ascii_case(&t.name));
            let name = t.name.clone();
            swatch(
                ("theme", ix),
                &t.name,
                self.preview(t).size_full().into_any_element(),
                selected,
                &self.theme,
                c,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.welcome_save(
                    &[("theme", name.as_str()), ("theme-dark", name.as_str())],
                    cx,
                )
            }))
        });
        step_body(
            "Pick a theme",
            "It applies right away. Themes in ~/.config/otty/themes are listed too.",
            c,
        )
        .children(self.welcome_error())
        .child(
            div()
                .id("welcome-themes")
                .mt(px(16.))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&w.scroll)
                .child(
                    div()
                        .pb(px(PAD))
                        .flex()
                        .flex_wrap()
                        .gap(px(12.))
                        .child(auto_tile)
                        .children(tiles),
                ),
        )
        .into_any_element()
    }

    /// A theme's terminal in miniature: a prompt and its colors.
    fn preview(&self, t: &ThemeChoice) -> gpui::Div {
        div()
            .p(px(9.))
            .flex()
            .flex_col()
            .justify_between()
            .bg(hsla(t.background))
            .text_color(hsla(t.foreground))
            .font_family(self.mono_font.clone())
            .text_size(px(11.))
            .child(
                div()
                    .flex()
                    .gap(px(5.))
                    .child(div().text_color(hsla(t.accents[2])).child("~"))
                    .child(if t.mode == Mode::Dark {
                        "ls -la"
                    } else {
                        "git log"
                    }),
            )
            .child(
                div().flex().gap(px(4.)).children(
                    t.accents
                        .iter()
                        .map(|a| div().size(px(9.)).rounded_full().bg(hsla(*a))),
                ),
            )
    }

    fn welcome_agents(&self, w: &Welcome, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let cli = cli_command();
        let lines = [
            ("Claude Code", format!("claude mcp add motty -- {cli} mcp")),
            ("Codex", format!("codex mcp add motty -- {cli} mcp")),
        ];
        let rows = lines.into_iter().enumerate().map(|(ix, (agent, line))| {
            let copied = w.copied == Some(ix);
            let text = line.clone();
            div()
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(div().text_size(px(11.5)).text_color(c.header).child(agent))
                .child(
                    div()
                        .min_h(px(34.))
                        .py(px(4.))
                        .pl(px(12.))
                        .pr(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .rounded(px(8.))
                        .bg(c.selected)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family(self.mono_font.clone())
                                .text_size(px(11.))
                                .text_color(c.text)
                                .child(line),
                        )
                        .child(
                            button(
                                ("copy", ix),
                                if copied { "Copied" } else { "Copy" },
                                false,
                                &self.theme,
                                c,
                            )
                            .h(px(26.))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                    if let Some(w) = this.welcome.as_mut() {
                                        w.copied = Some(ix);
                                    }
                                    cx.notify();
                                },
                            )),
                        ),
                )
        });
        let installed = cli == "motty";
        let (status, status_color) = match &w.install {
            Install::Idle if installed => {
                ("Installed at /usr/local/bin/motty.".into(), c.placeholder)
            }
            Install::Idle => (
                "Shells in Motty already have it; install it for other apps and terminals.".into(),
                c.placeholder,
            ),
            Install::Running => ("Installing…".into(), c.placeholder),
            Install::Done(s) => (s.clone(), c.placeholder),
            Install::Failed(s) => (s.clone(), hsla(Rgba::rgb(0xe5484d))),
        };
        let can_install = super::cli_dir().is_some() && !installed && w.install != Install::Running;
        step_body(
            "Bring your agents",
            "Give your agent Motty's MCP server to list panes, run commands and read output:",
            c,
        )
        .child(
            div()
                .mt(px(14.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .children(rows),
        )
        .child(
            div()
                .mt(px(18.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_color(c.text)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("The motty command"),
                )
                .child(
                    div().text_color(c.placeholder).child(
                        "Opens tabs and splits, sends text and waits on commands from scripts.",
                    ),
                )
                .child(
                    div()
                        .mt(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .when(can_install, |d| {
                            d.child(
                                button("install-cli", "Install Command", false, &self.theme, c)
                                    .ml_0()
                                    .on_click(cx.listener(|this, _, _, cx| this.install_cli(cx))),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.))
                                .text_color(status_color)
                                .child(SharedString::from(status)),
                        ),
                ),
        )
        .into_any_element()
    }

    fn install_cli(&mut self, cx: &mut Context<Self>) {
        let Some(cli) = super::cli_dir().map(|d| d.join("motty")) else {
            return;
        };
        if let Some(w) = self.welcome.as_mut() {
            w.install = Install::Running;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { install_cli(&cli) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(w) = this.welcome.as_mut() {
                    w.install = match result {
                        Ok(s) => Install::Done(s),
                        Err(s) => Install::Failed(s),
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn welcome_shortcuts(
        &self,
        w: &Welcome,
        c: &Colors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shortcuts: [(&str, Box<dyn Action>); 9] = [
            ("Command Palette", Box::new(CommandPalette)),
            ("Open Quickly", Box::new(OpenQuickly)),
            ("New Tab", Box::new(NewTab)),
            ("Split Right", Box::new(SplitRight)),
            ("Split Down", Box::new(SplitDown)),
            ("Find", Box::new(Find)),
            ("Jump To a Command", Box::new(JumpTo)),
            ("Details Panel", Box::new(ToggleDetailsPanel)),
            ("Tabs Panel", Box::new(ToggleTabsPanel)),
        ];
        let target = w.target.as_ref().map(|t| t.focus_handle(cx));
        let rows = shortcuts.into_iter().map(|(title, action)| {
            let caps = target
                .as_ref()
                .and_then(|f| window.highest_precedence_binding_for_action_in(&*action, f))
                .and_then(|b| b.keystrokes().first().map(|k| keycaps(k.inner())))
                .unwrap_or_default();
            div()
                .h(px(29.))
                .flex()
                .items_center()
                .border_b(px(1.))
                .border_color(c.separator)
                .child(div().flex_1().text_color(c.text).child(title))
                .child(
                    div()
                        .flex()
                        .gap(px(4.))
                        .children(caps.into_iter().map(|cap| {
                            div()
                                .min_w(px(21.))
                                .h(px(20.))
                                .px(px(4.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.))
                                .border(px(0.5))
                                .border_color(c.keycap_border)
                                .bg(c.keycap)
                                .text_size(px(11.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(c.keycap_text)
                                .child(cap)
                        })),
                )
        });
        step_body(
            "A few shortcuts",
            "The rest are in the command palette. Rebind any with keybind in config.toml.",
            c,
        )
        .child(div().mt(px(12.)).flex().flex_col().children(rows))
        .into_any_element()
    }

    fn welcome_error(&self) -> Option<impl IntoElement> {
        let error = self.welcome.as_ref()?.error.clone()?;
        Some(
            div()
                .mt(px(10.))
                .text_size(px(12.))
                .text_color(hsla(Rgba::rgb(0xe5484d)))
                .child(error),
        )
    }
}

/// A step's title and lead paragraph, with room below for the rest.
fn step_body(title: &'static str, text: &'static str, c: &Colors) -> gpui::Div {
    div()
        .size_full()
        .px(px(PAD))
        .pt(px(PAD))
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(19.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c.text)
                .child(title),
        )
        .child(div().mt(px(6.)).text_color(c.placeholder).child(text))
}

fn accent(theme: &motty_theme::Theme) -> Hsla {
    hsla(Rgba::rgb(if theme.meta.mode == Mode::Dark {
        0x0a84ff
    } else {
        0x007aff
    }))
}

fn button(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    primary: bool,
    theme: &motty_theme::Theme,
    c: &Colors,
) -> gpui::Stateful<gpui::Div> {
    let (bg, fg) = if primary {
        (accent(theme), hsla(Rgba::rgb(0xffffff)))
    } else {
        (c.selected, c.text)
    };
    let hover = Hsla {
        l: (bg.l + if primary { 0.06 } else { 0.04 }).min(1.),
        a: bg.a + if primary { 0. } else { 0.04 },
        ..bg
    };
    div()
        .id(id.into())
        .h(px(30.))
        .ml(px(8.))
        .px(px(14.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .bg(bg)
        .hover(move |s| s.bg(hover))
        .text_color(fg)
        .font_weight(gpui::FontWeight::MEDIUM)
        .child(label)
}

fn radio(on: bool, theme: &motty_theme::Theme, c: &Colors) -> impl IntoElement {
    div()
        .mt(px(1.))
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border(px(if on { 5. } else { 1. }))
        .border_color(if on { accent(theme) } else { c.border })
        .when(on, |d| d.bg(hsla(Rgba::rgb(0xffffff))))
}

fn swatch(
    id: impl Into<gpui::ElementId>,
    name: &str,
    preview: AnyElement,
    selected: bool,
    theme: &motty_theme::Theme,
    c: &Colors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .w(px(SWATCH_WIDTH))
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .h(px(SWATCH_PREVIEW))
                .rounded(px(9.))
                .overflow_hidden()
                .border(px(if selected { 2. } else { 1. }))
                .border_color(if selected { accent(theme) } else { c.border })
                .child(preview),
        )
        .child(
            div()
                .px(px(2.))
                .truncate()
                .text_size(px(12.))
                .text_color(if selected { c.text } else { c.placeholder })
                .when(selected, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                .child(SharedString::from(name.to_owned())),
        )
}
