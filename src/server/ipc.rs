//! Requests from programs inside panes (editor plugins) via the `dtm` CLI.

use super::layout::{PaneId, Rect};
use super::Server;
use crate::action::Dir;
use crate::protocol::PaneDir;

/// What a `#{...}` format can say about a pane.
#[derive(Debug, Default, Clone, Copy)]
pub struct FormatCtx {
    pub pane_id: PaneId,
    pub zoomed: bool,
    pub active: bool,
    pub at_left: bool,
    pub at_right: bool,
    pub at_top: bool,
    pub at_bottom: bool,
}

/// Expand `#{name}` (and `##` for a literal `#`). Unknown names expand to nothing.
pub fn expand_format(format: &str, ctx: &FormatCtx) -> String {
    let flag = |b: bool| if b { "1" } else { "0" }.to_string();
    let mut out = String::new();
    let mut rest = format;
    while let Some(i) = rest.find('#') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        if let Some(r) = rest.strip_prefix('#') {
            out.push('#');
            rest = r;
        } else if let Some(r) = rest.strip_prefix('{') {
            let Some(end) = r.find('}') else {
                out.push_str("#{");
                rest = r;
                continue;
            };
            out.push_str(&match &r[..end] {
                "pane_id" => ctx.pane_id.to_string(),
                "window_zoomed_flag" => flag(ctx.zoomed),
                "pane_active" => flag(ctx.active),
                "pane_at_left" => flag(ctx.at_left),
                "pane_at_right" => flag(ctx.at_right),
                "pane_at_top" => flag(ctx.at_top),
                "pane_at_bottom" => flag(ctx.at_bottom),
                _ => String::new(),
            });
            rest = &r[end + 1..];
        } else {
            out.push('#');
        }
    }
    out.push_str(rest);
    out
}

impl Server {
    /// `dtm select-pane`: move focus relative to `pane`, as if its owner had pressed the key.
    pub fn select_pane_ipc(&mut self, pane: PaneId, dir: PaneDir) -> Result<(), String> {
        if let Some((si, extra)) = self.extra_of(pane) {
            // Past the edge of the dock: back to the window, which is on the sidebar's side.
            let toward_window = match dir {
                PaneDir::Left => self.toward_sidebar(Dir::Left),
                PaneDir::Right => self.toward_sidebar(Dir::Right),
                _ => false,
            };
            if toward_window && matches!(extra, super::state::Extra::Dock(_)) {
                self.sessions[si].dock.focused = false;
                self.mark_dirty();
            }
            return Ok(());
        }
        let (si, wi) = self.locate_pane(pane).ok_or_else(|| format!("no such pane: {pane}"))?;
        let dir = match dir {
            PaneDir::Last => {
                self.focus_last_in(si, wi);
                self.mark_dirty();
                return Ok(());
            }
            PaneDir::Left => Dir::Left,
            PaneDir::Right => Dir::Right,
            PaneDir::Up => Dir::Up,
            PaneDir::Down => Dir::Down,
        };
        if !self.focus_dir_from(si, wi, pane, dir) && self.toward_sidebar(dir) && wi == self.sessions[si].active {
            // Past the edge on the sidebar's side: the sidebar takes focus for the user driving this session.
            let sid = self.sessions[si].id;
            let client = self
                .clients
                .values()
                .filter(|c| c.session == Some(sid))
                .max_by_key(|c| c.last_active)
                .map(|c| c.id);
            if let Some(cid) = client {
                self.enter_sidebar(cid)?;
            }
        }
        self.mark_dirty();
        Ok(())
    }

    /// `dtm query`: expand a format for `pane`.
    pub fn query_ipc(&self, pane: PaneId, format: &str) -> Result<String, String> {
        if self.extra_of(pane).is_some() {
            // Dock and popup panes sit at every edge: the plugin always hands over to dtm.
            let ctx = FormatCtx {
                pane_id: pane,
                active: true,
                at_left: true,
                at_right: true,
                at_top: true,
                at_bottom: true,
                ..Default::default()
            };
            return Ok(expand_format(format, &ctx));
        }
        let (si, wi) = self.locate_pane(pane).ok_or_else(|| format!("no such pane: {pane}"))?;
        let s = &self.sessions[si];
        let w = &s.windows[wi];
        let mut rects = Vec::new();
        w.root.layout(Rect::new(0, 0, s.size.0, s.size.1), &mut rects);
        let r = rects.iter().find(|(id, _)| *id == pane).map(|(_, r)| *r).ok_or("pane not in layout")?;
        let ctx = FormatCtx {
            pane_id: pane,
            zoomed: w.zoomed,
            active: w.active == pane,
            at_left: r.x == 0,
            at_right: r.x + r.w == s.size.0,
            at_top: r.y == 0,
            at_bottom: r.y + r.h == s.size.1,
        };
        Ok(expand_format(format, &ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_known_variables() {
        let ctx = FormatCtx { pane_id: 7, zoomed: true, at_left: true, ..Default::default() };
        assert_eq!(expand_format("#{window_zoomed_flag}", &ctx), "1");
        assert_eq!(expand_format("#{pane_at_left}#{pane_at_right}", &ctx), "10");
        assert_eq!(expand_format("pane #{pane_id} active=#{pane_active}", &ctx), "pane 7 active=0");
    }

    #[test]
    fn handles_odd_input() {
        let ctx = FormatCtx::default();
        assert_eq!(expand_format("", &ctx), "");
        assert_eq!(expand_format("plain", &ctx), "plain");
        assert_eq!(expand_format("##{x}", &ctx), "#{x}");
        assert_eq!(expand_format("#{nope}|", &ctx), "|");
        assert_eq!(expand_format("a#b", &ctx), "a#b");
        assert_eq!(expand_format("#{unterminated", &ctx), "#{unterminated");
        assert_eq!(expand_format("end#", &ctx), "end#");
    }
}
