mod actions;
mod agents;
mod dock;
mod dock_click;
mod editor;
mod folders;
mod fonts;
mod history;
mod ipc;
mod materials;
mod menus;
mod native_menu;
mod notifications;
mod panes;
mod pasteboard;
mod recovery;
mod secure_input;
mod settings;
mod shell_integration;
mod terminal_view;
mod titlebar;
mod window_menu;
mod workspace;

use gpui::{
    App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, prelude::*, px, size,
};
use gpui_platform::application;
use motty_theme::Config;

use crate::workspace::{Workspace, mode_for};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("MOTTY_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let app = application().with_assets(settings::Assets);
    // A Dock click with no window open opens one.
    // A Dock click opens a window when there's none (with windows open,
    // `dock_click` goes to the next tab that failed).
    app.on_reopen(|cx| {
        if cx
            .windows()
            .iter()
            .any(|w| w.downcast::<Workspace>().is_some())
        {
            return workspace::show_next_failing(cx);
        }
        if let Err(e) = open_window(load_config(), None, None, cx) {
            tracing::error!("failed to open a window: {e:#}");
        }
    });
    app.run(|cx: &mut App| {
        fonts::load(cx);
        ipc::start(cx);
        dock_click::install(cx);
        cx.on_action(|_: &actions::Quit, cx| cx.quit());
        cx.on_action(|_: &actions::QuitKeepWindows, cx| {
            recovery::keep_windows();
            cx.quit()
        });
        cx.on_action(|_: &actions::MinimizeAll, _| minimize_all());
        cx.on_action(|_: &actions::ClearRecentFiles, cx| {
            folders::clear_recent_files();
            menus::refresh(cx);
        });
        cx.on_action(|_: &actions::OpenSettings, cx| settings::open(None, cx));
        cx.on_action(|_: &actions::About, _| menus::show_about());
        cx.on_action(|a: &actions::CopyText, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(a.text.clone()))
        });
        cx.on_action(|a: &actions::OpenUrl, cx| cx.open_url(&a.url));
        cx.on_action(|_: &actions::ToggleSecureKeyboardEntry, cx| secure_input::toggle(cx));
        workspace::cursor_hiding(&load_config(), cx);
        cx.on_action(|_: &actions::HideApp, cx| cx.hide());
        cx.on_action(|_: &actions::HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &actions::ShowAll, cx| cx.unhide_other_apps());
        cx.on_action(|_: &actions::OpenHelp, cx| {
            cx.open_url("https://github.com/farvisun/motty#readme")
        });
        cx.on_action(|_: &actions::NewWindow, cx| {
            if let Err(e) = open_window(load_config(), None, None, cx) {
                tracing::error!("failed to open a window: {e:#}");
            }
        });
        // Otty stays in the Dock after its last window closes unless
        // `quit-after-last-window-closed`; a Dock click opens a window.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() && load_config().quit_after_last_window {
                cx.quit();
            }
        })
        .detach();

        recovery::start(cx);
        let config = load_config();
        for w in actions::bind_keys(cx, &config.keybinds) {
            tracing::warn!("{w}");
        }
        menus::set(cx);
        let restored = recovery::to_restore(config.restore_session);
        let windows = restored.map(|s| s.windows).unwrap_or_default();
        let mut opened = false;
        for state in &windows {
            match open_window(config.clone(), None, Some(state), cx) {
                Ok(()) => opened = true,
                Err(e) => tracing::error!("couldn't restore a window: {e:#}"),
            }
        }
        if !opened && let Err(e) = open_window(config, command_arg(), None, cx) {
            eprintln!("motty: {e:#}");
            cx.quit();
        }
        if workspace::first_run()
            && let Some(window) = cx.windows().first().and_then(|w| w.downcast::<Workspace>())
        {
            let _ = window.update(cx, |workspace, window, cx| {
                workspace.show_welcome(window, cx)
            });
        }
        // Test runs set MOTTY_NO_ACTIVATE so the window doesn't take the
        // keyboard from whatever the user is typing into.
        if std::env::var_os("MOTTY_NO_ACTIVATE").is_none() {
            cx.activate(true);
        }
    });
}

/// The config files with command-line overrides applied: `--theme NAME`
/// replaces the configured theme for both appearances.
pub fn load_config() -> Config {
    let mut config = Config::load();
    if let Some(name) = flag_value("--theme") {
        config.theme = Some(name.clone());
        config.theme_dark = Some(name);
    }
    config
}

/// `--bounds x,y,w,h` places the first window exactly (used by the parity harness).
fn bounds_arg(cx: &App) -> Bounds<gpui::Pixels> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == "--bounds"
            && let Some(v) = args.next()
        {
            let n: Vec<f32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            if let [x, y, w, h] = n[..] {
                return Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
            }
        }
    }
    Bounds::centered(None, size(px(1200.), px(800.)), cx)
}

/// The value after `flag`, among the arguments before `-e`.
fn flag_value(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().take_while(|a| a != "-e").collect();
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

/// `--extra-tabs N` opens N more shell tabs (used by the parity harness).
fn extra_tabs_arg() -> usize {
    flag_value("--extra-tabs")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// `--split right|down|left|up` splits the first pane, running the same `-e`
/// command in the new one (used by the parity harness).
fn split_arg() -> Option<panes::Direction> {
    Some(match flag_value("--split")?.as_str() {
        "right" => panes::Direction::Right,
        "left" => panes::Direction::Left,
        "down" => panes::Direction::Down,
        "up" => panes::Direction::Up,
        _ => return None,
    })
}

/// `Motty -e program [args...]` runs a command instead of the login shell.
fn command_arg() -> Option<(String, Vec<String>)> {
    let mut args = std::env::args().skip_while(|a| a != "-e").skip(1);
    let program = args.next()?;
    Some((program, args.collect()))
}

fn open_window(
    config: Config,
    command: Option<(String, Vec<String>)>,
    restore: Option<&recovery::WindowState>,
    cx: &mut App,
) -> anyhow::Result<()> {
    open_window_inner(config, command, restore, None, None, cx)?;
    Ok(())
}

/// A window whose first shell starts in `cwd` and runs `line`.
pub(crate) fn open_window_running(
    config: Config,
    cwd: Option<std::path::PathBuf>,
    line: &str,
    cx: &mut App,
) -> anyhow::Result<()> {
    let window = open_window_inner(config, None, None, None, cwd, cx)?;
    if line.is_empty() {
        return Ok(());
    }
    let line = format!("{line}\r");
    window.update(cx, |workspace, _, cx| {
        if let Some(view) = workspace.focused_pane() {
            view.update(cx, |v, cx| v.write_input(&line, cx));
        }
    })?;
    Ok(())
}

/// A window for a tab moved out of another one (Move Tab to New Window).
pub(crate) fn open_window_with_tab(
    config: Config,
    tab: workspace::MovedTab,
    cx: &mut App,
) -> anyhow::Result<()> {
    open_window_inner(config, None, None, Some(tab), None, cx)?;
    Ok(())
}

fn open_window_inner(
    config: Config,
    command: Option<(String, Vec<String>)>,
    restore: Option<&recovery::WindowState>,
    moved: Option<workspace::MovedTab>,
    cwd: Option<std::path::PathBuf>,
    cx: &mut App,
) -> anyhow::Result<gpui::WindowHandle<Workspace>> {
    let moving = moved.is_some();
    let mode = mode_for(cx.window_appearance());
    let theme = config.resolve_theme(mode).theme;
    let bounds = match restore {
        Some(state) => {
            let [x, y, w, h] = state.bounds;
            let saved = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
            // A window left on a display that's gone comes back centered.
            if cx.displays().iter().any(|d| d.bounds().intersects(&saved)) {
                saved
            } else {
                Bounds::centered(None, saved.size, cx)
            }
        }
        None => bounds_arg(cx),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Motty".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(9.), px(9.))),
        }),
        window_background: workspace::background_appearance(&theme),
        app_id: Some("dev.motty.Motty".into()),
        window_min_size: Some(size(px(360.), px(200.))),
        ..Default::default()
    };
    // `window-working-directory`: a folder, or home; launched from a
    // shell, home means where it was launched (from Finder the cwd is `/`).
    let cwd = cwd.or_else(
        || match config.working_dir(motty_theme::NewTerminal::Window) {
            motty_theme::WorkingDir::Path(path) => Some(path),
            _ => std::env::current_dir()
                .ok()
                .filter(|d| d.parent().is_some()),
        },
    );
    // The harness's extra tabs and split are for the first window only.
    let extra_tabs = if moving { 0 } else { extra_tabs_arg() };
    let split = if moving { None } else { split_arg() };
    let window = cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let mut workspace =
                Workspace::new(config, command.clone(), cwd, restore, moved, window, cx);
            for _ in 0..extra_tabs {
                workspace.open_tab(window, cx);
            }
            if let Some(direction) = split {
                workspace.split(direction, command, window, cx);
            }
            workspace
        })
    })?;
    // Test runs: in front so it draws, without taking the keyboard.
    if std::env::var_os("MOTTY_NO_ACTIVATE").is_some() {
        window.update(cx, |_, window, _| materials::show_inactive(window))?;
    }
    if !window.update(cx, |workspace, _, _| workspace.has_tabs())? {
        anyhow::bail!("failed to start the shell");
    }
    Ok(window)
}

/// Window › Minimize All.
fn minimize_all() {
    #[cfg(target_os = "macos")]
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        objc2_app_kit::NSApplication::sharedApplication(mtm).miniaturizeAll(None);
    }
}
