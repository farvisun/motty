//! How Motty identifies itself and which VT requests it honours: the
//! queries alacritty's handler drops or answers as alacritty (DA1, DA2,
//! XTVERSION, XTWINOPS 14t/16t/21t, ENQ), answered on the PTY thread in
//! stream order so a program reading up to the DA1 reply sees them all.

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::Params;

/// The name XTVERSION reports (Settings › Shell › Terminal Identity).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Identity {
    #[default]
    Motty,
    /// For programs that only turn features on for kitty or Ghostty.
    Kitty,
    Ghostty,
    /// Sent as written.
    Custom(String),
}

impl Identity {
    /// `motty`, `kitty`, `ghostty` (any case), or text of your own.
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "" | "motty" => Self::Motty,
            "kitty" => Self::Kitty,
            "ghostty" => Self::Ghostty,
            _ => Self::Custom(text.trim().to_owned()),
        }
    }

    /// The name and version, as each terminal spells them.
    pub fn xtversion(&self) -> String {
        match self {
            Self::Motty => format!("motty({})", env!("CARGO_PKG_VERSION")),
            Self::Kitty => "kitty(0.42.0)".into(),
            Self::Ghostty => "ghostty 1.2.0".into(),
            Self::Custom(text) => text.clone(),
        }
    }
}

/// OSC 4/10/11 color query replies: none, `rgb:rr/gg/bb` or
/// `rgb:rrrr/gggg/bbbb`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorReport {
    None,
    Bits8,
    #[default]
    Bits16,
}

impl ColorReport {
    /// Rewrites alacritty's 16-bit reply; `None` drops it.
    pub(crate) fn apply(self, reply: String) -> Option<String> {
        match self {
            Self::None => None,
            Self::Bits16 => Some(reply),
            Self::Bits8 => {
                let Some(at) = reply.find("rgb:") else {
                    return Some(reply);
                };
                let start = at + 4;
                // `rrrr/gggg/bbbb`: each 8-bit value written twice.
                let Some(channels) = reply.get(start..start + 14) else {
                    return Some(reply);
                };
                let short: Vec<&str> = channels
                    .split('/')
                    .map(|c| c.get(..2).unwrap_or(c))
                    .collect();
                Some(format!(
                    "{}{}{}",
                    &reply[..start],
                    short.join("/"),
                    &reply[start + 14..]
                ))
            }
        }
    }
}

/// The `Terminal Identity & VT` config keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VtOptions {
    /// Programs may turn on the kitty keyboard protocol.
    pub kitty_keyboard: bool,
    /// DECKPAM is honoured; off, the keypad always types digits.
    pub keypad_app_allowed: bool,
    /// KAM (`CSI 2 h`) may lock the keyboard.
    pub kam_allowed: bool,
    /// The reply to ENQ (0x05); empty sends nothing.
    pub enquiry_response: String,
    pub color_report: ColorReport,
    /// Programs may read the title (XTWINOPS 21t).
    pub title_report: bool,
    pub identity: Identity,
}

impl Default for VtOptions {
    fn default() -> Self {
        Self {
            kitty_keyboard: true,
            keypad_app_allowed: true,
            kam_allowed: true,
            enquiry_response: String::new(),
            color_report: ColorReport::Bits16,
            title_report: false,
            identity: Identity::Motty,
        }
    }
}

impl VtOptions {
    /// The mode keys are encoded in: no application keypad unless DECKPAM
    /// is honoured.
    pub fn key_mode(&self, mut mode: TermMode) -> TermMode {
        if !self.keypad_app_allowed {
            mode.remove(TermMode::APP_KEYPAD);
        }
        mode
    }

    /// The program locked the keyboard (KAM) and that's honoured.
    pub fn keyboard_locked(&self, mode: TermMode) -> bool {
        self.kam_allowed && mode.contains(TermMode::KEYBOARD_LOCKED)
    }
}

/// A query answered here rather than by alacritty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Query {
    /// DA1, `CSI c`.
    PrimaryAttributes,
    /// DA2, `CSI > c`.
    SecondaryAttributes,
    /// `CSI > q`.
    Version,
    /// XTWINOPS `CSI 14 t`: the text area in pixels.
    TextAreaPixels,
    /// XTWINOPS `CSI 16 t`: one cell in pixels.
    CellPixels,
    /// XTWINOPS `CSI 21 t`: the title.
    Title,
    /// ENQ.
    Enquiry,
}

/// The query a CSI sequence makes, if it's one of [`Query`]'s.
pub(crate) fn csi_query(params: &Params, intermediates: &[u8], action: char) -> Option<Query> {
    let first = params.iter().next().map_or(0, |p| p[0]);
    match (action, intermediates) {
        ('c', []) if first == 0 => Some(Query::PrimaryAttributes),
        ('c', [b'>']) if first == 0 => Some(Query::SecondaryAttributes),
        ('q', [b'>']) if first == 0 => Some(Query::Version),
        ('t', []) => match first {
            14 => Some(Query::TextAreaPixels),
            16 => Some(Query::CellPixels),
            21 => Some(Query::Title),
            _ => None,
        },
        _ => None,
    }
}

/// `1.2.3` as `10203`, as xterm and alacritty encode it for DA2.
fn version_number(version: &str) -> u32 {
    let version = version.split(['-', '+']).next().unwrap_or_default();
    version
        .split('.')
        .take(3)
        .map(|part| part.parse::<u32>().unwrap_or(0))
        .chain(std::iter::repeat(0))
        .take(3)
        .fold(0, |acc, n| acc * 100 + n.min(99))
}

/// The reply to `query`, or `None` when there's nothing to say.
pub(crate) fn reply(
    query: Query,
    options: &VtOptions,
    window: WindowSize,
    title: Option<&str>,
) -> Option<String> {
    Some(match query {
        // VT220-class (62) with Sixel graphics (4) and ANSI color (22).
        Query::PrimaryAttributes => "\x1b[?62;4;22c".into(),
        Query::SecondaryAttributes => {
            format!("\x1b[>0;{};1c", version_number(env!("CARGO_PKG_VERSION")))
        }
        Query::Version => format!("\x1bP>|{}\x1b\\", options.identity.xtversion()),
        Query::TextAreaPixels => format!(
            "\x1b[4;{};{}t",
            window.num_lines as u32 * window.cell_height as u32,
            window.num_cols as u32 * window.cell_width as u32
        ),
        Query::CellPixels => format!("\x1b[6;{};{}t", window.cell_height, window.cell_width),
        Query::Title if options.title_report => {
            let title: String = title
                .unwrap_or_default()
                .chars()
                .filter(|c| !c.is_control())
                .collect();
            format!("\x1b]l{title}\x1b\\")
        }
        Query::Title => return None,
        Query::Enquiry if options.enquiry_response.is_empty() => return None,
        Query::Enquiry => options.enquiry_response.clone(),
    })
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::Term;
    use alacritty_terminal::event::{Event, EventListener};
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;
    use alacritty_terminal::vte::{Parser, Perform};

    use super::*;

    const WINDOW: WindowSize = WindowSize {
        num_lines: 24,
        num_cols: 80,
        cell_width: 9,
        cell_height: 18,
    };

    #[derive(Default)]
    struct Queries(Vec<Query>);

    impl Perform for Queries {
        fn csi_dispatch(&mut self, params: &Params, ints: &[u8], _ignore: bool, action: char) {
            self.0.extend(csi_query(params, ints, action));
        }
    }

    fn queries(bytes: &[u8]) -> Vec<Query> {
        let mut found = Queries::default();
        Parser::new().advance(&mut found, bytes);
        found.0
    }

    fn answer(bytes: &[u8], options: &VtOptions) -> Vec<String> {
        queries(bytes)
            .into_iter()
            .filter_map(|q| reply(q, options, WINDOW, Some("vim\x07 main.rs")))
            .collect()
    }

    #[test]
    fn recognizes_queries() {
        use Query::*;
        assert_eq!(
            queries(b"\x1b[c\x1b[0c\x1b[>c\x1b[>0c\x1b[>q\x1b[14t\x1b[16t\x1b[18t\x1b[21t"),
            [
                PrimaryAttributes,
                PrimaryAttributes,
                SecondaryAttributes,
                SecondaryAttributes,
                Version,
                TextAreaPixels,
                CellPixels,
                Title
            ]
        );
        // DA3, cursor style and other XTWINOPS aren't ours.
        assert!(queries(b"\x1b[=c\x1b[2 q\x1b[22t\x1b[8;1;1t").is_empty());
    }

    #[test]
    fn replies() {
        let options = VtOptions::default();
        let version = env!("CARGO_PKG_VERSION");
        assert_eq!(answer(b"\x1b[c", &options), ["\x1b[?62;4;22c"]);
        assert_eq!(
            answer(b"\x1b[>c", &options),
            [format!("\x1b[>0;{};1c", version_number(version))]
        );
        assert_eq!(
            answer(b"\x1b[>q", &options),
            [format!("\x1bP>|motty({version})\x1b\\")]
        );
        assert_eq!(answer(b"\x1b[14t", &options), ["\x1b[4;432;720t"]);
        assert_eq!(answer(b"\x1b[16t", &options), ["\x1b[6;18;9t"]);
        // The title only when allowed, and without control characters.
        assert!(answer(b"\x1b[21t", &options).is_empty());
        let open = VtOptions {
            title_report: true,
            ..VtOptions::default()
        };
        assert_eq!(answer(b"\x1b[21t", &open), ["\x1b]lvim main.rs\x1b\\"]);
    }

    #[test]
    fn identity_and_version() {
        assert_eq!(version_number("1.0.2"), 10002);
        assert_eq!(version_number("0.26.0-rc1"), 2600);
        assert_eq!(version_number("2.3"), 20300);
        assert_eq!(Identity::parse("Kitty"), Identity::Kitty);
        assert_eq!(Identity::parse("motty"), Identity::Motty);
        assert_eq!(Identity::parse("ghostty"), Identity::Ghostty);
        assert_eq!(
            Identity::parse("WezTerm 20240203"),
            Identity::Custom("WezTerm 20240203".into())
        );
        assert!(Identity::Kitty.xtversion().starts_with("kitty("));
        assert!(Identity::Ghostty.xtversion().starts_with("ghostty "));
        let custom = VtOptions {
            identity: Identity::parse("WezTerm 20240203"),
            ..VtOptions::default()
        };
        assert_eq!(
            answer(b"\x1b[>q", &custom),
            ["\x1bP>|WezTerm 20240203\x1b\\"]
        );
    }

    #[test]
    fn enquiry_and_colors() {
        let options = VtOptions::default();
        assert_eq!(reply(Query::Enquiry, &options, WINDOW, None), None);
        let answerback = VtOptions {
            enquiry_response: "motty".into(),
            ..VtOptions::default()
        };
        assert_eq!(
            reply(Query::Enquiry, &answerback, WINDOW, None).as_deref(),
            Some("motty")
        );
        let full = "\x1b]11;rgb:1e1e/2f2f/a0a0\x1b\\".to_owned();
        assert_eq!(ColorReport::Bits16.apply(full.clone()), Some(full.clone()));
        assert_eq!(
            ColorReport::Bits8.apply(full.clone()).as_deref(),
            Some("\x1b]11;rgb:1e/2f/a0\x1b\\")
        );
        assert_eq!(
            ColorReport::Bits8
                .apply("\x1b]4;1;rgb:ffff/0000/0000\x07".into())
                .as_deref(),
            Some("\x1b]4;1;rgb:ff/00/00\x07")
        );
        assert_eq!(ColorReport::None.apply(full), None);
    }

    std::thread_local! {
        /// The replies of this test's Term.
        static REPLIES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Feeds `bytes` to the Term; returns its replies.
    fn feed(term: &mut Term<Sink>, bytes: &[u8]) -> Vec<String> {
        let mut parser: Processor = Processor::new();
        parser.advance(term, bytes);
        REPLIES.with_borrow_mut(std::mem::take)
    }

    #[derive(Clone, Copy, Default)]
    struct Sink;

    impl EventListener for Sink {
        fn send_event(&self, event: Event) {
            if let Event::PtyWrite(text) = event {
                REPLIES.with_borrow_mut(|r| r.push(text));
            }
        }
    }

    fn term() -> Term<Sink> {
        let size = crate::GridSize {
            cols: 20,
            rows: 4,
            cell_width: 8.,
            cell_height: 16.,
        };
        let config = Config {
            kitty_keyboard: true,
            ..Config::default()
        };
        Term::new(config, &size, Sink)
    }

    #[test]
    fn key_encodings_are_recorded_and_reset() {
        let mut term = term();
        assert_eq!(feed(&mut term, b"\x1b[>11u\x1b[?u"), ["\x1b[?11u"]);
        assert!(term.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES));
        assert_eq!(feed(&mut term, b"\x1b[>4;2m\x1b[?4m"), ["\x1b[>4;2m"]);
        assert!(term.mode().contains(TermMode::MODIFY_OTHER_KEYS_2));
        feed(&mut term, b"\x1b[>4;1m");
        assert!(term.mode().contains(TermMode::MODIFY_OTHER_KEYS_1));
        assert!(!term.mode().contains(TermMode::MODIFY_OTHER_KEYS_2));
        // The alternate screen has a stack of its own; both go.
        feed(&mut term, b"\x1b[?1049h\x1b[>1u");
        term.reset_key_encodings();
        assert!(
            !term
                .mode()
                .intersects(TermMode::KITTY_KEYBOARD_PROTOCOL | TermMode::MODIFY_OTHER_KEYS)
        );
        feed(&mut term, b"\x1b[?1049l");
        assert!(!term.mode().intersects(TermMode::KITTY_KEYBOARD_PROTOCOL));
        assert_eq!(
            feed(&mut term, b"\x1b[?u\x1b[?4m"),
            ["\x1b[?0u", "\x1b[>4;0m"]
        );
        // RIS resets them too, and KAM.
        feed(&mut term, b"\x1b[>1u\x1b[>4;2m\x1b[2h");
        assert!(term.mode().contains(TermMode::KEYBOARD_LOCKED));
        assert_eq!(feed(&mut term, b"\x1b[2$p"), ["\x1b[2;1$y"]);
        feed(&mut term, b"\x1bc");
        assert!(!term.mode().intersects(
            TermMode::KITTY_KEYBOARD_PROTOCOL
                | TermMode::MODIFY_OTHER_KEYS
                | TermMode::KEYBOARD_LOCKED
        ));
        feed(&mut term, b"\x1b[2h\x1b[2l");
        assert!(!term.mode().contains(TermMode::KEYBOARD_LOCKED));
    }

    #[test]
    fn kitty_keyboard_off_ignores_requests() {
        let mut term = Term::new(
            Config::default(),
            &crate::GridSize {
                cols: 20,
                rows: 4,
                cell_width: 8.,
                cell_height: 16.,
            },
            Sink,
        );
        assert!(feed(&mut term, b"\x1b[>1u\x1b[?u").is_empty());
        assert!(!term.mode().intersects(TermMode::KITTY_KEYBOARD_PROTOCOL));
    }

    #[test]
    fn device_status_reports() {
        let mut term = term();
        assert_eq!(feed(&mut term, b"\x1b[5n"), ["\x1b[0n"]);
        assert_eq!(feed(&mut term, b"ab\r\n c\x1b[6n"), ["\x1b[2;3R"]);
        assert_eq!(feed(&mut term, b"\x1b[18t"), ["\x1b[8;4;20t"]);
    }

    #[test]
    fn key_mode_policies() {
        let mode = TermMode::APP_KEYPAD | TermMode::KEYBOARD_LOCKED;
        let strict = VtOptions {
            keypad_app_allowed: false,
            kam_allowed: false,
            ..VtOptions::default()
        };
        assert!(!strict.key_mode(mode).contains(TermMode::APP_KEYPAD));
        assert!(!strict.keyboard_locked(mode));
        let default = VtOptions::default();
        assert!(default.key_mode(mode).contains(TermMode::APP_KEYPAD));
        assert!(default.keyboard_locked(mode));
    }
}
