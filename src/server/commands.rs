//! Running parsed commands (the `:` prompt, `dtm run`) and completing them as they are typed.

use std::path::PathBuf;

use super::actions::Trigger;
use super::layout::Axis;
use super::state::{ClientId, SessionId};
use super::Server;
use crate::action::ACTION_NAMES;
use crate::command::{Command, COMMAND_NAMES};
use crate::project;

/// Commands remembered for Up/Down in the `:` prompt.
const HISTORY: usize = 100;

impl Server {
    /// A directory typed in a command: `~` and relative paths resolve from the shown pane's directory.
    fn resolve_dir(&self, sid: SessionId, text: &str) -> PathBuf {
        let base = self.active_cwd(sid).unwrap_or_default();
        project::expand(text, &base, &project::home_dir())
    }

    /// Run `cmd` in session `sid`; `cid` is the client it was typed in, when there is one.
    pub fn run_command(&mut self, cid: Option<ClientId>, sid: SessionId, cmd: Command) -> Result<(), String> {
        let si = self.session_index(sid).ok_or("no session")?;
        match cmd {
            Command::Action(action) => {
                let cid = cid.ok_or_else(|| format!("no client attached to {}", self.sessions[si].name))?;
                self.run_action(cid, action, Trigger::key(Vec::new()));
            }
            Command::NewWindow { cwd, name, command } => {
                let cwd = cwd.map(|c| self.resolve_dir(sid, &c));
                self.new_window_in(sid, cwd, name.filter(|n| !n.trim().is_empty()), command)?;
            }
            Command::SplitWindow { side_by_side, cwd, command } => {
                let cwd = cwd.map(|c| self.resolve_dir(sid, &c));
                let axis = if side_by_side { Axis::Cols } else { Axis::Rows };
                self.split_in(sid, axis, cwd, command)?;
            }
            Command::NewSession { name, cwd } => {
                if let Some(n) = &name
                    && self.sessions.iter().any(|s| &s.name == n)
                {
                    return Err(format!("duplicate session: {n}"));
                }
                let cwd = cwd.map(|c| self.resolve_dir(sid, &c)).or_else(|| self.active_cwd(sid));
                let size = match cid {
                    Some(cid) => self.content_size(cid),
                    None => self.sessions[si].size,
                };
                let new = self.create_session(name, cwd, size)?;
                if let Some(cid) = cid {
                    self.switch_client(cid, new);
                }
            }
            Command::KillSession(name) => {
                let id = self.sessions.iter().find(|s| s.name == name).map(|s| s.id).ok_or(format!("no session {name}"))?;
                self.kill_session(id);
            }
            Command::RenameWindow(name) => {
                self.sessions[si].active_window_mut().pinned_name = Some(name);
            }
            Command::RenameSession(name) => {
                if self.sessions.iter().any(|s| s.name == name && s.id != sid) {
                    return Err(format!("duplicate session: {name}"));
                }
                self.sessions[si].name = name;
            }
            Command::SendKeys { pane, bytes } => {
                let id = match pane {
                    Some(p) => p,
                    None => self.focused_pane(si).ok_or("no pane to type into")?,
                };
                let pane = self.panes.get(&id).ok_or_else(|| format!("no such pane: {id}"))?;
                pane.pty.write(bytes);
            }
        }
        self.mark_dirty();
        Ok(())
    }

    /// Parse and run a line typed into the `:` prompt.
    pub fn run_command_line(&mut self, cid: ClientId, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let history = &mut self.command_history;
        if history.last().map(String::as_str) != Some(line) {
            history.push(line.to_string());
            if history.len() > HISTORY {
                history.remove(0);
            }
        }
        let Some(sid) = self.clients.get(&cid).and_then(|c| c.session) else { return };
        let result = Command::parse(line).and_then(|cmd| self.run_command(Some(cid), sid, cmd));
        if let Err(e) = result {
            self.message(cid, e);
        }
    }
}

/// Names that complete `typed` (the first word of a command line), sorted.
pub fn completions(typed: &str) -> Vec<String> {
    let mut names: Vec<String> =
        ACTION_NAMES.iter().chain(COMMAND_NAMES).filter(|n| n.starts_with(typed)).map(|n| n.to_string()).collect();
    names.sort();
    names.dedup();
    names
}

/// The longest start all of `names` share.
pub fn common_prefix(names: &[String]) -> String {
    let Some(first) = names.first() else { return String::new() };
    let mut len = first.len();
    for n in &names[1..] {
        len = len.min(first.bytes().zip(n.bytes()).take_while(|(a, b)| a == b).count());
    }
    first[..len].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_actions_and_commands_by_prefix() {
        assert_eq!(completions("toggle-m"), vec!["toggle-maximize"]);
        assert!(completions("new-").contains(&"new-window".to_string()));
        assert!(completions("new-").contains(&"new-session".to_string()));
        assert!(completions("send-k").contains(&"send-keys".to_string()), "commands complete too");
        assert!(completions("zzz").is_empty());
        let all = completions("");
        assert!(all.windows(2).all(|w| w[0] < w[1]), "sorted and unique");
    }

    #[test]
    fn common_prefix_of_candidates() {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(common_prefix(&v(&["copy-up", "copy-user"])), "copy-u");
        assert_eq!(common_prefix(&v(&["a"])), "a");
        assert_eq!(common_prefix(&v(&[])), "");
        assert_eq!(common_prefix(&v(&["abc", "xyz"])), "");
    }
}
