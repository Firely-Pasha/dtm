//! The command language of `<prefix> :` and `dtm run`: any action by name, plus a few
//! commands that take flags (`new-window -c DIR`, `send-keys`, ...).

use crate::action::Action;
use crate::keys::{encode, parse_key};
use crate::project::quote_args;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Action(Action),
    /// `new-window [-c DIR] [-n NAME] [COMMAND]`
    NewWindow { cwd: Option<String>, name: Option<String>, command: Option<String> },
    /// `split-window [-h] [-v] [-c DIR] [COMMAND]`; `-h` puts the panes side by side.
    SplitWindow { side_by_side: bool, cwd: Option<String>, command: Option<String> },
    /// `new-session [-s NAME] [-c DIR]`
    NewSession { name: Option<String>, cwd: Option<String> },
    /// `kill-session NAME`
    KillSession(String),
    /// `rename-window NAME`
    RenameWindow(String),
    /// `rename-session NAME`
    RenameSession(String),
    /// `send-keys [-l] [-t PANE] KEY|TEXT...`, already turned into the bytes to type.
    SendKeys { pane: Option<u32>, bytes: Vec<u8> },
}

/// Names of the commands that are not actions (the `:` prompt completes these too).
pub const COMMAND_NAMES: &[&str] =
    &["new-window", "split-window", "new-session", "kill-session", "rename-window", "rename-session", "send-keys"];

/// Split a command line into words: blanks separate, `'...'` and `"..."` group (`\"` and `\\`
/// work inside double quotes), and a backslash quotes the next character elsewhere.
pub fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err("unterminated ' quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => cur.push(c),
                            Some(c) => {
                                cur.push('\\');
                                cur.push(c);
                            }
                            None => return Err("unterminated \" quote".into()),
                        },
                        Some(c) => cur.push(c),
                        None => return Err("unterminated \" quote".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                cur.push(chars.next().unwrap_or('\\'));
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

/// The flags and the rest of a command's arguments.
struct Opts {
    values: Vec<(String, String)>,
    flags: Vec<String>,
    rest: Vec<String>,
}

impl Opts {
    /// `with_value` flags take the next word; `bare` ones stand alone. The first word that is
    /// not a flag (or comes after `--`) starts the rest.
    fn parse(command: &str, args: &[String], with_value: &[&str], bare: &[&str]) -> Result<Opts, String> {
        let mut o = Opts { values: Vec::new(), flags: Vec::new(), rest: Vec::new() };
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            if a == "--" {
                o.rest = args[i + 1..].to_vec();
                break;
            }
            if with_value.contains(&a) {
                let v = args.get(i + 1).ok_or_else(|| format!("{command}: {a} needs a value"))?;
                o.values.push((a.to_string(), v.clone()));
                i += 2;
            } else if bare.contains(&a) {
                o.flags.push(a.to_string());
                i += 1;
            } else if a.starts_with('-') && a.len() > 1 && a.parse::<f64>().is_err() {
                return Err(format!("{command}: unknown option {a}"));
            } else {
                o.rest = args[i..].to_vec();
                break;
            }
        }
        Ok(o)
    }

    fn value(&self, flag: &str) -> Option<String> {
        self.values.iter().rev().find(|(f, _)| f == flag).map(|(_, v)| v.clone())
    }

    fn flag(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }

    /// The rest as one shell command line: a single word is taken as it is, several are quoted.
    fn command(&self) -> Option<String> {
        match self.rest.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            many => Some(quote_args(many)),
        }
    }
}

impl Command {
    pub fn parse(line: &str) -> Result<Command, String> {
        let words = split_words(line)?;
        let Some((name, args)) = words.split_first() else { return Err("empty command".into()) };
        let name = name.as_str();
        match name {
            "new-window" if !args.is_empty() => {
                let o = Opts::parse(name, args, &["-c", "-n"], &[])?;
                Ok(Command::NewWindow { cwd: o.value("-c"), name: o.value("-n"), command: o.command() })
            }
            "split-window" => {
                let o = Opts::parse(name, args, &["-c"], &["-h", "-v"])?;
                if o.flag("-h") && o.flag("-v") {
                    return Err("split-window: use -h or -v, not both".into());
                }
                Ok(Command::SplitWindow { side_by_side: o.flag("-h"), cwd: o.value("-c"), command: o.command() })
            }
            "new-session" if !args.is_empty() => {
                let o = Opts::parse(name, args, &["-s", "-c"], &[])?;
                if !o.rest.is_empty() {
                    return Err("new-session: unexpected argument".into());
                }
                Ok(Command::NewSession { name: o.value("-s"), cwd: o.value("-c") })
            }
            "kill-session" if !args.is_empty() => Ok(Command::KillSession(args.join(" "))),
            "rename-window" if !args.is_empty() => Ok(Command::RenameWindow(args.join(" "))),
            "rename-session" if !args.is_empty() => Ok(Command::RenameSession(args.join(" "))),
            "send-keys" => {
                let o = Opts::parse(name, args, &["-t"], &["-l"])?;
                let pane = match o.value("-t") {
                    Some(p) => Some(p.parse::<u32>().map_err(|_| format!("send-keys: bad pane {p:?}"))?),
                    None => None,
                };
                if o.rest.is_empty() {
                    return Err("send-keys needs keys or text".into());
                }
                let mut bytes = Vec::new();
                for word in &o.rest {
                    // A word that names a key (Enter, C-c, Up, F5) is that key; anything else is typed.
                    match parse_key(word) {
                        Ok(combo) if !o.flag("-l") => bytes.extend(encode(combo)),
                        _ => bytes.extend(word.as_bytes()),
                    }
                }
                Ok(Command::SendKeys { pane, bytes })
            }
            _ => Ok(Command::Action(line.trim().parse()?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        split_words(line).unwrap()
    }

    #[test]
    fn splits_words_with_quotes_and_escapes() {
        assert_eq!(words("a  b\tc"), ["a", "b", "c"]);
        assert_eq!(words("  "), Vec::<String>::new());
        assert_eq!(words("new-window -n 'my win' \"cargo test\""), ["new-window", "-n", "my win", "cargo test"]);
        assert_eq!(words(r#"echo "say \"hi\" \\ done""#), ["echo", r#"say "hi" \ done"#]);
        assert_eq!(words(r"a\ b c"), ["a b", "c"]);
        assert_eq!(words("''"), [""], "an empty quoted word is a word");
        assert_eq!(words("a'b c'd"), ["ab cd"]);
        assert!(split_words("'open").is_err());
        assert!(split_words("\"open").is_err());
    }

    #[test]
    fn plain_actions_parse_as_actions() {
        assert_eq!(Command::parse("toggle-maximize"), Ok(Command::Action(Action::ToggleMaximize)));
        assert_eq!(Command::parse(" select-window 3 "), Ok(Command::Action(Action::SelectWindow(3))));
        assert_eq!(Command::parse("new-window"), Ok(Command::Action(Action::NewWindow)));
        assert_eq!(Command::parse("kill-session"), Ok(Command::Action(Action::KillSession)));
        assert_eq!(Command::parse("rename-window"), Ok(Command::Action(Action::RenameWindow)));
        assert!(Command::parse("bogus").unwrap_err().contains("unknown action"));
        assert!(Command::parse("").is_err());
    }

    #[test]
    fn new_window_and_split_take_flags_and_a_command() {
        assert_eq!(
            Command::parse("new-window -c ~/code -n api cargo run"),
            Ok(Command::NewWindow { cwd: Some("~/code".into()), name: Some("api".into()), command: Some("cargo run".into()) })
        );
        assert_eq!(
            Command::parse("new-window 'tail -f log'"),
            Ok(Command::NewWindow { cwd: None, name: None, command: Some("tail -f log".into()) })
        );
        assert_eq!(
            Command::parse("new-window -- -weird"),
            Ok(Command::NewWindow { cwd: None, name: None, command: Some("-weird".into()) })
        );
        assert_eq!(
            Command::parse("split-window -h -c /tmp"),
            Ok(Command::SplitWindow { side_by_side: true, cwd: Some("/tmp".into()), command: None })
        );
        assert_eq!(
            Command::parse("split-window"),
            Ok(Command::SplitWindow { side_by_side: false, cwd: None, command: None })
        );
        assert!(Command::parse("split-window -h -v").is_err());
        assert!(Command::parse("new-window -c").unwrap_err().contains("needs a value"));
        assert!(Command::parse("new-window -x").unwrap_err().contains("unknown option"));
    }

    #[test]
    fn sessions_and_names() {
        assert_eq!(
            Command::parse("new-session -s work -c /tmp"),
            Ok(Command::NewSession { name: Some("work".into()), cwd: Some("/tmp".into()) })
        );
        assert_eq!(Command::parse("kill-session work"), Ok(Command::KillSession("work".into())));
        assert_eq!(Command::parse("rename-window my new name"), Ok(Command::RenameWindow("my new name".into())));
        assert_eq!(Command::parse("rename-session s2"), Ok(Command::RenameSession("s2".into())));
    }

    #[test]
    fn send_keys_types_text_and_presses_named_keys() {
        assert_eq!(Command::parse("send-keys ls Enter"), Ok(Command::SendKeys { pane: None, bytes: b"ls\r".to_vec() }));
        assert_eq!(Command::parse("send-keys -t 7 C-c"), Ok(Command::SendKeys { pane: Some(7), bytes: vec![3] }));
        assert_eq!(Command::parse("send-keys 'echo hi' Enter"), Ok(Command::SendKeys { pane: None, bytes: b"echo hi\r".to_vec() }));
        assert_eq!(Command::parse("send-keys Up F5"), Ok(Command::SendKeys { pane: None, bytes: b"\x1b[A\x1b[15~".to_vec() }));
        assert_eq!(Command::parse("send-keys x y"), Ok(Command::SendKeys { pane: None, bytes: b"xy".to_vec() }));
        // -l types every word as text, even `Enter`
        assert_eq!(Command::parse("send-keys -l Enter"), Ok(Command::SendKeys { pane: None, bytes: b"Enter".to_vec() }));
        assert!(Command::parse("send-keys").is_err());
        assert!(Command::parse("send-keys -t x a").unwrap_err().contains("bad pane"));
    }
}
