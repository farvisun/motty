//! The kitty clipboard protocol (OSC 5522): programs read and write any
//! kind of clipboard data, pictures and HTML included, where OSC 52 only
//! carries text. With paste events (mode 5522) a paste tells the program
//! which types the clipboard holds, and it reads the one it wants.
//!
//! Packets are parsed and writes collected on the PTY thread; the app gets
//! whole [`Request`]s, asks about them as it does about OSC 52, and answers
//! with the reply builders here.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// Most bytes one write may put on the clipboard.
pub const WRITE_LIMIT: usize = 64 << 20;
/// Bytes of data per reply packet, before base64.
const CHUNK: usize = 4096;
/// The type a read asks for to list the clipboard's types.
pub const LIST: &str = ".";

/// A program's password, which the user may trust for the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Password {
    pub secret: String,
    /// Who's asking, in the program's words.
    pub name: String,
}

/// What a program asks of the clipboard.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// The data of each MIME type in `types` the clipboard has, or with
    /// [`LIST`] the types themselves.
    Read {
        types: Vec<String>,
        password: Option<Password>,
        reply: Reply,
    },
    /// Data for each MIME type, in order.
    Write {
        items: Vec<(String, Vec<u8>)>,
        password: Option<Password>,
        reply: Reply,
    },
}

impl Request {
    pub fn password(&self) -> Option<&Password> {
        match self {
            Self::Read { password, .. } | Self::Write { password, .. } => password.as_ref(),
        }
    }

    /// Whether this only lists the clipboard's types, which needs no
    /// permission.
    pub fn is_listing(&self) -> bool {
        matches!(self, Self::Read { types, .. } if types.iter().all(|t| t == LIST))
    }
}

/// Where answers to a request go: the id a multiplexer tagged it with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    id: Option<String>,
}

impl Reply {
    fn packet(&self, kind: &str, status: &str, extra: &str, payload: Option<&str>) -> String {
        let mut out = format!("\x1b]5522;type={kind}:status={status}{extra}");
        if let Some(id) = &self.id {
            out.push_str(":id=");
            out.push_str(id);
        }
        if let Some(payload) = payload {
            out.push(';');
            out.push_str(payload);
        }
        out.push_str("\x1b\\");
        out
    }

    /// The answer to a read: each type's data in chunks, then done.
    pub fn read(&self, items: &[(String, Vec<u8>)]) -> String {
        let mut out = self.packet("read", "OK", "", None);
        for (mime, data) in items {
            let extra = format!(":mime={}", STANDARD.encode(mime));
            for chunk in chunks(data) {
                out.push_str(&self.packet("read", "DATA", &extra, Some(&STANDARD.encode(chunk))));
            }
        }
        out.push_str(&self.packet("read", "DONE", "", None));
        out
    }

    /// The answer to a listing: the types, space separated.
    pub fn list(&self, types: &[String]) -> String {
        self.read(&[(LIST.to_owned(), types.join(" ").into_bytes())])
    }

    /// A finished write.
    pub fn written(&self) -> String {
        self.packet("write", "DONE", "", None)
    }

    /// A refused read or write.
    pub fn denied(&self, request: &Request) -> String {
        let kind = match request {
            Request::Read { .. } => "read",
            Request::Write { .. } => "write",
        };
        self.packet(kind, "EPERM", "", None)
    }

    /// A write that couldn't be done (the clipboard refused the data).
    pub fn failed_write(&self) -> String {
        self.packet("write", "EIO", "", None)
    }
}

/// `data` in reply-sized pieces; empty data is one empty piece.
fn chunks(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let empty: &[u8] = &[];
    data.chunks(CHUNK).chain(data.is_empty().then_some(empty))
}

/// The packets a paste sends with paste events on: the clipboard's
/// `types`, and a one-time password that lets the program read without
/// being asked about.
pub fn paste_event(types: &[String], secret: &str) -> String {
    let reply = Reply::default();
    let pw = format!(":pw={}", STANDARD.encode(secret));
    let mut out = reply.packet("read", "OK", &pw, None);
    let extra = format!(":mime={}{pw}", STANDARD.encode(LIST));
    let list = STANDARD.encode(types.join(" "));
    out.push_str(&reply.packet("read", "DATA", &extra, Some(&list)));
    out.push_str(&reply.packet("read", "DONE", &pw, None));
    out
}

/// One OSC 5522 packet, as the VT parser split it at `;`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Packet {
    kind: String,
    mime: Option<String>,
    primary: bool,
    id: Option<String>,
    password: Option<Password>,
    /// Base64, not yet decoded: a write's chunks only decode together.
    payload: Vec<u8>,
}

impl Packet {
    /// `params` after the `5522`: the metadata, then the payload.
    pub fn parse(params: &[&[u8]]) -> Self {
        let mut packet = Self::default();
        let (mut pw, mut name) = (None, None);
        let metadata = params.first().copied().unwrap_or_default();
        for pair in metadata.split(|&b| b == b':') {
            let text = String::from_utf8_lossy(pair);
            let Some((key, value)) = text.split_once('=') else {
                continue;
            };
            match key {
                "type" => packet.kind = value.to_owned(),
                "mime" => packet.mime = decode_text(value),
                "loc" => packet.primary = value == "primary",
                "id" => {
                    let id: String = value
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '.'))
                        .collect();
                    packet.id = Some(id).filter(|id| !id.is_empty());
                }
                "pw" => pw = decode_text(value),
                "name" => name = decode_text(value),
                _ => {}
            }
        }
        // A password without a name counts as none.
        if let (Some(secret), Some(name)) = (pw, name)
            && !secret.is_empty()
            && !name.is_empty()
        {
            packet.password = Some(Password { secret, name });
        }
        packet.payload = params.get(1).copied().unwrap_or_default().to_vec();
        packet
    }

    fn reply(&self) -> Reply {
        Reply {
            id: self.id.clone(),
        }
    }
}

fn decode_text(value: &str) -> Option<String> {
    String::from_utf8(STANDARD.decode(value).ok()?).ok()
}

/// What a packet comes to: a request for the app, an answer to send right
/// away, or nothing yet.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Request(Request),
    Reply(String),
    Nothing,
}

/// A write being sent, packet by packet.
#[derive(Default)]
pub struct Writes {
    current: Option<Write>,
}

struct Write {
    password: Option<Password>,
    reply: Reply,
    /// Each type's base64, as it came.
    items: Vec<(String, Vec<u8>)>,
    aliases: Vec<(String, Vec<String>)>,
    /// Decoded size so far, about.
    size: usize,
    /// It failed and was answered; the rest of it is ignored.
    failed: bool,
}

impl Writes {
    pub fn handle(&mut self, packet: Packet) -> Outcome {
        let reply = packet.reply();
        match packet.kind.as_str() {
            "read" => {
                // macOS has no primary selection.
                if packet.primary {
                    return Outcome::Reply(reply.packet("read", "ENOSYS", "", None));
                }
                // Reads have no error for bad data: they're ignored.
                let Some(list) = STANDARD
                    .decode(&packet.payload)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                else {
                    return Outcome::Nothing;
                };
                let types: Vec<String> = list.split_whitespace().map(str::to_owned).collect();
                if types.is_empty() {
                    return Outcome::Nothing;
                }
                Outcome::Request(Request::Read {
                    types,
                    password: packet.password,
                    reply,
                })
            }
            "write" => {
                let failed = packet.primary;
                self.current = Some(Write {
                    password: packet.password,
                    reply: reply.clone(),
                    items: Vec::new(),
                    aliases: Vec::new(),
                    size: 0,
                    failed,
                });
                if failed {
                    return Outcome::Reply(reply.packet("write", "ENOSYS", "", None));
                }
                Outcome::Nothing
            }
            "wdata" => {
                let Some(write) = self.current.as_mut().filter(|w| !w.failed) else {
                    return Outcome::Nothing;
                };
                let Some(mime) = packet.mime else {
                    // The end.
                    let write = self.current.take().expect("checked above");
                    return write.finish();
                };
                write.size += packet.payload.len() / 4 * 3;
                if write.size > WRITE_LIMIT {
                    return write.fail("EFBIG");
                }
                match write.items.iter_mut().find(|(m, _)| *m == mime) {
                    Some((_, data)) => data.extend_from_slice(&packet.payload),
                    None => write.items.push((mime, packet.payload)),
                }
                Outcome::Nothing
            }
            "walias" => {
                let Some(write) = self.current.as_mut().filter(|w| !w.failed) else {
                    return Outcome::Nothing;
                };
                let aliases = STANDARD
                    .decode(&packet.payload)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok());
                match (packet.mime, aliases) {
                    (Some(target), Some(aliases)) => {
                        let aliases = aliases.split_whitespace().map(str::to_owned).collect();
                        write.aliases.push((target, aliases));
                        Outcome::Nothing
                    }
                    _ => write.fail("EINVAL"),
                }
            }
            _ => Outcome::Nothing,
        }
    }
}

impl Write {
    fn fail(&mut self, status: &str) -> Outcome {
        self.failed = true;
        self.items.clear();
        Outcome::Reply(self.reply.packet("write", status, "", None))
    }

    fn finish(self) -> Outcome {
        let mut items = Vec::with_capacity(self.items.len());
        for (mime, data) in self.items {
            match STANDARD.decode(&data) {
                Ok(data) => items.push((mime, data)),
                Err(_) => return Outcome::Reply(self.reply.packet("write", "EINVAL", "", None)),
            }
        }
        for (target, aliases) in self.aliases {
            let Some(data) = items
                .iter()
                .find(|(m, _)| *m == target)
                .map(|(_, d)| d.clone())
            else {
                continue;
            };
            for alias in aliases {
                if !items.iter().any(|(m, _)| *m == alias) {
                    items.push((alias, data.clone()));
                }
            }
        }
        if items.is_empty() {
            return Outcome::Reply(self.reply.packet("write", "EINVAL", "", None));
        }
        Outcome::Request(Request::Write {
            items,
            password: self.password,
            reply: self.reply,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        STANDARD.encode(s)
    }

    fn packet(metadata: &str, payload: &str) -> Packet {
        Packet::parse(&[metadata.as_bytes(), payload.as_bytes()])
    }

    #[test]
    fn reads_ask_the_app() {
        let mut writes = Writes::default();
        let out = writes.handle(packet("type=read:id=a b!", &b64("text/plain image/png")));
        let Outcome::Request(Request::Read { types, reply, .. }) = out else {
            panic!("{out:?}");
        };
        assert_eq!(types, ["text/plain", "image/png"]);
        // The id comes back without what isn't allowed in it.
        assert_eq!(
            reply.read(&[("text/plain".into(), b"hi".to_vec())]),
            format!(
                "\x1b]5522;type=read:status=OK:id=ab\x1b\\\
                 \x1b]5522;type=read:status=DATA:mime={}:id=ab;aGk=\x1b\\\
                 \x1b]5522;type=read:status=DONE:id=ab\x1b\\",
                b64("text/plain")
            )
        );
    }

    #[test]
    fn listing_needs_no_permission() {
        let mut writes = Writes::default();
        let Outcome::Request(request) = writes.handle(packet("type=read", &b64("."))) else {
            panic!();
        };
        assert!(request.is_listing());
    }

    #[test]
    fn bad_reads_are_ignored_and_primary_is_unsupported() {
        let mut writes = Writes::default();
        assert_eq!(
            writes.handle(packet("type=read", "not base64!")),
            Outcome::Nothing
        );
        let Outcome::Reply(reply) = writes.handle(packet("type=read:loc=primary", &b64(".")))
        else {
            panic!();
        };
        assert!(reply.contains("status=ENOSYS"));
    }

    #[test]
    fn writes_collect_chunks_and_aliases() {
        let mut writes = Writes::default();
        let png = b64("image/png");
        assert_eq!(writes.handle(packet("type=write", "")), Outcome::Nothing);
        // One type's data may be split anywhere, padding only at its end.
        let data = STANDARD.encode(b"picture bytes");
        let (a, b) = data.split_at(5);
        writes.handle(packet(&format!("type=wdata:mime={png}"), a));
        writes.handle(packet(&format!("type=wdata:mime={png}"), b));
        writes.handle(packet(
            &format!("type=walias:mime={png}"),
            &b64("image/x-png"),
        ));
        let out = writes.handle(packet("type=wdata", ""));
        let Outcome::Request(Request::Write { items, .. }) = out else {
            panic!("{out:?}");
        };
        assert_eq!(
            items,
            [
                ("image/png".to_owned(), b"picture bytes".to_vec()),
                ("image/x-png".to_owned(), b"picture bytes".to_vec()),
            ]
        );
    }

    #[test]
    fn invalid_data_fails_the_whole_write() {
        let mut writes = Writes::default();
        writes.handle(packet("type=write", ""));
        writes.handle(packet(
            &format!("type=wdata:mime={}", b64("text/plain")),
            "aGk",
        ));
        let Outcome::Reply(reply) = writes.handle(packet("type=wdata", "")) else {
            panic!();
        };
        assert!(reply.contains("type=write:status=EINVAL"));
        // Until a new write starts, the rest is ignored.
        assert_eq!(writes.handle(packet("type=wdata", "")), Outcome::Nothing);
    }

    #[test]
    fn oversized_writes_are_refused() {
        let mut writes = Writes::default();
        writes.handle(packet("type=write", ""));
        let big = "A".repeat(WRITE_LIMIT / 3 * 4 + 8);
        let out = writes.handle(packet(
            &format!("type=wdata:mime={}", b64("text/plain")),
            &big,
        ));
        assert!(matches!(out, Outcome::Reply(r) if r.contains("EFBIG")));
    }

    #[test]
    fn passwords_need_a_name() {
        let with = packet(
            &format!("type=read:pw={}:name={}", b64("s3"), b64("vim")),
            "",
        );
        assert_eq!(
            with.password,
            Some(Password {
                secret: "s3".into(),
                name: "vim".into()
            })
        );
        assert_eq!(
            packet(&format!("type=read:pw={}", b64("s3")), "").password,
            None
        );
    }

    #[test]
    fn paste_events_list_types_with_a_password() {
        let out = paste_event(&["text/plain".into(), "image/png".into()], "tok");
        let pw = b64("tok");
        assert_eq!(
            out,
            format!(
                "\x1b]5522;type=read:status=OK:pw={pw}\x1b\\\
                 \x1b]5522;type=read:status=DATA:mime=Lg==:pw={pw};{}\x1b\\\
                 \x1b]5522;type=read:status=DONE:pw={pw}\x1b\\",
                b64("text/plain image/png")
            )
        );
    }

    #[test]
    fn large_data_is_chunked() {
        let reply = Reply::default().read(&[("image/png".into(), vec![7; CHUNK * 2 + 1])]);
        assert_eq!(reply.matches("status=DATA").count(), 3);
    }
}
