//! Spawns real processes on a PTY and inspects the resulting grid.

use std::time::{Duration, Instant};

use motty_term::alacritty_terminal::grid::Dimensions;
use motty_term::alacritty_terminal::index::{Column, Line, Point};
use motty_term::{GridSize, Session, SessionConfig};

fn run(cmd: &str) -> Session {
    let config = SessionConfig {
        command: Some(("/bin/sh".into(), vec!["-c".into(), cmd.into()])),
        ..Default::default()
    };
    let size = GridSize {
        cols: 80,
        rows: 24,
        cell_width: 8.,
        cell_height: 16.,
    };
    Session::spawn(config, size).unwrap().0
}

fn line_text(session: &Session, line: i32) -> String {
    let term = session.term.lock();
    let cols = term.columns();
    (0..cols)
        .map(|c| term.grid()[Point::new(Line(line), Column(c))].c)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

fn wait_for(session: &Session, line: i32, expected: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let text = line_text(session, line);
        if text == expected || Instant::now() > deadline {
            return text;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn output_lands_in_grid() {
    let s = run("printf 'hello\\nworld'; sleep 2");
    assert_eq!(wait_for(&s, 0, "hello"), "hello");
    assert_eq!(wait_for(&s, 1, "world"), "world");
}

#[test]
fn input_is_echoed_by_cat() {
    let s = run("cat");
    s.write(b"ping\r".as_slice());
    assert_eq!(wait_for(&s, 0, "ping"), "ping");
}

#[test]
fn prompt_line_has_no_stray_characters() {
    let s = run("printf '\\033]0;title\\007$ '; sleep 2");
    assert_eq!(wait_for(&s, 0, "$"), "$");
}

#[test]
fn foreground_process_is_reported() {
    let s = run("cd /tmp && exec sleep 5");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut name = None;
    while Instant::now() < deadline {
        name = s.foreground_process_name();
        if name.as_deref() == Some("sleep") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(name.as_deref(), Some("sleep"));
    assert_eq!(
        s.foreground_cwd().map(|p| p.canonicalize().unwrap()),
        Some(std::path::Path::new("/tmp").canonicalize().unwrap())
    );
}

#[test]
fn busy_while_a_child_runs() {
    let s = run("sleep 5; true");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !s.is_busy() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(s.is_busy());
    // macOS /bin/sh re-execs bash, so only check a name is found.
    assert!(s.shell_name().is_some());
}

#[test]
fn default_shell_is_named_through_login_and_idle() {
    let size = GridSize {
        cols: 80,
        rows: 24,
        cell_width: 8.,
        cell_height: 16.,
    };
    let s = Session::spawn(SessionConfig::default(), size).unwrap().0;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut name = None;
    while Instant::now() < deadline {
        name = s.shell_name();
        if name.as_deref().is_some_and(|n| n != "login") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let name = name.expect("shell name");
    assert_ne!(name, "login");
    // Wait out shell startup (rc files may run short-lived commands).
    let deadline = Instant::now() + Duration::from_secs(5);
    while s.is_busy() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!s.is_busy(), "{name} at a prompt should be idle");
}
