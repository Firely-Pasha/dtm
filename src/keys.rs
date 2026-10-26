//! Key events: decoding raw terminal input and parsing key names from config.

pub const SHIFT: u8 = 1;
pub const ALT: u8 = 2;
pub const CTRL: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseKind {
    Press,
    Release,
    /// Motion with a button held.
    Drag,
    /// Motion with no button held.
    Move,
    WheelUp,
    WheelDown,
}

/// An SGR mouse report. Coordinates are 0-based cells of the outer terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MouseEvent {
    pub kind: MouseKind,
    /// 0 = left, 1 = middle, 2 = right.
    pub button: u8,
    pub x: u16,
    pub y: u16,
    pub mods: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Escape,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
    /// A whole bracketed paste, markers included in `raw`.
    Paste,
    Mouse(MouseEvent),
    /// Anything we don't interpret (mouse reports, unknown CSI ...).
    Other,
}

pub type KeyCombo = (Key, u8);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub mods: u8,
    /// The exact bytes received, so unbound keys can be forwarded untouched.
    pub raw: Vec<u8>,
}

impl KeyEvent {
    pub fn combo(&self) -> KeyCombo {
        (self.key, self.mods)
    }
}

const NAMES: &[(&str, Key)] = &[
    ("up", Key::Up),
    ("down", Key::Down),
    ("left", Key::Left),
    ("right", Key::Right),
    ("home", Key::Home),
    ("end", Key::End),
    ("pageup", Key::PageUp),
    ("pgup", Key::PageUp),
    ("pagedown", Key::PageDown),
    ("pgdn", Key::PageDown),
    ("insert", Key::Insert),
    ("delete", Key::Delete),
    ("enter", Key::Enter),
    ("tab", Key::Tab),
    ("escape", Key::Escape),
    ("esc", Key::Escape),
    ("space", Key::Char(' ')),
    ("bspace", Key::Backspace),
    ("backspace", Key::Backspace),
];

/// Parse notation such as `C-a`, `M-h`, `S-Left`, `C-M-x`, `F5`, `|`.
pub fn parse_key(s: &str) -> Result<KeyCombo, String> {
    let mut rest = s;
    let mut mods = 0u8;
    while rest.len() > 2 && rest.as_bytes()[1] == b'-' {
        match rest.as_bytes()[0] {
            b'C' | b'c' => mods |= CTRL,
            b'M' | b'm' | b'A' | b'a' => mods |= ALT,
            b'S' | b's' => mods |= SHIFT,
            _ => break,
        }
        rest = &rest[2..];
    }
    if rest.is_empty() {
        return Err(format!("empty key in {s:?}"));
    }
    let lower = rest.to_ascii_lowercase();
    let key = if let Some((_, k)) = NAMES.iter().find(|(n, _)| *n == lower) {
        *k
    } else if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=12).contains(n)) {
        Key::F(n)
    } else {
        let mut chars = rest.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Key::Char(c),
            _ => return Err(format!("unknown key {s:?}")),
        }
    };
    if let Key::Char(c) = key {
        if mods & CTRL != 0 && c.is_ascii_alphabetic() {
            return Ok((Key::Char(c.to_ascii_lowercase()), mods));
        }
        if mods & SHIFT != 0 && c.is_ascii_alphabetic() {
            return Ok((Key::Char(c.to_ascii_uppercase()), mods & !SHIFT));
        }
    }
    Ok((key, mods))
}

pub fn format_key((key, mods): KeyCombo) -> String {
    let mut s = String::new();
    if mods & CTRL != 0 {
        s.push_str("C-");
    }
    if mods & ALT != 0 {
        s.push_str("M-");
    }
    if mods & SHIFT != 0 {
        s.push_str("S-");
    }
    match key {
        Key::Char(' ') => s.push_str("Space"),
        Key::Char(c) => s.push(c),
        Key::Enter => s.push_str("Enter"),
        Key::Tab => s.push_str("Tab"),
        Key::Escape => s.push_str("Escape"),
        Key::Backspace => s.push_str("BSpace"),
        Key::Up => s.push_str("Up"),
        Key::Down => s.push_str("Down"),
        Key::Left => s.push_str("Left"),
        Key::Right => s.push_str("Right"),
        Key::Home => s.push_str("Home"),
        Key::End => s.push_str("End"),
        Key::PageUp => s.push_str("PageUp"),
        Key::PageDown => s.push_str("PageDown"),
        Key::Insert => s.push_str("Insert"),
        Key::Delete => s.push_str("Delete"),
        Key::F(n) => s.push_str(&format!("F{n}")),
        Key::Paste => s.push_str("Paste"),
        Key::Mouse(_) => s.push_str("Mouse"),
        Key::Other => s.push_str("Other"),
    }
    s
}

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";
/// Give up waiting for the end of a paste after this much data.
const MAX_PASTE: usize = 1 << 20;

/// Decode as many complete key events as possible from the front of `buf`.
/// With `flush` set, a trailing incomplete sequence is resolved (a lone ESC
/// becomes the Escape key) instead of waiting for more bytes.
/// The bytes a terminal sends for `combo`: the inverse of [`decode`], for `send-keys`.
/// Keys that are not on the keyboard (paste, mouse) give nothing.
pub fn encode((key, mods): KeyCombo) -> Vec<u8> {
    let x = 1 + u8::from(mods & SHIFT != 0) + 2 * u8::from(mods & ALT != 0) + 4 * u8::from(mods & CTRL != 0);
    // CSI [code ;] mods final, leaving the modifiers out when there are none.
    let csi = |code: u32, fin: char| -> Vec<u8> {
        if x == 1 {
            if fin == '~' { format!("\x1b[{code}~") } else { format!("\x1b[{fin}") }.into_bytes()
        } else {
            format!("\x1b[{code};{x}{fin}").into_bytes()
        }
    };
    let ss3 = |fin: char| -> Vec<u8> {
        if x == 1 { format!("\x1bO{fin}").into_bytes() } else { format!("\x1b[1;{x}{fin}").into_bytes() }
    };
    // A key with a plain byte form: Alt adds an ESC in front, other modifiers need CSI u.
    let plain = |code: u32, bytes: Vec<u8>| -> Vec<u8> {
        if mods & (CTRL | SHIFT) != 0 && code != 0 {
            return format!("\x1b[{code};{x}u").into_bytes();
        }
        let mut out = Vec::new();
        if mods & ALT != 0 {
            out.push(0x1b);
        }
        out.extend(bytes);
        out
    };
    match key {
        Key::Char(c) if mods & CTRL != 0 => {
            let control = match c {
                'a'..='z' => Some(c as u8 - b'a' + 1),
                ' ' => Some(0),
                '\\' | ']' | '^' | '_' => Some(c as u8 - b'\\' + 0x1c),
                _ => None,
            };
            match control {
                Some(b) if mods & SHIFT == 0 => {
                    let mut out = Vec::new();
                    if mods & ALT != 0 {
                        out.push(0x1b);
                    }
                    out.push(b);
                    out
                }
                _ => format!("\x1b[{};{x}u", c as u32).into_bytes(),
            }
        }
        Key::Char(c) => {
            let mut out = Vec::new();
            if mods & ALT != 0 {
                out.push(0x1b);
            }
            out.extend(c.to_string().into_bytes());
            out
        }
        Key::Enter => plain(13, vec![b'\r']),
        Key::Tab if mods == SHIFT => b"\x1b[Z".to_vec(),
        Key::Tab => plain(9, vec![b'\t']),
        Key::Escape => plain(27, vec![0x1b]),
        Key::Backspace => plain(127, vec![0x7f]),
        Key::Up => csi(1, 'A'),
        Key::Down => csi(1, 'B'),
        Key::Right => csi(1, 'C'),
        Key::Left => csi(1, 'D'),
        Key::Home => csi(1, 'H'),
        Key::End => csi(1, 'F'),
        Key::Insert => csi(2, '~'),
        Key::Delete => csi(3, '~'),
        Key::PageUp => csi(5, '~'),
        Key::PageDown => csi(6, '~'),
        Key::F(n @ 1..=4) => ss3(['P', 'Q', 'R', 'S'][n as usize - 1]),
        Key::F(n) => {
            let code = match n {
                5 => 15,
                6..=10 => 11 + n as u32,
                11 | 12 => 12 + n as u32,
                _ => return Vec::new(),
            };
            csi(code, '~')
        }
        Key::Paste | Key::Mouse(_) | Key::Other => Vec::new(),
    }
}

pub fn decode(buf: &mut Vec<u8>, flush: bool) -> Vec<KeyEvent> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        match decode_one(&buf[i..], flush) {
            Some((ev, n)) => {
                out.push(ev);
                i += n;
            }
            None => break,
        }
    }
    buf.drain(..i);
    out
}

fn ev(key: Key, mods: u8, raw: &[u8]) -> KeyEvent {
    KeyEvent { key, mods, raw: raw.to_vec() }
}

fn decode_one(b: &[u8], flush: bool) -> Option<(KeyEvent, usize)> {
    match b[0] {
        0x1B => decode_escape(b, flush),
        0x00 => Some((ev(Key::Char(' '), CTRL, &b[..1]), 1)),
        0x09 => Some((ev(Key::Tab, 0, &b[..1]), 1)),
        0x0D => Some((ev(Key::Enter, 0, &b[..1]), 1)),
        0x7F => Some((ev(Key::Backspace, 0, &b[..1]), 1)),
        c @ 0x01..=0x1A => Some((ev(Key::Char((b'a' + c - 1) as char), CTRL, &b[..1]), 1)),
        c @ 0x1C..=0x1F => {
            let ch = b"\\]^_"[(c - 0x1C) as usize] as char;
            Some((ev(Key::Char(ch), CTRL, &b[..1]), 1))
        }
        0x20..=0x7E => Some((ev(Key::Char(b[0] as char), 0, &b[..1]), 1)),
        first => {
            let need = match first {
                0xC2..=0xDF => 2,
                0xE0..=0xEF => 3,
                0xF0..=0xF4 => 4,
                _ => 0,
            };
            if need == 0 {
                return Some((ev(Key::Char('\u{FFFD}'), 0, &b[..1]), 1));
            }
            if b.len() < need {
                return if flush { Some((ev(Key::Char('\u{FFFD}'), 0, &b[..1]), 1)) } else { None };
            }
            match std::str::from_utf8(&b[..need]).ok().and_then(|s| s.chars().next()) {
                Some(c) => Some((ev(Key::Char(c), 0, &b[..need]), need)),
                None => Some((ev(Key::Char('\u{FFFD}'), 0, &b[..1]), 1)),
            }
        }
    }
}

fn decode_escape(b: &[u8], flush: bool) -> Option<(KeyEvent, usize)> {
    if b.len() == 1 {
        return flush.then(|| (ev(Key::Escape, 0, b), 1));
    }
    match b[1] {
        b'[' => decode_csi(b, flush),
        b'O' => {
            if b.len() < 3 {
                return if flush { Some((ev(Key::Escape, 0, &b[..1]), 1)) } else { None };
            }
            let key = match b[2] {
                b'A' => Key::Up,
                b'B' => Key::Down,
                b'C' => Key::Right,
                b'D' => Key::Left,
                b'H' => Key::Home,
                b'F' => Key::End,
                b'P' => Key::F(1),
                b'Q' => Key::F(2),
                b'R' => Key::F(3),
                b'S' => Key::F(4),
                _ => Key::Other,
            };
            Some((ev(key, 0, &b[..3]), 3))
        }
        _ => {
            // ESC-prefix means Alt.
            let (mut inner, n) = decode_one(&b[1..], flush)?;
            inner.mods |= ALT;
            inner.raw = b[..n + 1].to_vec();
            Some((inner, n + 1))
        }
    }
}

fn decode_csi(b: &[u8], flush: bool) -> Option<(KeyEvent, usize)> {
    if b.starts_with(PASTE_START) {
        if let Some(pos) = b.windows(PASTE_END.len()).position(|w| w == PASTE_END) {
            let n = pos + PASTE_END.len();
            return Some((ev(Key::Paste, 0, &b[..n]), n));
        }
        if b.len() >= MAX_PASTE {
            return Some((ev(Key::Paste, 0, b), b.len()));
        }
        return None;
    }
    let mut j = 2;
    while j < b.len() && (0x30..=0x3F).contains(&b[j]) {
        j += 1;
    }
    while j < b.len() && (0x20..=0x2F).contains(&b[j]) {
        j += 1;
    }
    if j >= b.len() {
        return if flush { Some((ev(Key::Escape, 0, &b[..1]), 1)) } else { None };
    }
    let fin = b[j];
    let raw = &b[..=j];
    if !(0x40..=0x7E).contains(&fin) {
        return Some((ev(Key::Other, 0, raw), j + 1));
    }
    let params_str = std::str::from_utf8(&b[2..j]).unwrap_or("");
    if let Some(rest) = params_str.strip_prefix('<') {
        let key = match (parse_sgr_mouse(rest, fin), raw) {
            (Some(m), _) => Key::Mouse(m),
            (None, _) => Key::Other,
        };
        return Some((ev(key, 0, raw), j + 1));
    }
    if params_str.starts_with(['<', '>', '=', '?']) || b[2..j].iter().any(|&c| (0x20..=0x2F).contains(&c)) {
        return Some((ev(Key::Other, 0, raw), j + 1));
    }
    let nums: Vec<u32> = params_str.split(';').map(|p| p.split(':').next().unwrap_or("").parse().unwrap_or(0)).collect();
    let mods = match nums.get(1) {
        Some(&m) if m > 1 => {
            let m = m - 1;
            (if m & 1 != 0 { SHIFT } else { 0 })
                | (if m & (2 | 8) != 0 { ALT } else { 0 })
                | (if m & 4 != 0 { CTRL } else { 0 })
        }
        _ => 0,
    };
    let key = match fin {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        b'P' => Key::F(1),
        b'Q' => Key::F(2),
        b'R' => Key::F(3),
        b'S' => Key::F(4),
        b'Z' => return Some((ev(Key::Tab, SHIFT, raw), j + 1)),
        // CSI code ; mods u (kitty, fixterms) and CSI 27 ; mods ; code ~ (xterm modifyOtherKeys).
        b'u' => return Some((codepoint_key(nums[0], mods, raw), j + 1)),
        b'~' if nums.first() == Some(&27) => {
            return Some((codepoint_key(nums.get(2).copied().unwrap_or(0), mods, raw), j + 1));
        }
        b'~' => match nums.first().copied().unwrap_or(0) {
            1 | 7 => Key::Home,
            2 => Key::Insert,
            3 => Key::Delete,
            4 | 8 => Key::End,
            5 => Key::PageUp,
            6 => Key::PageDown,
            n @ 11..=15 => Key::F((n - 10) as u8),
            n @ 17..=21 => Key::F((n - 11) as u8),
            n @ 23..=24 => Key::F((n - 12) as u8),
            _ => Key::Other,
        },
        _ => Key::Other,
    };
    let mods = if key == Key::Other { 0 } else { mods };
    Some((ev(key, mods, raw), j + 1))
}

/// A key reported by its Unicode code point, normalised the way `parse_key` spells it.
fn codepoint_key(code: u32, mods: u8, raw: &[u8]) -> KeyEvent {
    let key = match code {
        9 => Key::Tab,
        13 => Key::Enter,
        27 => Key::Escape,
        127 => Key::Backspace,
        c => match char::from_u32(c).filter(|c| !c.is_control()) {
            Some(c) => Key::Char(c),
            None => return ev(Key::Other, 0, raw),
        },
    };
    match key {
        Key::Char(c) if mods & CTRL != 0 && c.is_ascii_alphabetic() => ev(Key::Char(c.to_ascii_lowercase()), mods, raw),
        Key::Char(c) if mods & SHIFT != 0 && c.is_ascii_alphabetic() => {
            ev(Key::Char(c.to_ascii_uppercase()), mods & !SHIFT, raw)
        }
        _ => ev(key, mods, raw),
    }
}

fn parse_sgr_mouse(params: &str, fin: u8) -> Option<MouseEvent> {
    if fin != b'M' && fin != b'm' {
        return None;
    }
    let mut it = params.split(';').map(|p| p.parse::<u16>().ok());
    let (b, x, y) = (it.next()??, it.next()??, it.next()??);
    if it.next().is_some() || x == 0 || y == 0 {
        return None;
    }
    let mods = (if b & 4 != 0 { SHIFT } else { 0 }) | (if b & 8 != 0 { ALT } else { 0 }) | (if b & 16 != 0 { CTRL } else { 0 });
    let low = (b & 3) as u8;
    let (kind, button) = if b & 64 != 0 {
        match low {
            0 => (MouseKind::WheelUp, 0),
            1 => (MouseKind::WheelDown, 0),
            _ => return None,
        }
    } else if b & 32 != 0 {
        if low == 3 { (MouseKind::Move, 0) } else { (MouseKind::Drag, low) }
    } else if fin == b'm' {
        (MouseKind::Release, low.min(2))
    } else {
        (MouseKind::Press, low.min(2))
    };
    Some(MouseEvent { kind, button, x: x - 1, y: y - 1, mods })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(input: &[u8]) -> Vec<(Key, u8)> {
        let mut buf = input.to_vec();
        let v = decode(&mut buf, true);
        assert!(buf.is_empty(), "leftover {buf:?}");
        v.iter().map(KeyEvent::combo).collect()
    }

    #[test]
    fn parses_notation() {
        assert_eq!(parse_key("C-a"), Ok((Key::Char('a'), CTRL)));
        assert_eq!(parse_key("C-A"), Ok((Key::Char('a'), CTRL)));
        assert_eq!(parse_key("M-h"), Ok((Key::Char('h'), ALT)));
        assert_eq!(parse_key("S-Left"), Ok((Key::Left, SHIFT)));
        assert_eq!(parse_key("C-M-x"), Ok((Key::Char('x'), CTRL | ALT)));
        assert_eq!(parse_key("S-h"), Ok((Key::Char('H'), 0)));
        assert_eq!(parse_key("F5"), Ok((Key::F(5), 0)));
        assert_eq!(parse_key("|"), Ok((Key::Char('|'), 0)));
        assert_eq!(parse_key("-"), Ok((Key::Char('-'), 0)));
        assert_eq!(parse_key("C--"), Ok((Key::Char('-'), CTRL)));
        assert_eq!(parse_key("Space"), Ok((Key::Char(' '), 0)));
        assert_eq!(parse_key("PageUp"), Ok((Key::PageUp, 0)));
        assert!(parse_key("").is_err());
        assert!(parse_key("Bogus").is_err());
        assert!(parse_key("F13").is_err());
    }

    #[test]
    fn notation_round_trips() {
        for s in ["C-a", "M-h", "S-Left", "C-M-x", "F5", "|", "Space", "PageDown", "Enter", "C-Space"] {
            let k = parse_key(s).unwrap();
            assert_eq!(parse_key(&format_key(k)), Ok(k), "{s}");
        }
    }

    #[test]
    fn decodes_plain_and_control() {
        assert_eq!(keys(b"a"), vec![(Key::Char('a'), 0)]);
        assert_eq!(keys(b"\x01"), vec![(Key::Char('a'), CTRL)]);
        assert_eq!(keys(b"\r\t\x7f"), vec![(Key::Enter, 0), (Key::Tab, 0), (Key::Backspace, 0)]);
        assert_eq!(keys("é".as_bytes()), vec![(Key::Char('é'), 0)]);
        assert_eq!(keys(b"\x00"), vec![(Key::Char(' '), CTRL)]);
    }

    #[test]
    fn decodes_arrows_and_modifiers() {
        assert_eq!(keys(b"\x1b[A\x1bOB"), vec![(Key::Up, 0), (Key::Down, 0)]);
        assert_eq!(keys(b"\x1b[1;5C"), vec![(Key::Right, CTRL)]);
        assert_eq!(keys(b"\x1b[1;2D"), vec![(Key::Left, SHIFT)]);
        assert_eq!(keys(b"\x1b[1;3A"), vec![(Key::Up, ALT)]);
        assert_eq!(keys(b"\x1b[Z"), vec![(Key::Tab, SHIFT)]);
        assert_eq!(keys(b"\x1b[9;5u"), vec![(Key::Tab, CTRL)]);
        assert_eq!(keys(b"\x1b[9;6u"), vec![(Key::Tab, CTRL | SHIFT)]);
        assert_eq!(keys(b"\x1b[27;5;9~"), vec![(Key::Tab, CTRL)]);
        assert_eq!(keys(b"\x1b[97;5u"), vec![(Key::Char('a'), CTRL)]);
        assert_eq!(keys(b"\x1b[97;2u"), vec![(Key::Char('A'), 0)]);
        assert_eq!(parse_key("C-Tab"), Ok((Key::Tab, CTRL)));
        assert_eq!(parse_key("C-S-Tab"), Ok((Key::Tab, CTRL | SHIFT)));
    }

    #[test]
    fn decodes_tilde_and_function_keys() {
        assert_eq!(keys(b"\x1b[3~\x1b[5~\x1b[6~"), vec![(Key::Delete, 0), (Key::PageUp, 0), (Key::PageDown, 0)]);
        assert_eq!(keys(b"\x1b[15~\x1b[24~\x1bOP"), vec![(Key::F(5), 0), (Key::F(12), 0), (Key::F(1), 0)]);
        assert_eq!(keys(b"\x1b[15;5~"), vec![(Key::F(5), CTRL)]);
    }

    #[test]
    fn decodes_alt_and_escape() {
        assert_eq!(keys(b"\x1bh"), vec![(Key::Char('h'), ALT)]);
        assert_eq!(keys(b"\x1b"), vec![(Key::Escape, 0)]);
        assert_eq!(keys(b"\x1b\x1b"), vec![(Key::Escape, ALT)]);
        assert_eq!(keys(b"\x1b\x01"), vec![(Key::Char('a'), CTRL | ALT)]);
    }

    #[test]
    fn lone_escape_waits_until_flush() {
        let mut buf = b"\x1b".to_vec();
        assert!(decode(&mut buf, false).is_empty());
        assert_eq!(buf, b"\x1b");
        let v = decode(&mut buf, true);
        assert_eq!(v[0].key, Key::Escape);
    }

    #[test]
    fn split_sequences_wait_for_more() {
        let mut buf = b"\x1b[1;".to_vec();
        assert!(decode(&mut buf, false).is_empty());
        buf.extend_from_slice(b"5C");
        let v = decode(&mut buf, false);
        assert_eq!(v[0].combo(), (Key::Right, CTRL));
        let mut buf = vec![0xE2, 0x82];
        assert!(decode(&mut buf, false).is_empty());
        buf.push(0xAC);
        assert_eq!(decode(&mut buf, false)[0].key, Key::Char('€'));
    }

    #[test]
    fn keeps_raw_bytes() {
        let mut buf = b"\x1b[1;5Cx".to_vec();
        let v = decode(&mut buf, true);
        assert_eq!(v[0].raw, b"\x1b[1;5C");
        assert_eq!(v[1].raw, b"x");
    }

    #[test]
    fn bracketed_paste_is_one_event() {
        let mut buf = b"\x1b[200~ab\x01c\x1b[201~z".to_vec();
        let v = decode(&mut buf, true);
        assert_eq!(v[0].key, Key::Paste);
        assert_eq!(v[0].raw, b"\x1b[200~ab\x01c\x1b[201~");
        assert_eq!(v[1].key, Key::Char('z'));
        let mut partial = b"\x1b[200~abc".to_vec();
        assert!(decode(&mut partial, true).is_empty());
    }

    fn mouse(input: &[u8]) -> MouseEvent {
        match keys(input)[0].0 {
            Key::Mouse(m) => m,
            other => panic!("not a mouse event: {other:?}"),
        }
    }

    #[test]
    fn decodes_sgr_mouse() {
        let m = mouse(b"\x1b[<0;10;5M");
        assert_eq!((m.kind, m.button, m.x, m.y), (MouseKind::Press, 0, 9, 4));
        assert_eq!(mouse(b"\x1b[<0;10;5m").kind, MouseKind::Release);
        assert_eq!(mouse(b"\x1b[<2;1;1M").button, 2);
        assert_eq!(mouse(b"\x1b[<64;3;4M").kind, MouseKind::WheelUp);
        assert_eq!(mouse(b"\x1b[<65;3;4M").kind, MouseKind::WheelDown);
        assert_eq!(mouse(b"\x1b[<32;3;4M").kind, MouseKind::Drag);
        assert_eq!(mouse(b"\x1b[<35;3;4M").kind, MouseKind::Move);
        assert_eq!(mouse(b"\x1b[<16;3;4M").mods, CTRL);
        assert_eq!(mouse(b"\x1b[<20;3;4M").mods, CTRL | SHIFT);
    }

    #[test]
    fn malformed_mouse_and_unknown_are_other() {
        assert_eq!(keys(b"\x1b[<0;0;5M"), vec![(Key::Other, 0)]);
        assert_eq!(keys(b"\x1b[<0;1M"), vec![(Key::Other, 0)]);
        assert_eq!(keys(b"\x1b[<66;1;1M"), vec![(Key::Other, 0)]);
        assert_eq!(keys(b"\x1b[1;5u"), vec![(Key::Other, 0)]);
    }

    #[test]
    fn encode_is_the_inverse_of_decode() {
        let combos: Vec<KeyCombo> = vec![
            (Key::Char('a'), 0),
            (Key::Char('A'), 0),
            (Key::Char('é'), 0),
            (Key::Char('a'), CTRL),
            (Key::Char(' '), CTRL),
            (Key::Char('a'), ALT),
            (Key::Char('a'), CTRL | ALT),
            (Key::Enter, 0),
            (Key::Tab, 0),
            (Key::Tab, SHIFT),
            (Key::Tab, CTRL),
            (Key::Escape, 0),
            (Key::Backspace, 0),
            (Key::Up, 0),
            (Key::Left, CTRL),
            (Key::Right, SHIFT | ALT),
            (Key::Home, 0),
            (Key::End, CTRL),
            (Key::Insert, 0),
            (Key::Delete, 0),
            (Key::PageUp, 0),
            (Key::PageDown, ALT),
            (Key::F(1), 0),
            (Key::F(4), CTRL),
            (Key::F(5), 0),
            (Key::F(10), SHIFT),
            (Key::F(12), 0),
        ];
        for combo in combos {
            let mut bytes = encode(combo);
            let events = decode(&mut bytes, true);
            assert_eq!(events.len(), 1, "{combo:?}");
            assert_eq!(events[0].combo(), combo, "{combo:?} -> {:?}", encode(combo));
        }
        assert_eq!(encode((Key::Char('x'), 0)), b"x");
        assert_eq!(encode((Key::Char('c'), CTRL)), [3]);
        assert_eq!(encode((Key::Enter, 0)), b"\r");
        assert!(encode((Key::Other, 0)).is_empty());
    }
}

