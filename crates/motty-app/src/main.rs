mod actions;
mod agents;
mod dock;
mod dock_click;
mod editor;
mod file_search;
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
mod preview;
mod recipes;
mod recovery;
mod secure_input;
mod settings;
mod share;
mod shell_integration;
mod tab_icons;
mod terminal_view;
mod titlebar;
mod tooltip;
// Not shown anywhere yet.
mod web_pane;
#[allow(dead_code)]
mod web_view;
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
    // Files opened from Finder (recipes), handled once the app runs.
    let (opened_tx, opened) = async_channel::unbounded::<Vec<String>>();
    app.on_open_urls(move |urls| {
        let _ = opened_tx.send_blocking(urls);
    });
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
        cx.spawn(async move |cx| {
            while let Ok(urls) = opened.recv().await {
                cx.update(|cx| open_urls(urls, cx));
            }
        })
        .detach();
        cx.on_action(|_: &actions::Quit, cx| cx.quit());
        cx.on_action(|_: &actions::QuitKeepWindows, cx| {
            recovery::keep_windows();
            cx.quit()
        });
        cx.on_action(|_: &actions::MinimizeAll, _| minimize_all());
        cx.on_action(|_: &actions::MemoryReport, cx| workspace::memory_report(cx));
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

thread_local! {
    static WINDOW_BOUNDS: std::cell::RefCell<
        std::collections::HashMap<gpui::WindowId, Bounds<gpui::Pixels>>,
    > = Default::default();
}

/// Where a workspace window was when it last drew, for [`cascaded`].
pub(crate) fn note_window_bounds(window: &gpui::Window) {
    let id = window.window_handle().window_id();
    WINDOW_BOUNDS.with(|b| b.borrow_mut().insert(id, window.bounds()));
}

/// A new window steps 28pt down and right of the front one, as Otty's do,
/// while that still fits on its display.
fn cascaded(cx: &mut App) -> Option<Bounds<gpui::Pixels>> {
    const STEP: f32 = 28.;
    let front = cx
        .window_stack()
        .unwrap_or_else(|| cx.windows())
        .into_iter()
        .find_map(|w| w.downcast::<Workspace>())?;
    // Read from the last frame: the front window is often the one asking,
    // and busy.
    let bounds = WINDOW_BOUNDS.with(|b| b.borrow().get(&front.window_id()).copied())?;
    let next = Bounds::new(bounds.origin + point(px(STEP), px(STEP)), bounds.size);
    cx.displays()
        .iter()
        .any(|d| d.bounds().contains(&next.bottom_right()) && d.bounds().contains(&next.origin))
        .then_some(next)
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

/// Finder's Open: a recipe file opens its recipe in the front window
/// (asking before its commands run); other files get an editor.
fn open_urls(urls: Vec<String>, cx: &mut App) {
    let paths: Vec<std::path::PathBuf> = urls
        .iter()
        .filter_map(|u| u.strip_prefix("file://"))
        .map(|p| std::path::PathBuf::from(percent_decode(p)))
        .collect();
    if paths.is_empty() {
        return;
    }
    let front = cx
        .window_stack()
        .into_iter()
        .flatten()
        .chain(cx.windows())
        .find_map(|w| w.downcast::<Workspace>());
    let front = match front {
        Some(w) => w,
        None => match open_window_inner(load_config(), None, None, None, None, cx) {
            Ok(w) => w,
            Err(e) => return tracing::error!("failed to open a window: {e:#}"),
        },
    };
    let _ = front.update(cx, |workspace, window, cx| {
        window.activate_window();
        for path in paths {
            let recipe = path
                .extension()
                .is_some_and(|x| x == recipes::EXTENSION || x == recipes::OTTY_EXTENSION);
            if recipe {
                workspace.open_recipe_path(&path, window, cx);
            } else if path.extension().is_some_and(|x| x == "ottytheme") {
                workspace.preview_import(&path, None, window, cx);
            } else {
                workspace.open_editor(Some(path), window, cx);
            }
        }
    });
}

/// `%20` and the like back to bytes.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A window for a recipe's tabs, which take its first tab's place.
pub(crate) fn open_window_for_recipe(
    config: Config,
    cwd: Option<std::path::PathBuf>,
    cx: &mut App,
) -> anyhow::Result<gpui::WindowHandle<Workspace>> {
    open_window_inner(config, None, None, None, cwd, cx)
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
        None => cascaded(cx).unwrap_or_else(|| bounds_arg(cx)),
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
