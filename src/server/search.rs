//! Window search: filter the sidebar's windows by typing.

use super::state::{ClientId, Search};
use super::Server;
use crate::config::SidebarMode;
use crate::keys::{self, Key, KeyEvent};

impl Server {
    /// Focus the sidebar in windows mode and start filtering.
    pub fn start_search(&mut self, cid: ClientId) -> Result<(), String> {
        if let Some(c) = self.clients.get_mut(&cid) {
            c.sidebar_mode = SidebarMode::Windows;
        }
        self.enter_sidebar(cid)?;
        let Some(c) = self.clients.get(&cid) else { return Ok(()) };
        let Some(si) = c.session.and_then(|sid| self.session_index(sid)) else { return Ok(()) };
        let origin = self.sessions[si].active;
        if let Some(c) = self.clients.get_mut(&cid) {
            c.search = Some(Search { query: String::new(), origin });
        }
        Ok(())
    }

    /// The rows now shown, and the client's session.
    fn search_rows(&self, cid: ClientId) -> Option<(super::state::SessionId, Vec<usize>)> {
        let c = self.clients.get(&cid)?;
        let s = self.sessions.iter().find(|s| Some(s.id) == c.session)?;
        Some((s.id, self.sidebar_rows(c, s)))
    }

    /// Show the window at the selected row, when the sidebar follows the selection.
    fn search_follow(&mut self, cid: ClientId) {
        let Some((sid, rows)) = self.search_rows(cid) else { return };
        let sel = self.clients.get(&cid).and_then(|c| c.sidebar_focus).unwrap_or(0);
        // Only windows that are already loaded are previewed; Enter starts the others.
        if self.cfg.sidebar.follow
            && let Some(&wi) = rows.get(sel)
            && self.sessions.iter().find(|s| s.id == sid).is_some_and(|s| !s.windows[wi].pending)
        {
            self.select_window(sid, wi);
        }
    }

    fn search_move(&mut self, cid: ClientId, delta: isize) {
        let Some((_, rows)) = self.search_rows(cid) else { return };
        if let Some(c) = self.clients.get_mut(&cid) {
            let sel = c.sidebar_focus.unwrap_or(0) as isize + delta;
            c.sidebar_focus = Some(sel.clamp(0, rows.len().saturating_sub(1) as isize) as usize);
        }
        self.search_follow(cid);
    }

    fn search_edit(&mut self, cid: ClientId, edit: impl FnOnce(&mut String)) {
        if let Some(q) = self.clients.get_mut(&cid).and_then(|c| c.search.as_mut()) {
            edit(&mut q.query);
        }
        if let Some(c) = self.clients.get_mut(&cid) {
            c.sidebar_focus = Some(0);
        }
        self.search_follow(cid);
    }

    /// Open the selected window, or (cancelled) go back to where the search began.
    fn search_end(&mut self, cid: ClientId, open: bool) {
        let Some((sid, rows)) = self.search_rows(cid) else { return };
        let Some(c) = self.clients.get_mut(&cid) else { return };
        let (sel, search) = (c.sidebar_focus.unwrap_or(0), c.search.take());
        if open {
            c.sidebar_focus = None;
            if let Some(&wi) = rows.get(sel) {
                self.open_window(sid, wi);
            }
        } else if let Some(origin) = search.map(|s| s.origin) {
            c.sidebar_focus = Some(origin);
            self.select_window(sid, origin);
        }
    }

    pub fn search_key(&mut self, cid: ClientId, ev: KeyEvent) {
        let ctrl = ev.mods & keys::CTRL != 0;
        let empty = self.clients.get(&cid).and_then(|c| c.search.as_ref()).is_none_or(|s| s.query.is_empty());
        match ev.key {
            Key::Enter => self.search_end(cid, true),
            Key::Escape => self.search_end(cid, false),
            Key::Backspace if empty => self.search_end(cid, false),
            Key::Backspace => self.search_edit(cid, |q| {
                q.pop();
            }),
            Key::Up => self.search_move(cid, -1),
            Key::Down => self.search_move(cid, 1),
            Key::Char('p' | 'k') if ctrl => self.search_move(cid, -1),
            Key::Char('n' | 'j') if ctrl => self.search_move(cid, 1),
            Key::Char(ch) if ev.mods & !keys::SHIFT == 0 => self.search_edit(cid, |q| q.push(ch)),
            _ => {}
        }
        self.mark_dirty();
    }
}
