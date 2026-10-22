//! Client input: key decoding, prefix handling, key tables, and prompts.

use std::time::{Duration, Instant};

use super::actions::Trigger;
use super::state::ClientId;
use super::{Event, Server};
use crate::keys::{self, Key, KeyEvent};

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
        if c.prompt.is_some() {
            return self.prompt_key(cid, ev);
        }
        if let Key::Mouse(m) = ev.key {
            return self.handle_mouse(cid, m);
        }
        if let Some(prefix_raw) = c.prefix.take() {
            self.mark_dirty();
            if let Some(action) = self.cfg.prefix_keys.get(&ev.combo()).cloned() {
                self.run_action(cid, action, Trigger { key: ev.raw.clone(), prefix: Some(prefix_raw) });
            }
            return;
        }
        // For a short while after a resize, plain h/j/k/l keep resizing.
        if c.sidebar_focus.is_none()
            && let Some(until) = c.resize_until {
                if Instant::now() < until
                    && let Some(action) = self.cfg.resize_keys.get(&ev.combo()).cloned() {
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
        self.panes.get(&s.active_window().active).is_some_and(|p| p.copy.is_some())
    }

    fn forward(&mut self, cid: ClientId, raw: Vec<u8>) {
        let Some(sid) = self.clients.get(&cid).and_then(|c| c.session) else { return };
        let Some(s) = self.sessions.iter().find(|s| s.id == sid) else { return };
        if let Some(p) = self.panes.get(&s.active_window().active) {
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
