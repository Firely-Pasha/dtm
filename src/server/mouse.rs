//! Mouse handling: sidebar clicks, pane focus, wheel scrolling, and
//! forwarding to applications that asked for mouse reports.

use super::copy::CopyMode;
use super::layout::PaneId;
use super::sidebar::{self, Line};
use super::state::ClientId;
use super::Server;
use crate::config::{Position, SidebarMode};
use crate::keys::{MouseEvent, MouseKind, ALT, CTRL, SHIFT};
use crate::term::screen::MouseTracking;

/// Lines scrolled per wheel notch.
const WHEEL_LINES: isize = 3;

/// Encode a mouse report for an application, using pane-relative 0-based coordinates.
pub fn encode_mouse(m: &MouseEvent, rx: usize, ry: usize, sgr: bool) -> Option<Vec<u8>> {
    let mods = (if m.mods & SHIFT != 0 { 4 } else { 0 })
        | (if m.mods & ALT != 0 { 8 } else { 0 })
        | (if m.mods & CTRL != 0 { 16 } else { 0 });
    let b = mods
        + match m.kind {
            MouseKind::Press => m.button as usize,
            MouseKind::Release => if sgr { m.button as usize } else { 3 },
            MouseKind::Drag => m.button as usize + 32,
            MouseKind::Move => 35,
            MouseKind::WheelUp => 64,
            MouseKind::WheelDown => 65,
        };
    if sgr {
        let end = if m.kind == MouseKind::Release { 'm' } else { 'M' };
        Some(format!("\x1b[<{};{};{}{}", b, rx + 1, ry + 1, end).into_bytes())
    } else if rx + 33 > 255 || ry + 33 > 255 || b + 32 > 255 {
        None
    } else {
        Some(vec![0x1b, b'[', b'M', (32 + b) as u8, (33 + rx) as u8, (33 + ry) as u8])
    }
}

impl Server {
    pub fn handle_mouse(&mut self, cid: ClientId, m: MouseEvent) {
        let Some(c) = self.clients.get(&cid) else { return };
        let Some(si) = c.session.and_then(|sid| self.sessions.iter().position(|s| s.id == sid)) else { return };
        if c.sidebar_max && c.sidebar_focus.is_some() {
            return self.sidebar_click(cid, si, m);
        }
        if let Some(p) = self.sessions[si].maximized.filter(|p| self.panes.contains_key(p)) {
            self.pane_mouse(p, &m, m.x as usize, m.y as usize);
            self.mark_dirty();
            return;
        }
        let sw = self.sidebar_width(c) as usize;
        let dw = self.dock_width(c, &self.sessions[si]) as usize;
        let cols = c.cols as usize;
        let (content_x, sidebar_x, dock_x) = match self.cfg.sidebar.position {
            Position::Left => (sw, 0, cols - dw),
            Position::Right => (dw, cols - sw, 0),
        };
        let cw = cols - sw - dw;
        let (x, y) = (m.x as usize, m.y as usize);
        if sw > 0 && x >= sidebar_x && x < sidebar_x + sw {
            return self.sidebar_click(cid, si, m);
        }
        if self.sessions[si].shown_popup().is_some() {
            return self.popup_mouse(si, content_x, m);
        }
        if dw > 0 && x >= dock_x && x < dock_x + dw {
            return self.dock_mouse(si, dock_x, dw, m);
        }
        if x < content_x || x >= content_x + cw {
            return;
        }
        let (cx, cy) = (x - content_x, y);
        let s = &self.sessions[si];
        if cx >= s.size.0 as usize || cy >= s.size.1 as usize {
            return;
        }
        let hit = s.active_window().rects(s.size).into_iter().find(|(_, r)| {
            r.w > 0
                && r.h > 0
                && (r.x as usize..(r.x + r.w) as usize).contains(&cx)
                && (r.y as usize..(r.y + r.h) as usize).contains(&cy)
        });
        let Some((pane_id, r)) = hit else { return };
        let (rx, ry) = (cx - r.x as usize, cy - r.y as usize);
        if m.kind == MouseKind::Press {
            self.sessions[si].dock.focused = false;
            if self.sessions[si].active_window().active != pane_id {
                self.sessions[si].active_window_mut().focus(pane_id);
                let wi = self.sessions[si].active;
                self.refresh_name(si, wi);
            }
        }
        self.pane_mouse(pane_id, &m, rx, ry);
        self.mark_dirty();
    }

    /// Events over the popup go to it; everything else is ignored while it is up.
    fn popup_mouse(&mut self, si: usize, content_x: usize, m: MouseEvent) {
        let s = &self.sessions[si];
        let Some(pane) = s.shown_popup() else { return };
        let inner = self.popup_inner(s.size);
        let (x, y) = (m.x as usize, m.y as usize);
        let (ix, iy) = (content_x + inner.x as usize, inner.y as usize);
        if x >= ix && y >= iy && x < ix + inner.w as usize && y < iy + inner.h as usize {
            self.pane_mouse(pane, &m, x - ix, y - iy);
            self.mark_dirty();
        }
    }

    fn dock_mouse(&mut self, si: usize, dock_x: usize, dw: usize, m: MouseEvent) {
        let sep_left = self.cfg.sidebar.position == Position::Left;
        let px = if sep_left { dock_x + 1 } else { dock_x };
        let (x, y) = (m.x as usize, m.y as usize);
        if x < px || x >= px + dw - 1 {
            return;
        }
        let dock = &self.sessions[si].dock;
        if y == 0 {
            if m.kind == MouseKind::Press && m.button == 0 {
                let names: Vec<&str> = dock.entries.iter().map(|e| e.name.as_str()).collect();
                let hit = super::render::dock_tabs(&names, dock.active, dw - 1)
                    .into_iter()
                    .find(|&(_, x0, x1)| (px + x0..px + x1).contains(&x))
                    .map(|t| t.0);
                if let Some(i) = hit {
                    let d = &mut self.sessions[si].dock;
                    d.active = i;
                    d.focused = true;
                    self.mark_dirty();
                }
            }
            return;
        }
        let Some(pane) = self.sessions[si].shown_dock_pane() else { return };
        if m.kind == MouseKind::Press {
            self.sessions[si].dock.focused = true;
        }
        self.pane_mouse(pane, &m, x - px, y - 1);
        self.mark_dirty();
    }

    fn pane_mouse(&mut self, pane_id: PaneId, m: &MouseEvent, rx: usize, ry: usize) {
        let Some(p) = self.panes.get_mut(&pane_id) else { return };
        let modes = p.screen.modes();
        let wheel = matches!(m.kind, MouseKind::WheelUp | MouseKind::WheelDown);
        let up = m.kind == MouseKind::WheelUp;
        if let Some(copy) = p.copy.as_mut() {
            if wheel {
                let live = copy.scroll(if up { WHEEL_LINES } else { -WHEEL_LINES }, &p.screen);
                if live && !up {
                    p.copy = None;
                }
            } else if m.kind == MouseKind::Press && m.button == 0 {
                copy.cx = rx;
                copy.cy = ry;
                copy.clamp(&p.screen);
            }
            return;
        }
        let wanted = match m.kind {
            MouseKind::Move => modes.mouse == MouseTracking::Any,
            MouseKind::Drag => matches!(modes.mouse, MouseTracking::Drag | MouseTracking::Any),
            _ => modes.mouse != MouseTracking::Off,
        };
        if wanted {
            if let Some(bytes) = encode_mouse(m, rx, ry, modes.mouse_sgr) {
                p.pty.write(bytes);
            }
        } else if wheel && modes.mouse == MouseTracking::Off {
            if modes.alt_screen {
                // Full-screen apps without mouse support: scroll with arrow keys.
                let key: &[u8] = match (up, modes.app_cursor) {
                    (true, true) => b"\x1bOA",
                    (true, false) => b"\x1b[A",
                    (false, true) => b"\x1bOB",
                    (false, false) => b"\x1b[B",
                };
                p.pty.write(key.repeat(WHEEL_LINES as usize));
            } else if up {
                let mut copy = CopyMode::new(&p.screen);
                copy.scroll(WHEEL_LINES, &p.screen);
                p.copy = Some(copy);
            }
        }
    }

    fn sidebar_click(&mut self, cid: ClientId, si: usize, m: MouseEvent) {
        if m.kind != MouseKind::Press || m.button != 0 {
            return;
        }
        let Some(c) = self.clients.get(&cid) else { return };
        let s = &self.sessions[si];
        let rows = self.sidebar_rows(c, s);
        let lines = self.sidebar_lines(c, s);
        let active_row = match c.sidebar_mode {
            SidebarMode::Windows => rows.iter().position(|&i| i == s.active).unwrap_or(0),
            SidebarMode::Sessions => si,
        };
        let active = lines.iter().position(|&l| l == Line::Row(active_row)).unwrap_or(0);
        let has_footer = self.footer_visible(c);
        let Some(at) = sidebar::item_at(lines.len(), active, c.rows as usize, has_footer, m.y as usize) else { return };
        let mode = c.sidebar_mode;
        let row = match lines[at] {
            Line::Row(row) => row,
            Line::Gap => return,
            // A group's heading opens its first listed window.
            Line::Group(gi) => rows.iter().position(|&i| s.group_of(i) == gi).unwrap_or(0),
        };
        let idx = rows[row];
        if let Some(c) = self.clients.get_mut(&cid)
            && c.sidebar_focus.is_some() {
                c.sidebar_focus = Some(row);
            }
        match mode {
            SidebarMode::Windows => {
                let sid = self.sessions[si].id;
                self.open_window(sid, idx);
            }
            SidebarMode::Sessions => {
                let id = self.sessions[idx].id;
                self.switch_client(cid, id);
                self.focus_shown(id);
            }
        }
        self.mark_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: MouseKind, button: u8, mods: u8) -> MouseEvent {
        MouseEvent { kind, button, x: 0, y: 0, mods }
    }

    #[test]
    fn encodes_sgr() {
        assert_eq!(encode_mouse(&ev(MouseKind::Press, 0, 0), 4, 9, true).unwrap(), b"\x1b[<0;5;10M");
        assert_eq!(encode_mouse(&ev(MouseKind::Release, 2, 0), 0, 0, true).unwrap(), b"\x1b[<2;1;1m");
        assert_eq!(encode_mouse(&ev(MouseKind::WheelUp, 0, 0), 1, 1, true).unwrap(), b"\x1b[<64;2;2M");
        assert_eq!(encode_mouse(&ev(MouseKind::Drag, 0, CTRL), 0, 0, true).unwrap(), b"\x1b[<48;1;1M");
    }

    #[test]
    fn encodes_legacy_x10() {
        assert_eq!(encode_mouse(&ev(MouseKind::Press, 0, 0), 0, 0, false).unwrap(), b"\x1b[M !!");
        assert_eq!(encode_mouse(&ev(MouseKind::Release, 1, 0), 2, 3, false).unwrap(), b"\x1b[M##$");
        assert!(encode_mouse(&ev(MouseKind::Press, 0, 0), 300, 0, false).is_none());
    }
}
