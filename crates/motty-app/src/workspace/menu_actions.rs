//! Menu bar items that act on the window: Duplicate Tab, New Shell, Connect
//! to Host, Open Folder › Config, Set Tab Prefix, Open in, Notifications &
//! Privileges, Merge All Windows and the agent conversation search.

use gpui::{Context, InteractiveElement};

use super::Workspace;
use super::tabs::Privilege;
use crate::actions::*;
use crate::agents::Badge;

impl Workspace {
    pub(super) fn register_menu_actions<E: InteractiveElement>(
        &self,
        el: E,
        cx: &mut Context<Self>,
    ) -> E {
        el.on_action(cx.listener(|this, _: &DuplicateTab, window, cx| {
            let cwd = this.active_folder(cx);
            if let Err(e) = this.add_tab(None, cwd, window, cx) {
                tracing::error!("failed to duplicate the tab: {e:#}");
            }
        }))
        .on_action(cx.listener(|this, a: &NewShell, window, cx| {
            if let Err(e) = this.new_shell_tab(&a.path, window, cx) {
                tracing::error!("failed to start {}: {e:#}", a.path.display());
            }
        }))
        .on_action(cx.listener(|this, a: &ConnectToHost, window, cx| {
            if let Err(e) = this.connect_to_host(&a.host, window, cx) {
                tracing::error!("failed to open a tab: {e:#}");
            }
        }))
        .on_action(cx.listener(|this, _: &OpenConfigFolder, window, cx| {
            let dir = motty_theme::config::motty_dir();
            let _ = std::fs::create_dir_all(&dir);
            if let Err(e) = this.add_tab(None, Some(dir), window, cx) {
                tracing::error!("failed to open a tab: {e:#}");
            }
        }))
        .on_action(cx.listener(|this, _: &SetTabPrefix, window, cx| {
            this.start_naming(this.active, true, window, cx)
        }))
        .on_action(cx.listener(|this, a: &OpenIn, _, cx| {
            if let Some(cwd) = this.active_folder(cx) {
                super::open_in(&a.bundle, &cwd);
            }
        }))
        .on_action(cx.listener(|this, a: &TogglePrivilege, _, cx| {
            let defaults = this.config.badges;
            let (Some(p), Some(tab)) =
                (Privilege::ALL.get(a.index), this.tabs.get_mut(this.active))
            else {
                return;
            };
            let mut b = tab.privileges.unwrap_or(defaults);
            let on = p.field(&mut b);
            *on = !*on;
            tab.privileges = Some(b);
            this.sync_shell_privileges(cx);
            cx.defer(crate::menus::refresh);
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &UseDefaultPrivileges, _, cx| {
            if let Some(tab) = this.tabs.get_mut(this.active) {
                tab.privileges = None;
            }
            this.sync_shell_privileges(cx);
            cx.defer(crate::menus::refresh);
            cx.notify();
        }))
        .on_action(
            cx.listener(|this, _: &MergeAllWindows, window, cx| this.merge_all_windows(window, cx)),
        )
        .on_action(cx.listener(|this, _: &CloseAll, window, cx| {
            // Each window asks for itself, this one last.
            let here = window.window_handle();
            for handle in cx.windows() {
                if handle == here {
                    continue;
                }
                if let Some(other) = handle.downcast::<Workspace>() {
                    let _ = other.update(cx, |ws, window, cx| {
                        ws.close_asking(super::closing::Closing::Window, window, cx)
                    });
                }
            }
            this.close_asking(super::closing::Closing::Window, window, cx)
        }))
        .on_action(cx.listener(|_, _: &ReloadConfig, _, cx| super::reload_all(cx)))
        .on_action(
            cx.listener(|this, _: &MarkTabUnread, _, cx| this.mark_active(Badge::Finished, cx)),
        )
        .on_action(
            cx.listener(|this, _: &MarkTabProcessing, _, cx| this.mark_active(Badge::Running, cx)),
        )
        .on_action(cx.listener(|this, _: &MarkTabFailed, _, cx| this.mark_active(Badge::Error, cx)))
        .on_action(
            cx.listener(|this, _: &SearchAgentConversations, window, cx| {
                this.show_open_quickly(super::open_quickly::Filter::Agents, window, cx)
            }),
        )
    }

    fn mark_active(&mut self, badge: Badge, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.mark = Some(badge);
            cx.notify();
        }
    }

    /// The active tab's Notifications & Privileges switches, in menu order,
    /// and whether they differ from Settings.
    pub(crate) fn privileges_for_menus(&self) -> Option<(Vec<(&'static str, bool)>, bool)> {
        let tab = self.tabs.get(self.active)?;
        tab.has_terminal().then_some(())?;
        let current = tab.privileges.unwrap_or(self.config.badges);
        let list = Privilege::ALL
            .into_iter()
            .map(|p| (p.label(), p.get(&current)))
            .collect();
        Some((list, tab.privileges.is_some()))
    }
}
