//! The `<prefix> ?` list of key bindings.

use std::collections::HashMap;

use super::Server;
use crate::action::Action;
use crate::keys::{format_key, KeyCombo};

/// One section: a heading, then `key  action` lines with the same action next to each other.
fn section(out: &mut Vec<String>, title: String, table: &HashMap<KeyCombo, Action>) {
    if table.is_empty() {
        return;
    }
    let mut rows: Vec<(String, String)> = table.iter().map(|(k, a)| (a.to_string(), format_key(*k))).collect();
    rows.sort();
    if !out.is_empty() {
        out.push(String::new());
    }
    out.push(title);
    for (action, key) in rows {
        out.push(format!("  {key:<12}{action}"));
    }
}

/// The lines that match `query` (any case), each under its heading; a heading that matches
/// brings its whole section. Nothing matching gives a single note.
pub fn filter_help(lines: Vec<String>, query: &str) -> Vec<String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return lines;
    }
    let mut out: Vec<String> = Vec::new();
    let mut section: Vec<String> = Vec::new();
    let flush = |section: &mut Vec<String>, out: &mut Vec<String>| {
        let Some(heading) = section.first().cloned() else { return };
        let all = heading.to_lowercase().contains(&q);
        let hits: Vec<&String> = section[1..].iter().filter(|l| all || l.to_lowercase().contains(&q)).collect();
        if !hits.is_empty() || all && section.len() == 1 {
            if !out.is_empty() {
                out.push(String::new());
            }
            out.push(heading);
            out.extend(hits.into_iter().cloned());
        }
        section.clear();
    };
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if !line.starts_with(' ') {
            flush(&mut section, &mut out);
        }
        section.push(line);
    }
    flush(&mut section, &mut out);
    if out.is_empty() {
        out.push("  no match".into());
    }
    out
}

impl Server {
    /// What the help box lists for `help`: the bindings, filtered by its query.
    pub fn help_view(&self, help: &super::state::HelpState) -> Vec<String> {
        filter_help(self.help_lines(), &help.query)
    }

    /// The effective bindings, remaps included. Headings have no indent.
    pub fn help_lines(&self) -> Vec<String> {
        let cfg = &self.cfg;
        let mut out = Vec::new();
        section(&mut out, format!("After the prefix ({})", format_key(cfg.prefix)), &cfg.prefix_keys);
        section(&mut out, "Without the prefix".into(), &cfg.root_keys);
        section(&mut out, "Sidebar, while it has focus".into(), &cfg.sidebar_keys);
        section(&mut out, "Copy mode".into(), &cfg.copy_keys);
        section(&mut out, "For a moment after a resize".into(), &cfg.resize_keys);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn sample() -> Vec<String> {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        v(&["Prefix", "  c           new-window", "  n           next-window", "", "Copy mode", "  y           copy-yank", "  n           copy-search-next"])
    }

    #[test]
    fn filtering_keeps_matching_lines_under_their_headings() {
        let f = |q: &str| filter_help(sample(), q);
        assert_eq!(f(""), sample());
        assert_eq!(f("YANK"), vec!["Copy mode", "  y           copy-yank"], "any case, other sections dropped");
        assert_eq!(f("next"), vec!["Prefix", "  n           next-window", "", "Copy mode", "  n           copy-search-next"]);
        assert_eq!(f("copy mode"), vec!["Copy mode", "  y           copy-yank", "  n           copy-search-next"], "a heading brings its section");
        assert_eq!(f("zzz"), vec!["  no match"]);
    }

    #[test]
    fn sections_list_each_binding_by_action_and_skip_empty_tables() {
        let cfg = Config::from_toml("[keys.prefix]\n\"x\" = \"toggle-maximize\"\n").unwrap();
        let mut out = Vec::new();
        section(&mut out, "Prefix".into(), &cfg.prefix_keys);
        assert_eq!(out[0], "Prefix");
        assert!(out.contains(&format!("  {:<12}{}", "x", "toggle-maximize")), "{out:?}");
        assert!(out.iter().skip(1).all(|l| l.starts_with("  ")), "entries are indented");
        let before = out.len();
        section(&mut out, "Empty".into(), &HashMap::new());
        assert_eq!(out.len(), before);
        // every default binding has a readable action name
        assert!(out.iter().all(|l| !l.contains("Resize(") && !l.contains("Copy(")), "{out:?}");
    }
}
