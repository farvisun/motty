//! Splits the PTY byte stream around image sequences.
//!
//! vte drops APC strings (`ESC _ … ST`) and alacritty ignores sixel DCS
//! (`ESC P … q … ST`), so those, iTerm2's `OSC 1337 ; File=` family and
//! XTSMGRAPHICS (`CSI ? Pi ; Pa ; Pv S`) are cut out here and everything else
//! goes on to the VT parser untouched. Plain output only costs a `memchr`
//! for ESC plus a look at the two or three bytes after each one, and is
//! handed on in one piece up to the next image sequence.
//!
//! This sits in front of the shell-integration sniffer rather than in it:
//! that runs a second vte parser, which drops APC bytes without telling
//! its `Perform` and caps OSC parameters far below an image's size.

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;

/// Longest sequence kept; anything bigger is read to its end and dropped.
pub const MAX_SEQUENCE: usize = 128 << 20;
/// Longest DCS/CSI parameter run looked at before deciding.
const MAX_PARAMS: usize = 64;

/// The OSC 1337 keys that carry files.
const ITERM_KEYS: [&[u8]; 4] = [
    b"1337;File=",
    b"1337;MultipartFile=",
    b"1337;FilePart=",
    b"1337;FileEnd",
];

/// A complete image-related sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sequence {
    /// APC `G…`: the bytes after `G`.
    Kitty(Vec<u8>),
    /// DCS `P1;P2;P3 q …`: the parameters and the sixel data.
    Sixel { params: Vec<u32>, data: Vec<u8> },
    /// OSC `1337;…`: the bytes after `1337;`.
    Iterm(Vec<u8>),
    /// XTSMGRAPHICS `CSI ? Pi;Pa;Pv S`: its parameters.
    Xtsm(Vec<u32>),
}

/// What the caller does with the bytes [`Scanner::next`] consumed.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Feed the consumed input to the VT parser.
    Pass,
    /// Feed these bytes (held back from earlier input) to the VT parser.
    PassHeld(Vec<u8>),
    /// An image sequence ended.
    Sequence(Sequence),
    /// Part of a sequence, kept; nothing to do yet.
    Pending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Any APC; only `G` ones are kept (vte drops the rest anyway).
    Apc,
    Sixel,
    Iterm,
}

enum Classified {
    /// Ours: a body of `kind` follows the first `header` bytes.
    Body(Kind, usize),
    /// Complete as it stands (XTSMGRAPHICS), `len` bytes long.
    Whole(usize),
    NotOurs,
    NeedMore,
}

#[derive(Default)]
enum State {
    #[default]
    Ground,
    /// The start of an escape that may be ours, cut off by the end of input.
    Prefix(Vec<u8>),
    Body {
        kind: Kind,
        header: Vec<u8>,
        data: Vec<u8>,
        /// Over [`MAX_SEQUENCE`]: read to the end, then dropped.
        overflow: bool,
        /// The last byte was ESC: `\` completes ST.
        esc: bool,
    },
}

#[derive(Default)]
pub struct Scanner {
    state: State,
}

impl Scanner {
    /// Consumes some of `input` (at least one byte when it isn't empty) and
    /// says what to do with it.
    pub fn next(&mut self, input: &[u8]) -> (usize, Step) {
        match &mut self.state {
            State::Ground => self.ground(input),
            State::Prefix(_) => self.prefix(input),
            State::Body { .. } => self.body(input),
        }
    }

    fn ground(&mut self, input: &[u8]) -> (usize, Step) {
        for at in memchr::memchr_iter(ESC, input) {
            // Most escapes are SGR and other CSI: not ours at a glance.
            match input.get(at + 1) {
                Some(b'[') if input.get(at + 2).is_some_and(|&b| b != b'?') => continue,
                Some(b) if !matches!(b, b'_' | b'P' | b']' | b'[') => continue,
                _ => {}
            }
            match classify(&input[at..]) {
                Classified::NotOurs => continue,
                _ if at > 0 => return (at, Step::Pass),
                Classified::NeedMore => {
                    self.state = State::Prefix(input.to_vec());
                    return (input.len(), Step::Pending);
                }
                Classified::Whole(len) => {
                    return (len, Step::Sequence(whole(&input[..len])));
                }
                Classified::Body(kind, header) => {
                    self.start_body(kind, &input[..header]);
                    return (header, Step::Pending);
                }
            }
        }
        (input.len(), Step::Pass)
    }

    /// Adds input to a held prefix a byte at a time until it's decided.
    fn prefix(&mut self, input: &[u8]) -> (usize, Step) {
        let State::Prefix(held) = &mut self.state else {
            unreachable!()
        };
        for (i, &b) in input.iter().enumerate() {
            held.push(b);
            match classify(held) {
                Classified::NeedMore => continue,
                Classified::Whole(_) => {
                    let seq = whole(held);
                    self.state = State::Ground;
                    return (i + 1, Step::Sequence(seq));
                }
                Classified::Body(kind, _) => {
                    let header = std::mem::take(held);
                    self.start_body(kind, &header);
                    return (i + 1, Step::Pending);
                }
                Classified::NotOurs => {
                    // Pass up to the next ESC, which may start one of ours.
                    let cut = memchr::memchr(ESC, &held[1..]).map_or(held.len(), |k| k + 1);
                    let rest = held.split_off(cut);
                    let pass = std::mem::replace(held, rest);
                    if held.is_empty() {
                        self.state = State::Ground;
                    }
                    return (i + 1, Step::PassHeld(pass));
                }
            }
        }
        (input.len(), Step::Pending)
    }

    fn start_body(&mut self, kind: Kind, header: &[u8]) {
        self.state = State::Body {
            kind,
            header: header.to_vec(),
            data: Vec::new(),
            overflow: false,
            esc: false,
        };
    }

    fn body(&mut self, input: &[u8]) -> (usize, Step) {
        let State::Body {
            kind,
            data,
            overflow,
            esc,
            ..
        } = &mut self.state
        else {
            unreachable!()
        };
        if *esc {
            // ESC ends the string; with `\` it's ST, else the ESC starts
            // whatever comes next.
            let Some(&next) = input.first() else {
                return (0, Step::Pending);
            };
            let st = next == b'\\';
            let step = self.finish();
            if !st {
                self.state = State::Prefix(vec![ESC]);
            }
            return (usize::from(st), step);
        }
        let end = match kind {
            Kind::Iterm => memchr::memchr3(ESC, BEL, CAN, input),
            _ => memchr::memchr3(ESC, CAN, SUB, input),
        };
        let take = end.unwrap_or(input.len());
        if !*overflow {
            if data.len() + take > MAX_SEQUENCE {
                *overflow = true;
                *data = Vec::new();
            } else {
                data.extend_from_slice(&input[..take]);
            }
        }
        let Some(end) = end else {
            return (input.len(), Step::Pending);
        };
        match input[end] {
            ESC => {
                *esc = true;
                let (used, step) = self.body(&input[end + 1..]);
                (end + 1 + used, step)
            }
            BEL => (end + 1, self.finish()),
            // CAN and SUB abort the string, as in vte.
            _ => {
                self.state = State::Ground;
                (end + 1, Step::Pending)
            }
        }
    }

    /// Ends the current body, back to ground.
    fn finish(&mut self) -> Step {
        let State::Body {
            kind,
            header,
            data,
            overflow,
            ..
        } = std::mem::take(&mut self.state)
        else {
            unreachable!()
        };
        if overflow {
            tracing::warn!("dropped an image sequence over {MAX_SEQUENCE} bytes");
            return Step::Pending;
        }
        match kind {
            Kind::Apc => match data.split_first() {
                Some((b'G', rest)) => Step::Sequence(Sequence::Kitty(rest.to_vec())),
                _ => Step::Pending,
            },
            Kind::Sixel => Step::Sequence(Sequence::Sixel {
                params: numbers(&header[2..header.len() - 1]),
                data,
            }),
            Kind::Iterm => {
                // The key may have been read as part of the header (when
                // it was split across reads) or of the data.
                let mut body = header[2..].to_vec();
                body.extend_from_slice(&data);
                body.drain(..5);
                Step::Sequence(Sequence::Iterm(body))
            }
        }
    }
}

/// Decides what the escape at the start of `bytes` is.
fn classify(bytes: &[u8]) -> Classified {
    let Some(&kind) = bytes.get(1) else {
        return Classified::NeedMore;
    };
    match kind {
        b'_' => Classified::Body(Kind::Apc, 2),
        b'P' => match params_end(&bytes[2..]) {
            Some(Ok(n)) if bytes[2 + n] == b'q' => Classified::Body(Kind::Sixel, 3 + n),
            Some(Ok(_)) => Classified::NotOurs,
            Some(Err(())) => Classified::NotOurs,
            None => Classified::NeedMore,
        },
        b']' => {
            let rest = &bytes[2..];
            if ITERM_KEYS.iter().any(|key| rest.starts_with(key)) {
                Classified::Body(Kind::Iterm, 2)
            } else if ITERM_KEYS.iter().any(|key| key.starts_with(rest)) {
                Classified::NeedMore
            } else {
                Classified::NotOurs
            }
        }
        b'[' => match bytes.get(2) {
            None => Classified::NeedMore,
            Some(b'?') => match params_end(&bytes[3..]) {
                Some(Ok(n)) if bytes[3 + n] == b'S' => Classified::Whole(4 + n),
                Some(_) => Classified::NotOurs,
                None => Classified::NeedMore,
            },
            Some(_) => Classified::NotOurs,
        },
        _ => Classified::NotOurs,
    }
}

/// Length of the run of digits and `;` at the start of `bytes`: `None`
/// when it runs to the end, `Err` when it's implausibly long.
fn params_end(bytes: &[u8]) -> Option<Result<usize, ()>> {
    let n = bytes
        .iter()
        .take(MAX_PARAMS + 1)
        .position(|b| !matches!(b, b'0'..=b'9' | b';'));
    match n {
        Some(n) => Some(Ok(n)),
        None if bytes.len() > MAX_PARAMS => Some(Err(())),
        None => None,
    }
}

/// The XTSMGRAPHICS sequence `ESC [ ? params S`.
fn whole(bytes: &[u8]) -> Sequence {
    Sequence::Xtsm(numbers(&bytes[3..bytes.len() - 1]))
}

/// `;`-separated numbers; empty ones are 0.
fn numbers(bytes: &[u8]) -> Vec<u32> {
    if bytes.is_empty() {
        return Vec::new();
    }
    bytes
        .split(|&b| b == b';')
        .map(|n| {
            n.iter().fold(0u32, |acc, &d| {
                acc.saturating_mul(10).saturating_add(u32::from(d - b'0'))
            })
        })
        .collect()
}

/// Splits `input` into what the parser gets and the sequences, for tests.
#[cfg(test)]
pub fn split_all(scanner: &mut Scanner, input: &[u8]) -> (Vec<u8>, Vec<Sequence>) {
    let mut text = Vec::new();
    let mut seqs = Vec::new();
    let mut pos = 0;
    while pos < input.len() {
        let (used, step) = scanner.next(&input[pos..]);
        let range = pos..pos + used;
        pos += used;
        match step {
            Step::Pass => text.extend_from_slice(&input[range]),
            Step::PassHeld(bytes) => text.extend_from_slice(&bytes),
            Step::Sequence(seq) => seqs.push(seq),
            Step::Pending => {}
        }
    }
    (text, seqs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(input: &[u8]) -> (Vec<u8>, Vec<Sequence>) {
        split_all(&mut Scanner::default(), input)
    }

    /// Feeds `input` a byte at a time, as the worst-case read boundaries.
    fn split_bytewise(input: &[u8]) -> (Vec<u8>, Vec<Sequence>) {
        let mut scanner = Scanner::default();
        let mut text = Vec::new();
        let mut seqs = Vec::new();
        for b in input.chunks(1) {
            let (t, s) = split_all(&mut scanner, b);
            text.extend(t);
            seqs.extend(s);
        }
        (text, seqs)
    }

    #[test]
    fn plain_output_passes_in_one_piece() {
        let mut scanner = Scanner::default();
        let input = b"ls \x1b[31mred\x1b[0m \x1b]0;title\x07 \x1b[?25h done";
        assert_eq!(scanner.next(input), (input.len(), Step::Pass));
    }

    #[test]
    fn cuts_out_each_protocol() {
        let input = b"a\x1b_Gi=1;AAAA\x1b\\b\x1bP0;1;0q#0~\x1b\\c\x1b]1337;File=inline=1:QQ==\x07d\x1b[?1;1;0Se";
        for (text, seqs) in [split(input), split_bytewise(input)] {
            assert_eq!(text, b"abcde");
            assert_eq!(
                seqs,
                vec![
                    Sequence::Kitty(b"i=1;AAAA".to_vec()),
                    Sequence::Sixel {
                        params: vec![0, 1, 0],
                        data: b"#0~".to_vec()
                    },
                    Sequence::Iterm(b"File=inline=1:QQ==".to_vec()),
                    Sequence::Xtsm(vec![1, 1, 0]),
                ]
            );
        }
    }

    #[test]
    fn leaves_other_strings_and_csi_alone() {
        let input =
            b"\x1bP$qm\x1b\\\x1b]1337;SetMark\x07\x1b]133;A\x07\x1b[?2026h\x1b[2J\x1b_other\x1b\\x";
        for (text, seqs) in [split(input), split_bytewise(input)] {
            // Other APCs vanish, as vte would drop them.
            assert_eq!(
                text,
                b"\x1bP$qm\x1b\\\x1b]1337;SetMark\x07\x1b]133;A\x07\x1b[?2026h\x1b[2Jx"
            );
            assert!(seqs.is_empty());
        }
    }

    #[test]
    fn esc_inside_a_body_ends_it() {
        // ESC without `\` ends the APC and starts the next escape.
        let (text, seqs) = split(b"\x1b_Ga=d\x1b[1mX");
        assert_eq!(seqs, vec![Sequence::Kitty(b"a=d".to_vec())]);
        assert_eq!(text, b"\x1b[1mX");
        // CAN aborts it.
        let (text, seqs) = split(b"\x1b_Ga=d\x18X");
        assert!(seqs.is_empty());
        assert_eq!(text, b"X");
    }

    #[test]
    fn held_prefix_that_isnt_ours_keeps_a_following_escape() {
        let mut scanner = Scanner::default();
        assert_eq!(scanner.next(b"\x1b]13"), (4, Step::Pending));
        let (text, seqs) = split_all(&mut scanner, b"\x1b_Gq=2\x1b\\z");
        assert_eq!(text, b"\x1b]13z");
        assert_eq!(seqs, vec![Sequence::Kitty(b"q=2".to_vec())]);
    }
}
