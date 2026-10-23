//! Panes that belong to a session rather than a window: the popup shell and
//! the dock (a side panel of session-wide panes).

use super::layout::{PaneId, Rect};
use super::state::{ClientId, DockEntry, Extra, PaneSpec, Session, SessionId};
use super::Server;
use crate::project;

impl Server {
    /// The pane keys go to: the popup, else the focused dock pane, else the window's.
    pub fn focused_pane_in(&self, s: &Session) -> Option<PaneId> {
        if let Some(p) = s.shown_popup() {
            return Some(p);
        }
        if s.dock.focused
            && let Some(p) = s.shown_dock_pane()
        {
            return Some(p);
        }
        let w = s.active_window();
        (!w.pending).then_some(w.active)
    }

    pub fn focused_pane(&self, si: usize) -> Option<PaneId> {
        self.focused_pane_in(&self.sessions[si])
    }

    /// Is the popup or the dock holding the keyboard?
    pub fn extra_focused(&self, s: &Session) -> bool {
        s.shown_popup().is_some() || (s.dock.focused && s.shown_dock_pane().is_some())
    }

    pub fn extra_of(&self, pane: PaneId) -> Option<(usize, Extra)> {
        self.sessions.iter().enumerate().find_map(|(si, s)| {
            if s.popup == Some(pane) {
                Some((si, Extra::Popup))
            } else {
                s.dock.entries.iter().position(|e| e.pane == Some(pane)).map(|i| (si, Extra::Dock(i)))
            }
        })
    }

    // ---- popup ----

    /// The popup including its border, inside a pane area of `size`.
    pub fn popup_outer(&self, size: (u16, u16)) -> Rect {
        let fit = |s: project::Size, total: u16| project::cells(s, total).clamp(5.min(total), total.max(1));
        let (w, h) = (fit(self.cfg.popup.width, size.0), fit(self.cfg.popup.height, size.1));
        Rect::new((size.0 - w.min(size.0)) / 2, (size.1 - h.min(size.1)) / 2, w, h)
    }

    /// The popup's terminal area, inside its border.
    pub fn popup_inner(&self, size: (u16, u16)) -> Rect {
        let o = self.popup_outer(size);
        Rect::new(o.x + 1, o.y + 1, o.w.saturating_sub(2).max(1), o.h.saturating_sub(2).max(1))
    }

    /// Type the session's `pane_command`, then `own`, into a new pane's shell.
    pub fn setup_pane(&mut self, sid: SessionId, id: PaneId, own: Option<String>) {
        let setup = self.sessions.iter().find(|s| s.id == sid).and_then(|s| s.pane_command.clone());
        let mut text = String::new();
        for line in setup.iter().chain(own.iter()) {
            text.push_str(line);
            text.push('\r');
        }
        if let Some(p) = self.panes.get_mut(&id) {
            if !text.is_empty() {
                p.pty.write(text.into_bytes());
            }
            p.spec.command = own;
        }
    }

    pub fn toggle_popup(&mut self, sid: SessionId) -> Result<(), String> {
        let si = self.session_index(sid).ok_or("no session")?;
        if self.sessions[si].popup.is_some() {
            let s = &mut self.sessions[si];
            s.popup_shown = !s.popup_shown;
            return Ok(());
        }
        let inner = self.popup_inner(self.sessions[si].size);
        let cwd = self.active_cwd(sid);
        let id = self.alloc_pane_id();
        self.spawn_pane(id, cwd, (inner.w, inner.h))?;
        self.setup_pane(sid, id, None);
        let s = &mut self.sessions[si];
        s.popup = Some(id);
        s.popup_shown = true;
        Ok(())
    }

    // ---- dock ----

    fn dock_mut(&mut self, sid: SessionId) -> Result<&mut super::state::Dock, String> {
        let si = self.session_index(sid).ok_or("no session")?;
        Ok(&mut self.sessions[si].dock)
    }

    fn non_empty_dock(&mut self, sid: SessionId) -> Result<&mut super::state::Dock, String> {
        let d = self.dock_mut(sid)?;
        if d.entries.is_empty() {
            return Err("dock is empty: add a pane with the dock-add key".into());
        }
        Ok(d)
    }

    pub fn toggle_dock(&mut self, sid: SessionId) -> Result<(), String> {
        let d = self.non_empty_dock(sid)?;
        d.visible = !d.visible;
        d.focused = d.visible;
        self.update_session_size(sid);
        Ok(())
    }

    pub fn focus_dock(&mut self, sid: SessionId) -> Result<(), String> {
        let d = self.non_empty_dock(sid)?;
        d.visible = true;
        d.focused = true;
        self.update_session_size(sid);
        Ok(())
    }

    pub fn step_dock(&mut self, sid: SessionId, forward: bool) -> Result<(), String> {
        let d = self.non_empty_dock(sid)?;
        let n = d.entries.len();
        d.active = if forward { (d.active + 1) % n } else { (d.active + n - 1) % n };
        d.visible = true;
        self.update_session_size(sid);
        Ok(())
    }

    /// Add a pane running `command` (a plain shell when empty) and focus it.
    pub fn add_dock_entry(&mut self, sid: SessionId, command: &str) -> Result<(), String> {
        let cwd = self.active_cwd(sid);
        let command = Some(command.trim().to_string()).filter(|c| !c.is_empty());
        let name = project::default_pane_name(command.as_deref(), &self.cfg.shell);
        let d = self.dock_mut(sid)?;
        d.entries.push(DockEntry { name, spec: PaneSpec { cwd, command }, pane: None });
        d.active = d.entries.len() - 1;
        d.visible = true;
        d.focused = true;
        self.update_session_size(sid);
        Ok(())
    }

    /// Remove dock entry `idx` and hang up its terminal.
    pub fn remove_dock_entry(&mut self, si: usize, idx: usize) {
        let sid = self.sessions[si].id;
        let d = &mut self.sessions[si].dock;
        if idx >= d.entries.len() {
            return;
        }
        let entry = d.entries.remove(idx);
        if d.active > idx {
            d.active -= 1;
        }
        d.active = d.active.min(d.entries.len().saturating_sub(1));
        if d.entries.is_empty() {
            d.visible = false;
            d.focused = false;
        }
        if let Some(p) = entry.pane {
            self.panes.remove(&p);
        }
        self.update_session_size(sid);
        self.mark_dirty();
    }

    pub fn rename_dock_entry(&mut self, sid: SessionId, name: String) {
        if name.is_empty() {
            return;
        }
        if let Ok(d) = self.dock_mut(sid)
            && let Some(e) = d.entries.get_mut(d.active)
        {
            e.name = name;
        }
    }

    /// Start the dock pane that a watching client can see.
    pub fn start_visible_dock(&mut self) {
        let mut jobs: Vec<(usize, usize)> = Vec::new();
        for (si, s) in self.sessions.iter().enumerate() {
            if !self.clients.values().any(|c| c.session == Some(s.id)) {
                continue;
            }
            if s.dock.visible && s.dock.entries.get(s.dock.active).is_some_and(|e| e.pane.is_none()) {
                jobs.push((si, s.dock.active));
            }
        }
        for (si, idx) in jobs {
            let s = &self.sessions[si];
            let (sid, spec, size) = (s.id, s.dock.entries[idx].spec.clone(), s.dock_size);
            let id = self.alloc_pane_id();
            match self.spawn_pane(id, spec.cwd.clone(), (size.0.max(1), size.1.max(1))) {
                Ok(()) => {
                    self.sessions[si].dock.entries[idx].pane = Some(id);
                    self.setup_pane(sid, id, spec.command);
                    self.mark_dirty();
                }
                Err(e) => {
                    // Drop the entry, or this would be retried on every frame.
                    self.remove_dock_entry(si, idx);
                    self.message_viewers(sid, e);
                }
            }
        }
    }

    pub fn message_viewers(&mut self, sid: SessionId, text: String) {
        let viewers: Vec<ClientId> = self.clients.values().filter(|c| c.session == Some(sid)).map(|c| c.id).collect();
        for cid in viewers {
            self.message(cid, text.clone());
        }
    }
}
