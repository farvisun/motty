//! Syntax highlighting with syntect and bat's grammars (through two-face),
//! colored from the terminal palette so it follows the theme, as Otty does.
//!
//! Parse states are cached per line, so an edit re-parses from the edited
//! line only until the state matches the cache again, and scrolling parses
//! only what hasn't been parsed yet.

#![allow(dead_code)]

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use motty_render::TerminalStyle;
use motty_term::alacritty_terminal::vte::ansi::NamedColor;
use motty_theme::Rgba;
use motty_theme::theme::{luminance, mix};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

/// Lines longer than this aren't parsed (minified files would stall).
const MAX_LINE: usize = 20_000;

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// Every language's name, sorted, for a language picker.
pub fn languages() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = syntaxes()
        .syntaxes()
        .iter()
        .filter(|s| !s.hidden)
        .map(|s| s.name.as_str())
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// The language of a file, from its extension, then its name (`Makefile`),
/// then its first line (a shebang or a mode line).
pub fn detect(path: Option<&Path>, first_line: &str) -> Option<&'static str> {
    let set = syntaxes();
    let named = |name: &str| set.find_syntax_by_name(name);
    let found = path
        .and_then(|path| {
            let ext = path.extension().and_then(|e| e.to_str());
            let file = path.file_name().and_then(|n| n.to_str());
            let preferred = ext.and_then(|e| match e.to_ascii_lowercase().as_str() {
                "m" => named("Objective-C"),
                "h" => named("C"),
                "v" => named("Verilog"),
                "pl" => named("Perl"),
                _ => None,
            });
            preferred
                .or_else(|| file.and_then(|f| set.find_syntax_by_extension(f)))
                .or_else(|| ext.and_then(|e| set.find_syntax_by_extension(e)))
                .or_else(|| {
                    let file = file?;
                    ["Dockerfile", "Containerfile"]
                        .iter()
                        .any(|p| file.starts_with(p))
                        .then(|| named("Dockerfile"))
                        .flatten()
                })
        })
        .or_else(|| set.find_syntax_by_first_line(first_line))
        .or_else(|| shebang(first_line).and_then(|i| set.find_syntax_by_token(i)));
    found
        .filter(|s| s.name != "Plain Text")
        .map(|s| s.name.as_str())
}

/// The interpreter a `#!` line names, without its version (`python3` → `python`).
fn shebang(line: &str) -> Option<&str> {
    let mut words = line.strip_prefix("#!")?.split_whitespace();
    let mut program = words.next()?.rsplit('/').next()?;
    if program == "env" {
        program = words.find(|w| !w.starts_with('-') && !w.contains('='))?;
    }
    let program = program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    Some(match program {
        "sh" | "zsh" | "ksh" | "dash" | "ash" => "bash",
        "node" | "deno" | "bun" => "js",
        "pwsh" => "powershell",
        other => other,
    })
}

/// What a piece of code is, as far as coloring goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    Comment,
    Keyword,
    String,
    Number,
    Constant,
    Function,
    Type,
    Variable,
    Operator,
    Punctuation,
    Tag,
    Attribute,
    Escape,
    Heading,
    Emphasis,
    Strong,
    Link,
    Inserted,
    Deleted,
    Changed,
    Plain,
}

impl Token {
    /// Tokens whose delimiters (quotes, `#`) take their color. A diff's
    /// `+` and `-` don't: Otty leaves them plain.
    fn wraps_punctuation(self) -> bool {
        matches!(
            self,
            Token::String
                | Token::Escape
                | Token::Heading
                | Token::Emphasis
                | Token::Strong
                | Token::Link
                | Token::Changed
        )
    }
}

/// A run of one token within a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// Byte range in the line.
    pub range: Range<usize>,
    pub token: Token,
}

fn is(scope: &str, prefix: &str) -> bool {
    scope
        .strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// The token one scope stands for, by its most specific known prefix.
fn classify(scope: &str) -> Option<Token> {
    let any = |prefixes: &[&str]| prefixes.iter().any(|p| is(scope, p));
    Some(if any(&["comment"]) {
        Token::Comment
    } else if any(&["constant.character.escape"]) {
        Token::Escape
    } else if any(&["constant.numeric"]) {
        Token::Number
    } else if any(&["constant", "support.constant", "entity.name.constant"]) {
        Token::Constant
    } else if any(&["string.other.link", "markup.underline.link", "markup.link"]) {
        Token::Link
    } else if any(&["string", "markup.raw"]) {
        Token::String
    } else if any(&["keyword.operator.word"]) {
        Token::Keyword
    } else if any(&["keyword.operator"]) {
        Token::Operator
    } else if any(&["keyword", "storage", "variable.language"]) {
        Token::Keyword
    } else if any(&[
        "entity.name.function",
        "support.function",
        "variable.function",
        "support.macro",
        "entity.name.macro",
    ]) {
        Token::Function
    } else if any(&["entity.name.tag"]) {
        Token::Tag
    } else if any(&["entity.other.attribute-name"]) {
        Token::Attribute
    } else if any(&["entity.name.section", "markup.heading"]) {
        Token::Heading
    } else if any(&[
        "entity.name",
        "entity.other.inherited-class",
        "support.type",
        "support.class",
    ]) {
        Token::Type
    } else if any(&["variable", "support.variable"]) {
        Token::Variable
    } else if any(&["markup.inserted"]) {
        Token::Inserted
    } else if any(&["markup.deleted"]) {
        Token::Deleted
    } else if any(&["markup.changed"]) {
        Token::Changed
    } else if any(&["markup.italic"]) {
        Token::Emphasis
    } else if any(&["markup.bold"]) {
        Token::Strong
    } else if any(&["markup.quote"]) {
        Token::Comment
    } else if any(&["punctuation"]) {
        Token::Punctuation
    } else {
        return None;
    })
}

#[derive(Clone, PartialEq, Eq)]
struct State {
    parse: ParseState,
    scopes: ScopeStack,
}

/// Highlights one document, caching the parse state before each line.
pub struct Highlighter {
    syntax: Option<&'static SyntaxReference>,
    /// `states[i]` is the state before line `i`. `states[..valid]` is
    /// current; later entries predate an edit and are reused once parsing
    /// reaches the same state again. `None` marks lines an edit replaced.
    states: Vec<Option<Arc<State>>>,
    valid: usize,
    tokens: HashMap<Scope, Option<Token>>,
    line: String,
}

impl Highlighter {
    /// A highlighter for a language by name or token (`Rust`, `rs`, `rust`);
    /// unknown languages and `None` stay plain.
    pub fn new(language: Option<&str>) -> Self {
        let set = syntaxes();
        let syntax = language
            .and_then(|l| {
                set.find_syntax_by_name(l)
                    .or_else(|| set.find_syntax_by_token(l))
            })
            .filter(|s| s.name != "Plain Text");
        let states = syntax
            .map(|s| {
                vec![Some(Arc::new(State {
                    parse: ParseState::new(s),
                    scopes: ScopeStack::new(),
                }))]
            })
            .unwrap_or_default();
        Self {
            syntax,
            valid: states.len(),
            states,
            tokens: HashMap::new(),
            line: String::new(),
        }
    }

    /// The language's name, or `None` when plain.
    pub fn language(&self) -> Option<&'static str> {
        self.syntax.map(|s| s.name.as_str())
    }

    /// Forgets everything from `line` on (any edit, without reuse).
    pub fn invalidate_from(&mut self, line: usize) {
        if self.syntax.is_none() {
            return;
        }
        self.states.truncate(line + 1);
        self.valid = self.valid.min(self.states.len());
    }

    /// Lines `line..line + removed` became `inserted` lines (both at least
    /// one: an edit always touches the line it starts on). Lines after them
    /// keep their cached states until parsing proves them stale.
    pub fn edit(&mut self, line: usize, removed: usize, inserted: usize) {
        if self.syntax.is_none() {
            return;
        }
        let (removed, inserted) = (removed.max(1), inserted.max(1));
        let keep = line + 1;
        self.valid = self.valid.min(keep);
        if self.states.len() > keep {
            let end = (line + removed).min(self.states.len());
            self.states
                .splice(keep..end, std::iter::repeat_n(None, inserted - 1));
        }
    }

    /// Spans for each line in `range`, which must lie within the document.
    pub fn highlight<'a>(
        &mut self,
        line: impl Fn(usize) -> &'a str,
        range: Range<usize>,
    ) -> Vec<Vec<Span>> {
        if self.syntax.is_none() {
            return range.map(|i| plain(line(i))).collect();
        }
        let start = range.start.min(self.valid - 1);
        let mut out = Vec::with_capacity(range.len());
        let mut state = State::clone(self.states[start].as_ref().expect("valid state"));
        for i in start..range.end {
            let text = line(i);
            let spans = self.parse(&mut state, text, range.contains(&i));
            if range.contains(&i) {
                out.push(spans);
            }
            self.store(i + 1, &state);
        }
        out
    }

    /// Spans for `lines[range]`.
    pub fn highlight_lines(&mut self, lines: &[&str], range: Range<usize>) -> Vec<Vec<Span>> {
        self.highlight(|i| lines[i], range)
    }

    /// Records the state before line `ix`, ending the re-parse when it
    /// matches what was cached before the edit.
    fn store(&mut self, ix: usize, state: &State) {
        if ix < self.valid {
            return;
        }
        match self.states.get(ix) {
            Some(Some(old)) if **old == *state => {
                self.valid = self.states[ix..]
                    .iter()
                    .position(Option::is_none)
                    .map_or(self.states.len(), |n| ix + n);
            }
            _ => {
                let prev = &self.states[ix - 1];
                let new = match prev {
                    Some(prev) if **prev == *state => prev.clone(),
                    _ => Arc::new(state.clone()),
                };
                if ix < self.states.len() {
                    self.states[ix] = Some(new);
                } else {
                    self.states.push(Some(new));
                }
                self.valid = ix + 1;
            }
        }
    }

    /// Advances `state` over one line, returning its spans when `want`.
    fn parse(&mut self, state: &mut State, text: &str, want: bool) -> Vec<Span> {
        if text.len() > MAX_LINE {
            return if want { plain(text) } else { Vec::new() };
        }
        self.line.clear();
        self.line.push_str(text);
        self.line.push('\n');
        let Ok(ops) = state.parse.parse_line(&self.line, syntaxes()) else {
            return if want { plain(text) } else { Vec::new() };
        };
        let mut spans: Vec<Span> = Vec::new();
        let mut at = 0;
        for (pos, op) in &ops {
            let pos = (*pos).min(text.len());
            if want && pos > at {
                let token = self.token(state.scopes.as_slice());
                push(&mut spans, at..pos, token);
            }
            at = at.max(pos);
            let _ = state.scopes.apply(op);
        }
        if want && at < text.len() {
            let token = self.token(state.scopes.as_slice());
            push(&mut spans, at..text.len(), token);
        }
        spans
    }

    /// The innermost token in a scope stack. Comments win outright, and
    /// punctuation takes the color of the string or markup around it.
    fn token(&mut self, stack: &[Scope]) -> Token {
        let mut found = None;
        let mut punctuation = false;
        for scope in stack.iter().rev() {
            let token = *self
                .tokens
                .entry(*scope)
                .or_insert_with(|| classify(&scope.build_string()));
            match token {
                Some(Token::Comment) => return Token::Comment,
                Some(Token::Punctuation) => punctuation |= found.is_none(),
                Some(t) if found.is_none() => {
                    found = Some(if punctuation && !t.wraps_punctuation() {
                        Token::Punctuation
                    } else {
                        t
                    });
                }
                _ => {}
            }
        }
        found.unwrap_or(if punctuation {
            Token::Punctuation
        } else {
            Token::Plain
        })
    }
}

fn plain(text: &str) -> Vec<Span> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![Span {
            range: 0..text.len(),
            token: Token::Plain,
        }]
    }
}

fn push(spans: &mut Vec<Span>, range: Range<usize>, token: Token) {
    match spans.last_mut() {
        Some(last) if last.token == token && last.range.end == range.start => {
            last.range.end = range.end
        }
        _ => spans.push(Span { range, token }),
    }
}

/// A token's color in the terminal's palette, and whether it's italic.
/// `None` draws it in the default foreground.
pub fn token_color(token: Token, style: &TerminalStyle) -> Option<(Rgba, bool)> {
    let mut palette = [Rgba::TRANSPARENT; 16];
    palette.copy_from_slice(&style.palette[..16]);
    let background = if style.background.a > 0.5 {
        style.background
    } else {
        style.panel_background
    };
    palette_color(
        token,
        &palette,
        style.palette[NamedColor::Foreground as usize],
        background,
    )
}

/// [`token_color`] for a bare 16-color palette.
pub fn palette_color(
    token: Token,
    palette: &[Rgba; 16],
    foreground: Rgba,
    background: Rgba,
) -> Option<(Rgba, bool)> {
    let color = match token {
        Token::Comment => {
            // Some themes make bright black the background (Solarized).
            let grey = palette[8];
            if contrast(grey, background) >= 2. {
                grey
            } else {
                mix(foreground, background, 0.45)
            }
        }
        Token::Keyword => palette[5],
        Token::String => palette[2],
        Token::Number | Token::Constant | Token::Attribute | Token::Changed => palette[3],
        Token::Function | Token::Heading | Token::Link | Token::Inserted => palette[4],
        Token::Type | Token::Escape => palette[6],
        Token::Operator | Token::Tag | Token::Deleted => palette[1],
        Token::Emphasis => return Some((foreground, true)),
        Token::Variable | Token::Punctuation | Token::Strong | Token::Plain => return None,
    };
    Some((color, false))
}

/// Whether a token is drawn bold.
pub fn token_bold(token: Token) -> bool {
    matches!(token, Token::Heading | Token::Strong)
}

fn contrast(a: Rgba, b: Rgba) -> f32 {
    let (a, b) = (luminance(a) + 0.05, luminance(b) + 0.05);
    a.max(b) / a.min(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(h: &mut Highlighter, lines: &[&str], line: usize) -> Vec<(String, Token)> {
        let spans = h.highlight_lines(lines, line..line + 1).remove(0);
        spans
            .into_iter()
            .map(|s| (lines[line][s.range].to_string(), s.token))
            .collect()
    }

    fn token_of(spans: &[(String, Token)], text: &str) -> Option<Token> {
        spans
            .iter()
            .find(|(t, _)| t.trim() == text)
            .map(|(_, t)| *t)
    }

    #[test]
    fn detects_languages() {
        let p = |s: &str| detect(Some(Path::new(s)), "");
        assert_eq!(p("main.rs"), Some("Rust"));
        assert_eq!(p("a/b/script.py"), Some("Python"));
        assert_eq!(p("Makefile"), Some("Makefile"));
        assert_eq!(p("lib.h"), Some("C"));
        assert_eq!(p("View.m"), Some("Objective-C"));
        assert_eq!(p("top.v"), Some("Verilog"));
        assert_eq!(p("x.pl"), Some("Perl"));
        assert_eq!(p("Cargo.toml"), Some("TOML"));
        assert_eq!(p("app.ts"), Some("TypeScript"));
        assert_eq!(p("Dockerfile"), Some("Dockerfile"));
        assert_eq!(
            detect(Some(Path::new("run")), "#!/usr/bin/env python3"),
            Some("Python")
        );
        assert_eq!(
            detect(None, "#!/bin/zsh"),
            Some("Bourne Again Shell (bash)")
        );
        assert_eq!(detect(Some(Path::new("notes")), "hello"), None);
    }

    #[test]
    fn lists_languages() {
        let names = languages();
        assert!(names.len() > 100, "{}", names.len());
        assert!(names.contains(&"Rust") && names.contains(&"Swift") && names.contains(&"Zig"));
        assert!(
            names
                .windows(2)
                .all(|w| w[0].to_lowercase() <= w[1].to_lowercase())
        );
    }

    #[test]
    fn rust_tokens() {
        let lines = [
            "use std::io;",
            "pub fn main() {",
            "    let x = 42 + 1; // done",
            "    println!(\"hi\");",
            "}",
        ];
        let mut h = Highlighter::new(Some("Rust"));
        let l0 = tokens(&mut h, &lines, 0);
        assert_eq!(token_of(&l0, "use"), Some(Token::Keyword));
        let l1 = tokens(&mut h, &lines, 1);
        assert_eq!(token_of(&l1, "pub"), Some(Token::Keyword));
        assert_eq!(token_of(&l1, "fn"), Some(Token::Keyword));
        assert_eq!(token_of(&l1, "main"), Some(Token::Function));
        let l2 = tokens(&mut h, &lines, 2);
        assert_eq!(token_of(&l2, "let"), Some(Token::Keyword));
        assert_eq!(token_of(&l2, "42"), Some(Token::Number));
        assert_eq!(token_of(&l2, "="), Some(Token::Operator));
        assert_eq!(token_of(&l2, "// done"), Some(Token::Comment));
        let l3 = tokens(&mut h, &lines, 3);
        assert_eq!(token_of(&l3, "\"hi\""), Some(Token::String));
        // Spans tile each line.
        for (i, spans) in h.highlight_lines(&lines, 0..lines.len()).iter().enumerate() {
            let mut at = 0;
            for s in spans {
                assert_eq!(s.range.start, at);
                at = s.range.end;
            }
            assert_eq!(at, lines[i].len());
        }
    }

    #[test]
    fn edits_reparse_following_lines() {
        let mut lines = vec!["let a = 1;", "let b = 2;", "let c = 3;", "let d = 4;"];
        let mut h = Highlighter::new(Some("rs"));
        assert_eq!(
            token_of(&tokens(&mut h, &lines, 3), "let"),
            Some(Token::Keyword)
        );

        lines[0] = "/* let a = 1;";
        h.edit(0, 1, 1);
        assert_eq!(tokens(&mut h, &lines, 3)[0].1, Token::Comment);

        lines[0] = "let a = 1;";
        h.invalidate_from(0);
        assert_eq!(
            token_of(&tokens(&mut h, &lines, 3), "let"),
            Some(Token::Keyword)
        );

        // Inserting lines shifts the cache; parsing converges right after.
        h.highlight_lines(&lines, 0..lines.len());
        lines.insert(1, "/* x */");
        h.edit(1, 1, 2);
        let all = h.highlight_lines(&lines, 0..lines.len());
        assert_eq!(all.len(), 5);
        assert_eq!(h.valid, h.states.len());
        assert_eq!(all[1][0].token, Token::Comment);
        assert_eq!(all[4][0].token, Token::Keyword);
    }

    #[test]
    fn plain_without_language() {
        let mut h = Highlighter::new(None);
        let spans = h.highlight_lines(&["fn x", ""], 0..2);
        assert_eq!(
            spans[0],
            vec![Span {
                range: 0..4,
                token: Token::Plain
            }]
        );
        assert!(spans[1].is_empty());
        assert!(
            Highlighter::new(Some("no such language"))
                .language()
                .is_none()
        );
    }

    #[test]
    fn large_file_is_fast() {
        let source =
            "fn f(x: u32) -> u32 {\n    let y = x * 2; // twice\n    y + 1\n}\n".repeat(2_500);
        let lines: Vec<&str> = source.lines().collect();
        let mut h = Highlighter::new(Some("Rust"));
        let t = std::time::Instant::now();
        let end = h.highlight_lines(&lines, lines.len() - 50..lines.len());
        assert_eq!(end.len(), 50);
        h.edit(5_000, 1, 1);
        h.highlight_lines(&lines, 5_000..5_050);
        h.highlight_lines(&lines, lines.len() - 50..lines.len());
        assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
    }

    #[test]
    fn colors_follow_palette() {
        let mut palette = [Rgba::TRANSPARENT; 16];
        for (i, c) in palette.iter_mut().enumerate() {
            *c = Rgba::rgb(0x101010 * (i as u32 + 1));
        }
        let (fg, bg) = (Rgba::rgb(0xeeeeee), Rgba::rgb(0x000000));
        assert_eq!(
            palette_color(Token::Keyword, &palette, fg, bg),
            Some((palette[5], false))
        );
        assert_eq!(palette_color(Token::Plain, &palette, fg, bg), None);
        // Bright black too close to the background falls back to a grey.
        palette[8] = Rgba::rgb(0x050505);
        assert_ne!(
            palette_color(Token::Comment, &palette, fg, bg).unwrap().0,
            palette[8]
        );
    }

    #[test]
    fn colors_from_terminal_style() {
        for name in ["Tokyo Night", "Solarized Dark", "Motty Light"] {
            let source = motty_theme::config::builtin_source(name).expect(name);
            let theme = motty_theme::theme::parse(source).unwrap().theme;
            let style = TerminalStyle::from_theme(&theme, None, None, false);
            let (comment, _) = token_color(Token::Comment, &style).unwrap();
            assert!(contrast(comment, style.background.over(style.panel_background)) >= 1.5);
            assert_eq!(
                token_color(Token::Keyword, &style),
                Some((style.palette[5], false))
            );
            assert_eq!(token_color(Token::Punctuation, &style), None);
        }
    }
}
