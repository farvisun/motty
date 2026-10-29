//! What every command shares: the global flags, the connection to the app,
//! the default target, and printing tables or JSON.

use std::path::PathBuf;
use std::time::Duration;

use motty_ipc::{Client, PaneRef};
use serde::Serialize;

/// The global flags, as the commands use them.
pub struct Ctx {
    pub json: bool,
    pub no_headers: bool,
    pub quiet: bool,
    pub yes: bool,
    pub socket: Option<PathBuf>,
    /// The IPC read timeout; `None` waits forever.
    pub timeout: Option<Duration>,
}

impl Ctx {
    /// A connection with the `--timeout` read timeout.
    pub fn connect(&self) -> anyhow::Result<Client> {
        let mut client = match &self.socket {
            Some(path) => Client::connect_at(path)?,
            None => Client::connect()?,
        };
        client.set_timeout(self.timeout)?;
        Ok(client)
    }

    /// A connection that waits as long as the app takes (`run`, `wait`).
    pub fn connect_patient(&self) -> anyhow::Result<Client> {
        let mut client = self.connect()?;
        client.set_timeout(None)?;
        Ok(client)
    }

    pub fn print_json(&self, value: &impl Serialize) {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        );
    }

    /// `value` as JSON with `--json`, else `text()`.
    pub fn print(&self, value: &impl Serialize, text: impl FnOnce()) {
        if self.json {
            self.print_json(value);
        } else {
            text();
        }
    }

    /// A line that only says it worked; `-q` drops it.
    pub fn note(&self, line: impl std::fmt::Display) {
        if !self.quiet {
            println!("{line}");
        }
    }

    /// Columns padded to their widest cell, headers first unless
    /// `--no-headers`.
    pub fn table(&self, headers: &[&str], rows: &[Vec<String>]) {
        let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
        for row in rows {
            for (w, cell) in widths.iter_mut().zip(row) {
                *w = (*w).max(cell.chars().count());
            }
        }
        let line = |cells: &[String]| {
            let last = cells.len().saturating_sub(1);
            let mut out = String::new();
            for (i, (cell, w)) in cells.iter().zip(&widths).enumerate() {
                if i == last {
                    out.push_str(cell);
                } else {
                    out.push_str(&format!("{cell:<w$}  "));
                }
            }
            out.trim_end().to_owned()
        };
        if !self.no_headers {
            let headers: Vec<String> = headers.iter().map(|h| h.to_string()).collect();
            println!("{}", line(&headers));
        }
        for row in rows {
            println!("{}", line(row));
        }
    }
}

/// The pane this command runs in (`$MOTTY_PANE_ID`).
pub fn own_pane() -> Option<u64> {
    std::env::var(motty_ipc::PANE_ENV).ok()?.parse().ok()
}

/// What `--pane`, `--tab` and `--window` name; with none of them, the pane
/// this command runs in (and so its tab and window), else the focused one.
pub fn target(pane: Option<&str>, tab: Option<&str>, window: Option<&str>) -> PaneRef {
    if pane.is_none() && tab.is_none() && window.is_none() {
        return PaneRef::id(own_pane());
    }
    PaneRef {
        pane: None,
        select: pane.map(str::to_owned),
        tab: tab.map(str::to_owned),
        window: window.map(str::to_owned),
    }
}

/// `2026-10-29 14:05` (UTC) for Unix seconds.
pub fn utc_time(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let (h, m) = ((secs % 86_400) / 3600, (secs % 3600) / 60);
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_dates() {
        assert_eq!(utc_time(0), "1970-01-01 00:00");
        assert_eq!(utc_time(1_793_281_500), "2026-10-29 13:45");
        assert_eq!(utc_time(951_782_400), "2000-02-29 00:00");
    }

    #[test]
    fn default_target_is_the_callers_pane() {
        let t = target(Some("index:2"), None, Some("w_1"));
        assert_eq!(t.select.as_deref(), Some("index:2"));
        assert_eq!(t.window.as_deref(), Some("w_1"));
        assert!(t.pane.is_none());
    }
}
