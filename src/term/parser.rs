//! VT500-style escape sequence parser. Bytes in, `Action`s out.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Print(char),
    Execute(u8),
    Csi {
        /// Parameters; each parameter holds its colon-separated sub-parameters.
        params: Vec<Vec<u16>>,
        intermediates: Vec<u8>,
        /// Private marker byte (`?`, `>`, `<`, `=`) if present.
        private: Option<u8>,
        fin: u8,
    },
    Esc {
        intermediates: Vec<u8>,
        fin: u8,
    },
    Osc(Vec<Vec<u8>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    OscString,
    /// DCS, SOS, PM and APC strings are consumed and discarded.
    StringIgnore,
}

const MAX_PARAMS: usize = 32;
const MAX_OSC: usize = 4096;

pub struct Parser {
    state: State,
    params: Vec<Vec<u16>>,
    cur: Vec<u16>,
    cur_open: bool,
    intermediates: Vec<u8>,
    private: Option<u8>,
    osc: Vec<u8>,
    /// Set while an ESC has been seen inside a string (possible ST).
    string_esc: bool,
    utf8: [u8; 4],
    utf8_len: usize,
    utf8_need: usize,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub fn new() -> Self {
        Parser {
            state: State::Ground,
            params: Vec::new(),
            cur: Vec::new(),
            cur_open: false,
            intermediates: Vec::new(),
            private: None,
            osc: Vec::new(),
            string_esc: false,
            utf8: [0; 4],
            utf8_len: 0,
            utf8_need: 0,
        }
    }

    pub fn feed(&mut self, bytes: &[u8], out: &mut impl FnMut(Action)) {
        for &b in bytes {
            self.byte(b, out);
        }
    }

    fn clear(&mut self) {
        self.params.clear();
        self.cur.clear();
        self.cur_open = false;
        self.intermediates.clear();
        self.private = None;
    }

    fn byte(&mut self, b: u8, out: &mut impl FnMut(Action)) {
        // Strings have their own terminator handling.
        if matches!(self.state, State::OscString | State::StringIgnore) {
            return self.string_byte(b, out);
        }
        // UTF-8 continuation in progress (Ground only).
        if self.utf8_need > 0 {
            if b & 0xC0 == 0x80 {
                self.utf8[self.utf8_len] = b;
                self.utf8_len += 1;
                if self.utf8_len == self.utf8_need {
                    self.utf8_need = 0;
                    let ch = std::str::from_utf8(&self.utf8[..self.utf8_len])
                        .ok()
                        .and_then(|s| s.chars().next())
                        .unwrap_or('\u{FFFD}');
                    out(Action::Print(ch));
                }
                return;
            }
            // Broken sequence: emit replacement, then handle this byte normally.
            self.utf8_need = 0;
            out(Action::Print('\u{FFFD}'));
        }
        // Anywhere transitions.
        match b {
            0x18 | 0x1A => {
                self.state = State::Ground;
                out(Action::Execute(b));
                return;
            }
            0x1B => {
                self.clear();
                self.state = State::Escape;
                return;
            }
            _ => {}
        }
        match self.state {
            State::Ground => self.ground(b, out),
            State::Escape => self.escape(b, out),
            State::EscapeIntermediate => match b {
                0x00..=0x1F => out(Action::Execute(b)),
                0x20..=0x2F => self.intermediates.push(b),
                0x30..=0x7E => self.dispatch_esc(b, out),
                _ => {}
            },
            State::CsiEntry | State::CsiParam => self.csi_param(b, out),
            State::CsiIntermediate => match b {
                0x00..=0x1F => out(Action::Execute(b)),
                0x20..=0x2F => self.intermediates.push(b),
                0x40..=0x7E => self.dispatch_csi(b, out),
                0x30..=0x3F => self.state = State::CsiIgnore,
                _ => {}
            },
            State::CsiIgnore => match b {
                0x00..=0x1F => out(Action::Execute(b)),
                0x40..=0x7E => self.state = State::Ground,
                _ => {}
            },
            State::OscString | State::StringIgnore => unreachable!(),
        }
    }

    fn ground(&mut self, b: u8, out: &mut impl FnMut(Action)) {
        match b {
            0x00..=0x1F | 0x7F => {
                if b != 0x7F {
                    out(Action::Execute(b));
                }
            }
            0x20..=0x7E => out(Action::Print(b as char)),
            0xC2..=0xDF => self.utf8_start(b, 2),
            0xE0..=0xEF => self.utf8_start(b, 3),
            0xF0..=0xF4 => self.utf8_start(b, 4),
            _ => out(Action::Print('\u{FFFD}')),
        }
    }

    fn utf8_start(&mut self, b: u8, need: usize) {
        self.utf8[0] = b;
        self.utf8_len = 1;
        self.utf8_need = need;
    }

    fn escape(&mut self, b: u8, out: &mut impl FnMut(Action)) {
        match b {
            0x00..=0x1F => out(Action::Execute(b)),
            0x20..=0x2F => {
                self.intermediates.push(b);
                self.state = State::EscapeIntermediate;
            }
            b'[' => {
                self.clear();
                self.state = State::CsiEntry;
            }
            b']' => {
                self.osc.clear();
                self.string_esc = false;
                self.state = State::OscString;
            }
            b'P' | b'X' | b'^' | b'_' => {
                self.string_esc = false;
                self.state = State::StringIgnore;
            }
            0x30..=0x7E => self.dispatch_esc(b, out),
            _ => {}
        }
    }

    fn dispatch_esc(&mut self, fin: u8, out: &mut impl FnMut(Action)) {
        self.state = State::Ground;
        out(Action::Esc {
            intermediates: std::mem::take(&mut self.intermediates),
            fin,
        });
    }

    fn csi_param(&mut self, b: u8, out: &mut impl FnMut(Action)) {
        match b {
            0x00..=0x1F => out(Action::Execute(b)),
            0x30..=0x39 => {
                let v = self.cur.last_mut();
                match v {
                    Some(v) if self.cur_open => {
                        *v = v.saturating_mul(10).saturating_add((b - b'0') as u16);
                    }
                    _ => {
                        self.cur.push((b - b'0') as u16);
                        self.cur_open = true;
                    }
                }
                self.state = State::CsiParam;
            }
            b':' => {
                if !self.cur_open {
                    self.cur.push(0);
                }
                self.cur_open = false;
                self.state = State::CsiParam;
            }
            b';' => {
                self.end_param();
                self.state = State::CsiParam;
            }
            0x3C..=0x3F => {
                if self.state == State::CsiEntry {
                    self.private = Some(b);
                    self.state = State::CsiParam;
                } else {
                    self.state = State::CsiIgnore;
                }
            }
            0x20..=0x2F => {
                self.intermediates.push(b);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7E => self.dispatch_csi(b, out),
            _ => {}
        }
    }

    fn end_param(&mut self) {
        if self.params.len() < MAX_PARAMS {
            if self.cur.is_empty() {
                self.cur.push(0);
            }
            self.params.push(std::mem::take(&mut self.cur));
        } else {
            self.cur.clear();
        }
        self.cur_open = false;
    }

    fn dispatch_csi(&mut self, fin: u8, out: &mut impl FnMut(Action)) {
        if !self.cur.is_empty() || !self.params.is_empty() {
            self.end_param();
        }
        self.state = State::Ground;
        out(Action::Csi {
            params: std::mem::take(&mut self.params),
            intermediates: std::mem::take(&mut self.intermediates),
            private: self.private.take(),
            fin,
        });
    }

    fn string_byte(&mut self, b: u8, out: &mut impl FnMut(Action)) {
        if self.string_esc {
            self.string_esc = false;
            if b == b'\\' {
                return self.end_string(out);
            }
            // ESC aborted the string and starts a new sequence.
            self.state = State::Ground;
            self.clear();
            self.state = State::Escape;
            return self.byte_in_escape(b, out);
        }
        match b {
            0x1B => self.string_esc = true,
            0x07 if self.state == State::OscString => self.end_string(out),
            0x18 | 0x1A => self.state = State::Ground,
            _ => {
                if self.state == State::OscString && self.osc.len() < MAX_OSC {
                    self.osc.push(b);
                }
            }
        }
    }

    fn byte_in_escape(&mut self, b: u8, out: &mut impl FnMut(Action)) {
        self.escape(b, out);
    }

    fn end_string(&mut self, out: &mut impl FnMut(Action)) {
        if self.state == State::OscString {
            let parts = self.osc.split(|&c| c == b';').map(<[u8]>::to_vec).collect();
            out(Action::Osc(parts));
        }
        self.osc.clear();
        self.state = State::Ground;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8]) -> Vec<Action> {
        let mut p = Parser::new();
        let mut v = Vec::new();
        p.feed(bytes, &mut |a| v.push(a));
        v
    }

    fn csi(params: &[&[u16]], private: Option<u8>, fin: u8) -> Action {
        Action::Csi {
            params: params.iter().map(|p| p.to_vec()).collect(),
            intermediates: vec![],
            private,
            fin,
        }
    }

    #[test]
    fn prints_ascii_and_controls() {
        assert_eq!(
            parse(b"a\r\n"),
            vec![Action::Print('a'), Action::Execute(b'\r'), Action::Execute(b'\n')]
        );
    }

    #[test]
    fn decodes_utf8() {
        assert_eq!(parse("é€😀".as_bytes()), vec![
            Action::Print('é'),
            Action::Print('€'),
            Action::Print('😀')
        ]);
    }

    #[test]
    fn utf8_split_across_feeds() {
        let mut p = Parser::new();
        let mut v = Vec::new();
        let bytes = "€".as_bytes();
        p.feed(&bytes[..1], &mut |a| v.push(a));
        p.feed(&bytes[1..], &mut |a| v.push(a));
        assert_eq!(v, vec![Action::Print('€')]);
    }

    #[test]
    fn invalid_utf8_yields_replacement() {
        assert_eq!(
            parse(&[0xE2, b'a']),
            vec![Action::Print('\u{FFFD}'), Action::Print('a')]
        );
    }

    #[test]
    fn csi_with_params() {
        assert_eq!(parse(b"\x1b[1;31m"), vec![csi(&[&[1], &[31]], None, b'm')]);
    }

    #[test]
    fn csi_defaults_and_empty() {
        assert_eq!(parse(b"\x1b[H"), vec![csi(&[], None, b'H')]);
        assert_eq!(parse(b"\x1b[;5H"), vec![csi(&[&[0], &[5]], None, b'H')]);
    }

    #[test]
    fn csi_private_marker() {
        assert_eq!(parse(b"\x1b[?1049h"), vec![csi(&[&[1049]], Some(b'?'), b'h')]);
    }

    #[test]
    fn csi_colon_subparams() {
        assert_eq!(
            parse(b"\x1b[38:2::1:2:3m"),
            vec![csi(&[&[38, 2, 0, 1, 2, 3]], None, b'm')]
        );
    }

    #[test]
    fn csi_intermediate() {
        assert_eq!(
            parse(b"\x1b[2 q"),
            vec![Action::Csi {
                params: vec![vec![2]],
                intermediates: vec![b' '],
                private: None,
                fin: b'q'
            }]
        );
    }

    #[test]
    fn control_inside_csi_executes() {
        assert_eq!(
            parse(b"\x1b[1\n;2H"),
            vec![Action::Execute(b'\n'), csi(&[&[1], &[2]], None, b'H')]
        );
    }

    #[test]
    fn can_aborts_sequence() {
        assert_eq!(
            parse(b"\x1b[1\x18x"),
            vec![Action::Execute(0x18), Action::Print('x')]
        );
    }

    #[test]
    fn esc_sequences() {
        assert_eq!(
            parse(b"\x1b7\x1b(0"),
            vec![
                Action::Esc { intermediates: vec![], fin: b'7' },
                Action::Esc { intermediates: vec![b'('], fin: b'0' },
            ]
        );
    }

    #[test]
    fn osc_bel_and_st() {
        let want = Action::Osc(vec![b"0".to_vec(), b"title".to_vec()]);
        assert_eq!(parse(b"\x1b]0;title\x07"), vec![want.clone()]);
        assert_eq!(parse(b"\x1b]0;title\x1b\\"), vec![want]);
    }

    #[test]
    fn dcs_is_swallowed() {
        assert_eq!(
            parse(b"\x1bPq#0;2;0;0;0\x1b\\ok"),
            vec![Action::Print('o'), Action::Print('k')]
        );
    }

    #[test]
    fn esc_inside_string_starts_new_sequence() {
        assert_eq!(
            parse(b"\x1b]0;ab\x1b[1m"),
            vec![csi(&[&[1]], None, b'm')]
        );
    }

    #[test]
    fn huge_param_saturates() {
        assert_eq!(parse(b"\x1b[99999999m"), vec![csi(&[&[u16::MAX]], None, b'm')]);
    }
}
