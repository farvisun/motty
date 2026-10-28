mod actions;
mod fonts;
mod terminal_view;
mod workspace;

use gpui::{App, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowOptions, point, prelude::*, px, size};
use gpui_platform::application;
use motty_theme::{Config, Material};

use crate::workspace::{Workspace, mode_for};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("MOTTY_LOG").unwrap_or_else(|_| "warn".into()))
        .init();

    application().run(|cx: &mut App| {
        fonts::load(cx);
        actions::bind_keys(cx);
        cx.on_action(|_: &actions::Quit, cx| cx.quit());
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let config = Config::load();
        if let Err(e) = open_window(config, command_arg(), cx) {
            eprintln!("motty: {e:#}");
            cx.quit();
        }
        cx.activate(true);
    });
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

/// `--extra-tabs N` opens N more shell tabs (used by the parity harness).
fn extra_tabs_arg() -> usize {
    let args: Vec<String> = std::env::args().take_while(|a| a != "-e").collect();
    args.iter().position(|a| a == "--extra-tabs").and_then(|i| args.get(i + 1)?.parse().ok()).unwrap_or(0)
}

/// `Motty -e program [args...]` runs a command instead of the login shell.
fn command_arg() -> Option<(String, Vec<String>)> {
    let mut args = std::env::args().skip_while(|a| a != "-e").skip(1);
    let program = args.next()?;
    Some((program, args.collect()))
}

fn open_window(config: Config, command: Option<(String, Vec<String>)>, cx: &mut App) -> anyhow::Result<()> {
    let mode = mode_for(cx.window_appearance());
    let material = config.resolve_theme(mode).theme.window.material;
    let bounds = bounds_arg(cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Motty".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(9.), px(9.))),
        }),
        window_background: if material == Material::None {
            WindowBackgroundAppearance::Opaque
        } else {
            WindowBackgroundAppearance::Blurred
        },
        app_id: Some("dev.motty.Motty".into()),
        window_min_size: Some(size(px(360.), px(200.))),
        ..Default::default()
    };
    // Launched from a shell, start there; from Finder the cwd is `/`.
    let cwd = std::env::current_dir().ok().filter(|d| d.parent().is_some());
    let (session, events) = workspace::spawn_shell(&config, command, cwd)?;
    let extra_tabs = extra_tabs_arg();
    cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let mut workspace = Workspace::new(config, session, events, window, cx);
            for _ in 0..extra_tabs {
                workspace.open_tab(window, cx);
            }
            workspace
        })
    })?;
    Ok(())
}
