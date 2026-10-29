//! The new-tab button's chevron menu: Otty's ways to open a tab (a shell
//! from /etc/shells, a recent folder, a host from ~/.ssh/config, a folder
//! picked in Finder), and dividers between tabs in the sidebar.

use std::path::{Path, PathBuf};

use gpui::{Action, Context, PathPromptOptions, Window, point, px};

use super::Workspace;
use crate::actions::*;
use crate::native_menu::{self, Entry, Item};
use crate::titlebar;

/// Open Recent lists this many folders.
const RECENT: usize = 10;

enum Pick {
    /// A disabled item.
    None,
    Action(Box<dyn Action>),
    Home,
    Shell(PathBuf),
    Open,
    Folder(PathBuf),
    Host(String),
    InsertDivider,
    RemoveDividers,
}

impl Workspace {
    /// Pops the menu up under the chevron, which stays pressed meanwhile.
    pub(super) fn show_new_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.focus_handle.clone();
        let key = |action: &dyn Action| {
            window
                .highest_precedence_binding_for_action_in(action, &focus)
                .and_then(|b| b.keystrokes().first().map(|k| k.inner().clone()))
        };
        let mut entries: Vec<Entry> = Vec::new();
        let mut picks: Vec<Pick> = Vec::new();
        // Items count in menu order (submenus' in place), as picks do.
        let add = |picks: &mut Vec<Pick>, item: Item, pick: Pick| -> Entry {
            picks.push(pick);
            item.into()
        };

        entries.push(add(
            &mut picks,
            Item::new("New").keystroke(key(&NewTab)),
            Pick::Action(Box::new(NewTab)),
        ));
        entries.push(add(&mut picks, Item::new("New in Home Folder"), Pick::Home));
        // Submenu items count in place, so their picks go in the same order.
        let shells: Vec<Entry> = shells()
            .into_iter()
            .map(|path| {
                let name = file_name(&path);
                add(&mut picks, Item::new(name), Pick::Shell(path))
            })
            .collect();
        entries.push(Entry::Submenu {
            label: "New Shell".into(),
            entries: shells,
        });
        // Saved layouts, commands and snippets (⌘S saves one).
        let mut recipes: Vec<Entry> = crate::recipes::list()
            .into_iter()
            .map(|saved| {
                add(
                    &mut picks,
                    Item::new(saved.recipe.recipe.name.clone()),
                    Pick::Action(Box::new(ApplyRecipe { id: saved.id })),
                )
            })
            .collect();
        if recipes.is_empty() {
            recipes.push(add(
                &mut picks,
                Item::new("No Recipes").enabled(false),
                Pick::None,
            ));
        }
        recipes.push(Entry::Separator);
        recipes.push(add(
            &mut picks,
            Item::new("Save Recipe…").keystroke(key(&SaveRecipe)),
            Pick::Action(Box::new(SaveRecipe)),
        ));
        entries.push(Entry::Submenu {
            label: "Recipe".into(),
            entries: recipes,
        });
        entries.push(Entry::Separator);

        entries.push(add(
            &mut picks,
            Item::new("New File Editor"),
            Pick::Action(Box::new(NewFileEditor)),
        ));
        for title in ["New File Browser", "New Web Browser…"] {
            entries.push(add(&mut picks, Item::new(title).enabled(false), Pick::None));
        }
        entries.push(add(&mut picks, Item::new("Open…"), Pick::Open));
        entries.push(add(
            &mut picks,
            Item::new("Open Quickly…").keystroke(key(&OpenQuickly)),
            Pick::Action(Box::new(OpenQuickly)),
        ));
        entries.push(Entry::Separator);

        let home = dirs::home_dir().unwrap_or_default();
        let mut recent: Vec<Entry> = crate::folders::ranked()
            .into_iter()
            .take(RECENT)
            .map(|path| {
                let label = match path.strip_prefix(&home) {
                    Ok(rest) => format!("~/{}", rest.display()),
                    Err(_) => path.display().to_string(),
                };
                add(&mut picks, Item::new(label), Pick::Folder(path))
            })
            .collect();
        if recent.is_empty() {
            recent.push(add(
                &mut picks,
                Item::new("No Recent Folders").enabled(false),
                Pick::None,
            ));
        }
        entries.push(Entry::Submenu {
            label: "Open Recent".into(),
            entries: recent,
        });
        let mut hosts: Vec<Entry> = ssh_hosts()
            .into_iter()
            .map(|host| add(&mut picks, Item::new(host.clone()), Pick::Host(host)))
            .collect();
        if hosts.is_empty() {
            hosts.push(add(
                &mut picks,
                Item::new("No Hosts in ~/.ssh/config").enabled(false),
                Pick::None,
            ));
        }
        entries.push(Entry::Submenu {
            label: "Connect to Host".into(),
            entries: hosts,
        });
        entries.push(Entry::Separator);

        entries.push(add(
            &mut picks,
            Item::new("Insert Divider"),
            Pick::InsertDivider,
        ));
        let any_divider = self.tabs.iter().any(|t| t.divider);
        entries.push(add(
            &mut picks,
            Item::new("Remove All Dividers").enabled(any_divider),
            Pick::RemoveDividers,
        ));

        // Otty's menu hangs from the button group's left edge.
        let at = point(
            px(self.titlebar_group_left()),
            px(titlebar::TOP + titlebar::BUTTON + 1.),
        );
        self.new_menu_open = true;
        cx.notify();
        let this = cx.entity().downgrade();
        native_menu::popup_then(entries, at, None, window, cx, move |picked, window, cx| {
            let _ = this.update(cx, |this, cx| {
                this.new_menu_open = false;
                cx.notify();
                if let Some(pick) = picked.and_then(|ix| picks.into_iter().nth(ix)) {
                    this.run_new_menu_pick(pick, window, cx);
                }
            });
        });
    }

    fn run_new_menu_pick(&mut self, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        let result = match pick {
            Pick::None => Ok(()),
            Pick::Action(action) => {
                window.dispatch_action(action, cx);
                Ok(())
            }
            Pick::Home => self.add_tab(None, dirs::home_dir(), window, cx),
            Pick::Folder(path) => self.add_tab(None, Some(path), window, cx),
            Pick::Shell(path) => self.new_shell_tab(&path, window, cx),
            Pick::Host(host) => self.connect_to_host(&host, window, cx),
            Pick::Open => {
                self.open_from_finder(window, cx);
                Ok(())
            }
            Pick::InsertDivider => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.divider = true;
                }
                cx.notify();
                Ok(())
            }
            Pick::RemoveDividers => {
                for tab in &mut self.tabs {
                    tab.divider = false;
                }
                cx.notify();
                Ok(())
            }
        };
        if let Err(e) = result {
            tracing::error!("failed to open a tab: {e:#}");
        }
    }

    /// A tab running this shell (with integration) instead of the
    /// configured one.
    pub(super) fn new_shell_tab(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let configured = self.config.shell.replace(path.display().to_string());
        let result = self.add_tab(None, None, window, cx);
        self.config.shell = configured;
        result
    }

    /// A shell tab that runs `ssh host` first, so a failed or finished
    /// connection leaves the shell (and the error) behind.
    pub(super) fn connect_to_host(
        &mut self,
        host: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.add_tab(None, dirs::home_dir(), window, cx)?;
        if let Some(view) = self.focused_pane() {
            let line = format!("ssh {}\r", shell_quote(host));
            view.update(cx, |view, cx| view.write_input(&line, cx));
        }
        Ok(())
    }

    /// Open…: a folder opens a terminal there; a file, an editor.
    pub(super) fn open_from_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if !path.is_dir() {
                    return this.open_editor(Some(path), window, cx);
                }
                if let Err(e) = this.add_tab(None, Some(path), window, cx) {
                    tracing::error!("failed to open a tab: {e:#}");
                }
            });
        })
        .detach();
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The login shells in /etc/shells, one per name, by name.
pub(crate) fn shells() -> Vec<PathBuf> {
    let text = std::fs::read_to_string("/etc/shells").unwrap_or_default();
    parse_shells(&text)
        .into_iter()
        .filter(|p| p.exists())
        .collect()
}

fn parse_shells(text: &str) -> Vec<PathBuf> {
    let mut shells: Vec<PathBuf> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('/'))
        .map(PathBuf::from)
        .collect();
    shells.sort_by_key(|p| file_name(p));
    shells.dedup_by_key(|p| file_name(p));
    shells
}

/// The `Host` names in ~/.ssh/config (and the files it includes), in file
/// order, without patterns.
pub(crate) fn ssh_hosts() -> Vec<String> {
    let Some(dir) = dirs::home_dir().map(|h| h.join(".ssh")) else {
        return Vec::new();
    };
    let mut hosts = Vec::new();
    collect_hosts(&dir.join("config"), &dir, &mut hosts, 0);
    let mut seen = std::collections::HashSet::new();
    hosts.retain(|h| seen.insert(h.clone()));
    hosts
}

fn collect_hosts(file: &Path, dir: &Path, hosts: &mut Vec<String>, depth: usize) {
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    for (keyword, args) in ssh_lines(&text) {
        match keyword.as_str() {
            "host" => hosts.extend(args.into_iter().filter(|a| !a.contains(['*', '?', '!']))),
            "include" if depth < 4 => {
                for pattern in args {
                    let pattern = match pattern.strip_prefix("~/") {
                        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
                        None if pattern.starts_with('/') => PathBuf::from(pattern),
                        None => dir.join(pattern),
                    };
                    for path in glob_files(&pattern) {
                        collect_hosts(&path, dir, hosts, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }
}

/// (lowercased keyword, arguments) for each line of an ssh config.
fn ssh_lines(text: &str) -> Vec<(String, Vec<String>)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            // `Keyword value`, `Keyword=value` or `Keyword = value`.
            let split = line.find(|c: char| c.is_whitespace() || c == '=')?;
            let (keyword, rest) = line.split_at(split);
            let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '=');
            let args = rest
                .split_whitespace()
                .map(|a| a.trim_matches('"').to_owned())
                .filter(|a| !a.is_empty())
                .collect();
            Some((keyword.to_ascii_lowercase(), args))
        })
        .collect()
}

/// Files matching `pattern`, where only the last component may hold `*`.
fn glob_files(pattern: &Path) -> Vec<PathBuf> {
    let name = pattern
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.contains('*') {
        return vec![pattern.to_path_buf()];
    }
    let Some(parent) = pattern.parent() else {
        return Vec::new();
    };
    let (prefix, suffix) = name.split_once('*').unwrap_or((&name, ""));
    let mut files: Vec<PathBuf> = std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = file_name(p);
            p.is_file() && n.starts_with(prefix) && n.ends_with(suffix)
        })
        .collect();
    files.sort();
    files
}

/// `s` as one shell word.
fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.@:%+/".contains(c))
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shells_by_name_without_duplicates() {
        let text = "# comment\n/bin/zsh\n/bin/bash\n/usr/local/bin/bash\n\n/bin/sh\n";
        let names: Vec<String> = parse_shells(text).iter().map(|p| file_name(p)).collect();
        assert_eq!(names, ["bash", "sh", "zsh"]);
    }

    #[test]
    fn ssh_config_lines() {
        let text = "Host a b *.x\n  HostName 1.2.3.4\nHost=c\n# Host d\nInclude config.d/*\n";
        let lines = ssh_lines(text);
        assert_eq!(
            lines[0],
            ("host".into(), vec!["a".into(), "b".into(), "*.x".into()])
        );
        assert_eq!(lines[2], ("host".into(), vec!["c".into()]));
        assert_eq!(lines[3].0, "include");
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("user@host"), "user@host");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
