mod actions;
mod folders;
mod fonts;
mod ipc;
mod materials;
mod notifications;
mod panes;
mod recovery;
mod shell_integration;
mod terminal_view;
mod titlebar;
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

    application().run(|cx: &mut App| {
        fonts::load(cx);
        ipc::start(cx);
        cx.on_action(|_: &actions::Quit, cx| cx.quit());
        cx.on_action(|_: &actions::NewWindow, cx| {
            if let Err(e) = open_window(load_config(), None, None, cx) {
                tracing::error!("failed to open a window: {e:#}");
            }
        });
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        recovery::start(cx);
        let config = load_config();
        for w in actions::bind_keys(cx, &config.keybinds) {
            tracing::warn!("{w}");
        }
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
        cx.activate(true);
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
    // Launched from a shell, start there; from Finder the cwd is `/`.
    let cwd = std::env::current_dir()
        .ok()
        .filter(|d| d.parent().is_some());
    let extra_tabs = extra_tabs_arg();
    let split = split_arg();
    let window = cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let mut workspace = Workspace::new(config, command.clone(), cwd, restore, window, cx);
            for _ in 0..extra_tabs {
                workspace.open_tab(window, cx);
            }
            if let Some(direction) = split {
                workspace.split(direction, command, window, cx);
            }
            workspace
        })
    })?;
    if !window.update(cx, |workspace, _, _| workspace.has_tabs())? {
        anyhow::bail!("failed to start the shell");
    }
    Ok(())
}
