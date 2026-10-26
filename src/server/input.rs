//! Client input: key decoding, prefix handling, key tables, and prompts.

use std::time::{Duration, Instant};

use super::actions::Trigger;
use crate::action::{Action, Dir};
use super::state::ClientId;
use super::{Event, Server};
use crate::keys::{self, Key, KeyEvent, MouseEvent, MouseKind};

/// How long a lone ESC waits to see whether it starts an escape sequence.
const ESC_TIMEOUT: Duration = Duration::from_millis(25);
const MAX_PROMPT: usize = 64;

impl Server {
    pub fn handle_input(&mut self, cid: ClientId, data: Vec<u8>) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        if c.session.is_none() {
            return;
        }
        c.key_buf.extend_from_slice(&data);
        self.touch(cid);
        self.process_keys(cid, false);
    }

    pub fn process_keys(&mut self, cid: ClientId, flush: bool) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        let events = keys::decode(&mut c.key_buf, flush);
        for ev in events {
            if !self.clients.contains_key(&cid) {
                return;
            }
            self.handle_key(cid, ev);
            // Keys typed right after switching to a lazy window must reach its new panes.
            self.start_visible_windows();
        }
        let Some(c) = self.clients.get_mut(&cid) else { return };
        if !c.key_buf.is_empty() && !c.flush_scheduled {
            c.flush_scheduled = true;
            let tx = self.events.clone();
            tokio::spawn(async move {
                tokio::time::sleep(ESC_TIMEOUT).await;
                let _ = tx.send(Event::KeyFlush { id: cid });
            });
        }
    }

    fn handle_key(&mut self, cid: ClientId, ev: KeyEvent) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        // For a short while after a recent-window step, plain Tab / Shift-Tab keep stepping.
        let combo = ev.combo();
        let repeat = c.prefix.is_none()
            && c.prompt.is_none()
            && c.search.is_none()
            && c.cycle_until.is_some_and(|t| Instant::now() < t)
            && matches!(combo, (Key::Tab, 0) | (Key::Tab, keys::SHIFT));
        if repeat {
            let back = combo.1 == 0;
            self.run_action(cid, Action::RecentWindow(back), Trigger::key(ev.raw.clone()));
            return;
        }
        let table = if c.prefix.is_some() { &self.cfg.prefix_keys } else { &self.cfg.root_keys };
        let steps = matches!(table.get(&combo), Some(Action::RecentWindow(_)));
        if c.cycle.is_some()
            && !steps
            && combo != self.cfg.prefix
            && !matches!(ev.key, Key::Mouse(MouseEvent { kind: MouseKind::Move, .. }))
        {
            self.end_cycle(cid);
        }
        let Some(c) = self.clients.get_mut(&cid) else { return };
        if c.prompt.is_some() {
            return self.prompt_key(cid, ev);
        }
        if let Key::Mouse(m) = ev.key {
            return self.handle_mouse(cid, m);
        }
        // After the prefix and a group's tag, the next key is a window tag in that group.
        if let Some(gid) = c.pending_group.take() {
            c.message = None;
            if let Key::Char(ch) = ev.key
                && ev.mods & !keys::SHIFT == 0
            {
                self.open_tagged(cid, gid, ch);
                self.sidebar_select_shown(cid);
            }
            self.mark_dirty();
            return;
        }
        if c.search.is_some() {
            return self.search_key(cid, ev);
        }
        if let Some(prefix_raw) = c.prefix.take() {
            self.mark_dirty();
            if let Some(action) = self.cfg.prefix_keys.get(&ev.combo()).cloned() {
                self.run_action(cid, action, Trigger { key: ev.raw.clone(), prefix: Some(prefix_raw) });
            } else if let (Key::Char(ch), 0) = ev.combo()
                && let Some(gid) = self.group_tagged(cid, ch)
            {
                let name = self.group_name(cid, gid);
                self.message(cid, format!("group {name}: window?"));
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.pending_group = Some(gid);
                }
            }
            return;
        }
        // For a short while after a resize, plain h/j/k/l keep resizing.
        let sidebar_focused = c.sidebar_focus.is_some();
        if let Some(until) = c.resize_until {
                if Instant::now() < until
                    && let Some(action) = self.cfg.resize_keys.get(&ev.combo()).cloned()
                    // A focused sidebar only repeats widening and narrowing; j/k stay its own.
                    && (!sidebar_focused || matches!(action, Action::Resize(Dir::Left | Dir::Right, _))) {
                        self.run_action(cid, action, Trigger::key(ev.raw.clone()));
                        return;
                    }
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.resize_until = None;
                }
            }
        let Some(c) = self.clients.get_mut(&cid) else { return };
        if ev.combo() == self.cfg.prefix {
            c.prefix = Some(ev.raw);
            return;
        }
        if let Some(action) = self.cfg.root_keys.get(&ev.combo()).cloned() {
            self.run_action(cid, action, Trigger::key(ev.raw.clone()));
            return;
        }
        if self.clients.get(&cid).is_some_and(|c| c.sidebar_focus.is_some()) {
            // The focused sidebar swallows keys it has no binding for.
            if let Some(action) = self.cfg.sidebar_keys.get(&ev.combo()).cloned() {
                self.run_action(cid, action, Trigger::key(ev.raw.clone()));
            }
            return;
        }
        if self.active_pane_in_copy_mode(cid) {
            // Copy mode swallows keys it has no binding for.
            if let Some(action) = self.cfg.copy_keys.get(&ev.combo()).cloned() {
                self.run_action(cid, action, Trigger::key(ev.raw.clone()));
            }
            return;
        }
        self.forward(cid, ev.raw);
    }

    fn active_pane_in_copy_mode(&self, cid: ClientId) -> bool {
        let sid = self.clients.get(&cid).and_then(|c| c.session);
        let Some(s) = sid.and_then(|sid| self.sessions.iter().find(|s| s.id == sid)) else { return false };
        self.focused_pane_in(s).and_then(|id| self.panes.get(&id)).is_some_and(|p| p.copy.is_some())
    }

    fn forward(&mut self, cid: ClientId, raw: Vec<u8>) {
        let Some(sid) = self.clients.get(&cid).and_then(|c| c.session) else { return };
        let Some(s) = self.sessions.iter().find(|s| s.id == sid) else { return };
        if let Some(p) = self.focused_pane_in(s).and_then(|id| self.panes.get(&id)) {
            p.pty.write(raw);
        }
    }

    fn prompt_key(&mut self, cid: ClientId, ev: KeyEvent) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        let Some(p) = c.prompt.as_mut() else { return };
        if p.kind.is_confirm() {
            if matches!(ev.key, Key::Mouse(_)) {
                return;
            }
            let yes = matches!(ev.key, Key::Char('y' | 'Y'));
            let p = c.prompt.take().unwrap();
            if yes {
                self.commit_prompt(cid, p.kind, p.text);
            }
            self.mark_dirty();
            return;
        }
        match ev.key {
            Key::Enter => {
                let p = c.prompt.take().unwrap();
                self.commit_prompt(cid, p.kind, p.text);
            }
            Key::Escape => c.prompt = None,
            Key::Backspace => {
                p.text.pop();
            }
            Key::Char(ch) if ev.mods & !keys::SHIFT == 0 && p.text.chars().count() < MAX_PROMPT => p.text.push(ch),
            _ => {}
        }
        self.mark_dirty();
    }
}
