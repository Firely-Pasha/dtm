//! Client input: key decoding, prefix handling, key tables, and prompts.

use std::time::{Duration, Instant};

use super::actions::Trigger;
use crate::action::{Action, Dir};
use super::commands::{common_prefix, completions};
use super::state::{ClientId, Prompt, PromptKind, TabState};
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
        if c.help.is_some() {
            return self.help_key(cid, ev);
        }
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
        // A window that has not started has no pane to type into: `o` or Enter loads it.
        if let Some(sid) = self.unloaded_window(cid) {
            if matches!((ev.key, ev.mods), (Key::Enter, 0) | (Key::Char('o'), 0)) {
                self.focus_shown(sid);
                self.mark_dirty();
            }
            return;
        }
        self.forward(cid, ev.raw);
    }

    /// The client's session, when the window it shows has not started and has the keyboard.
    fn unloaded_window(&self, cid: ClientId) -> Option<super::state::SessionId> {
        let sid = self.clients.get(&cid)?.session;
        let s = self.sessions.iter().find(|s| Some(s.id) == sid)?;
        (s.active_window().pending && self.focused_pane_in(s).is_none()).then_some(s.id)
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
        if p.kind == PromptKind::Command && self.command_prompt_key(cid, &ev) {
            self.mark_dirty();
            return;
        }
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
            Key::Char(ch) if ev.mods & !keys::SHIFT == 0 && p.text.chars().count() < max_prompt(p.kind) => p.text.push(ch),
            _ => {}
        }
        self.mark_dirty();
    }

    /// Tab completion and history in the `:` prompt; true when the key was used up.
    fn command_prompt_key(&mut self, cid: ClientId, ev: &KeyEvent) -> bool {
        let history = self.command_history.clone();
        let Some(p) = self.clients.get_mut(&cid).and_then(|c| c.prompt.as_mut()) else { return false };
        // Choosing from the suggestions under the input: Tab / Down go on, Shift-Tab / Up go back.
        fn step(p: &mut Prompt, forward: bool) {
            let Some(t) = p.tab.as_mut() else { return };
            let n = t.candidates.len();
            let i = match (t.shown, forward) {
                (None, true) => 0,
                (None, false) => n - 1,
                (Some(i), true) => (i + 1) % n,
                (Some(i), false) => (i + n - 1) % n,
            };
            t.shown = Some(i);
            p.text = t.candidates[i].clone();
        }
        let has_suggestions = !p.text.contains(char::is_whitespace) && (p.tab.is_some() || !completions(&p.text).is_empty() && !p.text.is_empty());
        match ev.key {
            Key::Tab if ev.mods == keys::SHIFT && has_suggestions => {
                if p.tab.is_none() {
                    p.tab = Some(TabState { candidates: completions(&p.text), shown: None });
                }
                step(p, false);
                true
            }
            Key::Up | Key::Down if ev.mods == 0 && has_suggestions => {
                if p.tab.is_none() {
                    p.tab = Some(TabState { candidates: completions(&p.text), shown: None });
                }
                step(p, ev.key == Key::Down);
                true
            }
            Key::Tab if ev.mods == 0 => {
                if p.text.contains(char::is_whitespace) {
                    return true;
                }
                match p.tab.as_ref() {
                    Some(_) => step(p, true),
                    None => {
                        let cands = completions(&p.text);
                        match cands.len() {
                            0 => {}
                            1 => p.text = finished(&cands[0]),
                            _ => {
                                // Complete as far as the candidates agree, then cycle on the next press.
                                let common = common_prefix(&cands);
                                let shown = (common.len() <= p.text.len()).then_some(0);
                                p.text = if shown.is_some() { cands[0].clone() } else { common };
                                p.tab = Some(TabState { candidates: cands, shown });
                            }
                        }
                    }
                }
                true
            }
            Key::Up | Key::Down if ev.mods == 0 => {
                p.tab = None;
                match (ev.key, p.history) {
                    (Key::Up, None) => p.history = history.len().checked_sub(1),
                    (Key::Up, Some(i)) => p.history = Some(i.saturating_sub(1)),
                    (_, Some(i)) if i + 1 < history.len() => p.history = Some(i + 1),
                    (_, Some(_)) => {
                        p.history = None;
                        p.text.clear();
                    }
                    _ => {}
                }
                if let Some(h) = p.history.and_then(|i| history.get(i)) {
                    p.text = h.clone();
                }
                true
            }
            _ => {
                p.tab = None;
                false
            }
        }
    }

    /// Keys while the help list is open.
    pub(super) fn help_key(&mut self, cid: ClientId, ev: KeyEvent) {
        // Typing a search: text goes into the query, which filters the list as it changes.
        if let Some(h) = self.clients.get_mut(&cid).and_then(|c| c.help.as_mut()).filter(|h| h.typing) {
            match ev.key {
                Key::Enter => h.typing = false,
                Key::Escape => {
                    h.typing = false;
                    h.query.clear();
                }
                Key::Backspace => {
                    if h.query.pop().is_none() {
                        h.typing = false;
                    }
                }
                Key::Char(ch) if ev.mods & !keys::SHIFT == 0 => h.query.push(ch),
                Key::Up | Key::Down | Key::PageUp | Key::PageDown => {}
                _ => return,
            }
            if !matches!(ev.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown) {
                h.scroll = 0;
                return self.mark_dirty();
            }
        }
        let Some(h) = self.clients.get(&cid).and_then(|c| c.help.as_ref()) else { return };
        let total = self.help_view(h).len();
        let Some(c) = self.clients.get_mut(&cid) else { return };
        let body = super::render::help_body_rows(c.rows as usize);
        let Some(help) = c.help.as_mut() else { return };
        let max = total.saturating_sub(body);
        let mut close = false;
        match ev.key {
            Key::Char('/') if !help.typing => {
                help.typing = true;
                help.scroll = 0;
            }
            Key::Escape if !help.query.is_empty() => {
                help.query.clear();
                help.scroll = 0;
            }
            Key::Char('q' | '?') | Key::Escape => close = true,
            Key::Char('j') | Key::Down => help.scroll += 1,
            Key::Char('k') | Key::Up => help.scroll = help.scroll.saturating_sub(1),
            Key::PageDown | Key::Char(' ') => help.scroll += body,
            Key::PageUp | Key::Char('b') => help.scroll = help.scroll.saturating_sub(body),
            Key::Home | Key::Char('g') => help.scroll = 0,
            Key::End | Key::Char('G') => help.scroll = max,
            Key::Mouse(m) if m.kind == MouseKind::WheelDown => help.scroll += 3,
            Key::Mouse(m) if m.kind == MouseKind::WheelUp => help.scroll = help.scroll.saturating_sub(3),
            _ => {}
        }
        help.scroll = help.scroll.min(max);
        if close {
            c.help = None;
        }
        self.mark_dirty();
    }
}

/// How much a prompt takes: commands are longer than names.
fn max_prompt(kind: PromptKind) -> usize {
    if kind == PromptKind::Command { 256 } else { MAX_PROMPT }
}

/// A completed name, with a space after it when it needs an argument.
fn finished(name: &str) -> String {
    if name == "send-keys" || name.parse::<crate::action::Action>().is_err() { format!("{name} ") } else { name.to_string() }
}
