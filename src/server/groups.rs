//! Window groups: creating, renaming and tagging them, picking a window by
//! group and window tag, and moving windows between groups.

use super::state::{ClientId, Group, GroupId, SessionId};
use super::Server;
use crate::keys::{format_key, Key};

impl Server {
    /// Why `tag` cannot tag a group among `groups` (other than `except`), if it cannot.
    pub(super) fn tag_conflict(&self, groups: &[Group], tag: char, except: Option<GroupId>) -> Option<String> {
        if !tag.is_ascii_alphabetic() {
            return Some("a group tag is one letter".into());
        }
        if self.cfg.prefix_keys.contains_key(&(Key::Char(tag), 0)) {
            return Some(format!("{} {tag} is already bound", format_key(self.cfg.prefix)));
        }
        groups
            .iter()
            .find(|g| g.tag == tag && Some(g.id) != except)
            .map(|g| format!("tag {tag} is taken by group {}", g.name))
    }

    /// A tag no key binding or other group uses: a letter of `name` if one is free.
    pub(super) fn free_tag(&self, groups: &[Group], name: &str) -> char {
        let own = name.chars().filter(char::is_ascii_alphabetic).map(|c| c.to_ascii_lowercase());
        own.chain('a'..='z')
            .chain('A'..='Z')
            .find(|&t| self.tag_conflict(groups, t, None).is_none())
            .unwrap_or('?')
    }

    /// Start a group named `name` holding one new window, and open it.
    pub fn new_group(&mut self, cid: ClientId, sid: SessionId, name: &str) -> Result<(), String> {
        let si = self.session_index(sid).ok_or("no session")?;
        let name = match name.trim() {
            "" => {
                let groups = &self.sessions[si].groups;
                (2..).map(|i| i.to_string()).find(|n| !groups.iter().any(|g| &g.name == n)).unwrap()
            }
            n => n.to_string(),
        };
        let cwd = self.active_cwd(sid);
        let tag = self.free_tag(&self.sessions[si].groups, &name);
        let gid = self.sessions[si].add_group(name.clone(), tag);
        let size = self.sessions[si].size;
        let window = match self.make_window(cwd, size, gid) {
            Ok(w) => w,
            Err(e) => {
                self.sessions[si].normalize_groups();
                return Err(e);
            }
        };
        self.setup_pane(sid, window.active, None);
        let s = &mut self.sessions[si];
        s.windows.push(window);
        let wi = s.windows.len() - 1;
        self.refresh_name(si, wi);
        self.open_window(sid, wi);
        self.message(cid, format!("group {name}: tag {tag}"));
        Ok(())
    }

    pub(super) fn rename_group(&mut self, sid: SessionId, gid: GroupId, name: String) {
        if name.is_empty() {
            return;
        }
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) else { return };
        if let Some(g) = s.groups.iter_mut().find(|g| g.id == gid) {
            g.name = name;
        }
    }

    pub(super) fn set_group_tag(&mut self, sid: SessionId, gid: GroupId, text: &str) -> Result<(), String> {
        let si = self.session_index(sid).ok_or("no session")?;
        let mut chars = text.chars();
        let (Some(tag), None) = (chars.next(), chars.next()) else { return Err("a group tag is one letter".into()) };
        if let Some(why) = self.tag_conflict(&self.sessions[si].groups, tag, Some(gid)) {
            return Err(why);
        }
        if let Some(g) = self.sessions[si].groups.iter_mut().find(|g| g.id == gid) {
            g.tag = tag;
        }
        Ok(())
    }

    /// The group of the client's session tagged `tag`.
    pub(super) fn group_tagged(&self, cid: ClientId, tag: char) -> Option<GroupId> {
        let sid = self.clients.get(&cid)?.session?;
        let s = self.sessions.iter().find(|s| s.id == sid)?;
        s.groups.iter().find(|g| g.tag == tag).map(|g| g.id)
    }

    pub(super) fn group_name(&self, cid: ClientId, gid: GroupId) -> String {
        let sid = self.clients.get(&cid).and_then(|c| c.session);
        let s = self.sessions.iter().find(|s| Some(s.id) == sid);
        s.and_then(|s| s.groups.iter().find(|g| g.id == gid)).map(|g| g.name.clone()).unwrap_or_default()
    }

    /// Open the window tagged `tag` in group `gid`.
    pub(super) fn open_tagged(&mut self, cid: ClientId, gid: GroupId, tag: char) {
        let Some(sid) = self.clients.get(&cid).and_then(|c| c.session) else { return };
        let Some(s) = self.sessions.iter().find(|s| s.id == sid) else { return };
        let gi = s.group_index(gid);
        match gi.and_then(|gi| s.tagged_window(gi, tag)) {
            Some(wi) => self.open_window(sid, wi),
            None => {
                let name = gi.map(|gi| s.groups[gi].name.clone()).unwrap_or_default();
                self.message(cid, format!("no window {tag} in group {name}"));
            }
        }
    }

    pub(super) fn move_window(&mut self, sid: SessionId, from: usize, down: bool) -> Option<usize> {
        self.sessions.iter_mut().find(|s| s.id == sid)?.move_window(from, down)
    }
}
