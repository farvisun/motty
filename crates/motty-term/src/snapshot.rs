//! Scrollback as bytes a terminal can replay: text with SGR for colors and
//! attributes, wrapped rows left unbroken so they reflow at a new width.
//! Session recovery saves this on quit and parses it back in on launch.

use std::fmt::Write;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

/// The primary screen's lines above the cursor's (the live prompt), at most
/// `max_lines` of them. `None` while a full-screen program owns the screen.
pub fn serialize<T>(term: &Term<T>, max_lines: usize) -> Option<Vec<u8>> {
    if term.mode().contains(TermMode::ALT_SCREEN) {
        return None;
    }
    let grid = term.grid();
    let end = grid.cursor.point.line;
    let start = Line((end.0 - max_lines as i32).max(grid.topmost_line().0));
    let mut out = String::new();
    let mut style = String::new();
    let mut line = start;
    while line < end {
        let row = &grid[line];
        let wraps = row[grid.last_column()].flags.contains(Flags::WRAPLINE);
        // Trailing blanks are dropped unless the row wraps on.
        let mut len = grid.columns();
        if !wraps {
            while len > 0 && is_blank(&row[Column(len - 1)]) {
                len -= 1;
            }
        }
        for col in 0..len {
            let cell = &row[Column(col)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let sgr = sgr(cell);
            if sgr != style {
                out.push_str(if sgr.is_empty() { "\x1b[0m" } else { &sgr });
                style = sgr;
            }
            out.push(cell.c);
            for &c in cell.zerowidth().unwrap_or_default() {
                out.push(c);
            }
        }
        if !wraps {
            if !style.is_empty() {
                out.push_str("\x1b[0m");
                style.clear();
            }
            out.push_str("\r\n");
        }
        line += 1;
    }
    if !style.is_empty() {
        out.push_str("\x1b[0m");
    }
    Some(out.into_bytes())
}

fn is_blank(cell: &Cell) -> bool {
    cell.c == ' '
        && cell.bg == Color::Named(NamedColor::Background)
        && !cell.flags.intersects(Flags::INVERSE | Flags::UNDERLINE)
}

/// The SGR sequence selecting the cell's style, empty for the default one.
fn sgr(cell: &Cell) -> String {
    let mut params = String::new();
    let flags = [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::UNDERLINE, "4"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ];
    for (flag, code) in flags {
        if cell.flags.contains(flag) {
            let _ = write!(params, ";{code}");
        }
    }
    color(&mut params, cell.fg, 30, 90, 38);
    color(&mut params, cell.bg, 40, 100, 48);
    if params.is_empty() {
        String::new()
    } else {
        format!("\x1b[0{params}m")
    }
}

fn color(out: &mut String, color: Color, normal: u8, bright: u8, extended: u8) {
    match color {
        Color::Named(n) if (n as usize) < 8 => {
            let _ = write!(out, ";{}", normal + n as u8);
        }
        Color::Named(n) if (n as usize) < 16 => {
            let _ = write!(out, ";{}", bright + n as u8 - 8);
        }
        Color::Named(n) => {
            // Dim variants come back through the DIM flag.
            let dims = NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize;
            if dims.contains(&(n as usize)) {
                let _ = write!(
                    out,
                    ";{}",
                    normal + (n as usize - NamedColor::DimBlack as usize) as u8
                );
            }
        }
        Color::Indexed(i) => {
            let _ = write!(out, ";{extended};5;{i}");
        }
        Color::Spec(rgb) => {
            let _ = write!(out, ";{extended};2;{};{};{}", rgb.r, rgb.g, rgb.b);
        }
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::{Config, test::TermSize};
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;

    fn term(cols: usize, text: &[u8]) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &TermSize::new(cols, 5), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, text);
        term
    }

    #[test]
    fn round_trips_styles_and_reflows_wrapped_rows() {
        let src = term(
            10,
            b"plain\r\n\x1b[1;31mred\x1b[0m \x1b[38;2;1;2;3mrgb\x1b[0m\r\n0123456789abc\r\n$ ",
        );
        let bytes = serialize(&src, 100).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(
            text.starts_with("plain\r\n\x1b[0;1;31mred\x1b[0m \x1b[0;38;2;1;2;3mrgb\x1b[0m\r\n")
        );
        // The wrapped row comes back whole, and the prompt line is left out.
        assert!(text.ends_with("0123456789abc\r\n"));
        let wide = term(20, &bytes);
        assert_eq!(wide.grid()[Line(2)][Column(12)].c, 'c');
        assert_eq!(
            wide.grid()[Line(1)][Column(0)].fg,
            Color::Named(NamedColor::Red)
        );
    }
}
