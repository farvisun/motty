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
    let size = GridSize { cols: 80, rows: 24, cell_width: 8., cell_height: 16. };
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
