//! Bounded incremental ANSI input parsing. Escape sequences never reach rendering.
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Escape,
    Enter,
    Backspace,
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Mouse {
        button: u8,
        column: u16,
        row: u16,
        released: bool,
    },
    Paste(String),
}
#[derive(Default)]
pub struct Decoder {
    bytes: Vec<u8>,
    paste: Option<Vec<u8>>,
}
impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Key> {
        if self.bytes.len() + bytes.len() > 8192 {
            self.bytes.clear();
            self.paste = None;
            return vec![];
        }
        self.bytes.extend_from_slice(bytes);
        self.drain(false)
    }
    pub fn idle(&mut self) -> Vec<Key> {
        self.drain(true)
    }
    fn drain(&mut self, idle: bool) -> Vec<Key> {
        let mut out = Vec::new();
        loop {
            if self.bytes.is_empty() {
                break;
            }
            if let Some(paste) = self.paste.as_mut() {
                if let Some(end) = self.bytes.windows(6).position(|w| w == b"\x1b[201~") {
                    paste.extend_from_slice(&self.bytes[..end]);
                    self.bytes.drain(..end + 6);
                    let value = self.paste.take().unwrap();
                    out.push(Key::Paste(
                        String::from_utf8_lossy(&value)
                            .chars()
                            .filter(|c| !c.is_control())
                            .take(4096)
                            .collect(),
                    ));
                    continue;
                }
                if paste.len() + self.bytes.len() > 4096 {
                    self.paste = None;
                    self.bytes.clear();
                } else {
                    let keep = self.bytes.len().min(5);
                    let consumed = self.bytes.len() - keep;
                    paste.extend_from_slice(&self.bytes[..consumed]);
                    self.bytes.drain(..consumed);
                }
                break;
            }
            if self.bytes.starts_with(b"\x1b[200~") {
                self.bytes.drain(..6);
                self.paste = Some(Vec::new());
                continue;
            }
            let (key, count) = match self.bytes[0] {
                0x1b => {
                    if self.bytes.len() == 1 {
                        if idle {
                            (Some(Key::Escape), 1)
                        } else {
                            break;
                        }
                    } else if self.bytes[1] == b'[' {
                        let Some(end) = self
                            .bytes
                            .iter()
                            .enumerate()
                            .skip(2)
                            .find(|(_, b)| (0x40..=0x7e).contains(*b))
                            .map(|(i, _)| i)
                        else {
                            if idle {
                                self.bytes.clear();
                            }
                            break;
                        };
                        let raw = &self.bytes[2..=end];
                        let key = match raw {
                            b"A" => Some(Key::Up),
                            b"B" => Some(Key::Down),
                            b"C" => Some(Key::Right),
                            b"D" => Some(Key::Left),
                            b"H" | b"1~" => Some(Key::Home),
                            b"F" | b"4~" => Some(Key::End),
                            b"5~" => Some(Key::PageUp),
                            b"6~" => Some(Key::PageDown),
                            b"3~" => Some(Key::Delete),
                            b"Z" => Some(Key::BackTab),
                            _ => mouse(raw),
                        };
                        (key, end + 1)
                    } else {
                        (Some(Key::Escape), 1)
                    }
                }
                b'\r' | b'\n' => (Some(Key::Enter), 1),
                b'\t' => (Some(Key::Tab), 1),
                127 | 8 => (Some(Key::Backspace), 1),
                1..=26 => (Some(Key::Ctrl((self.bytes[0] + b'a' - 1) as char)), 1),
                0..=31 => (None, 1),
                b if b < 128 => (Some(Key::Char(b as char)), 1),
                b => {
                    let n = if b & 0xe0 == 0xc0 {
                        2
                    } else if b & 0xf0 == 0xe0 {
                        3
                    } else if b & 0xf8 == 0xf0 {
                        4
                    } else {
                        1
                    };
                    if self.bytes.len() < n {
                        if idle {
                            self.bytes.clear();
                        }
                        break;
                    }
                    let c = std::str::from_utf8(&self.bytes[..n])
                        .ok()
                        .and_then(|s| s.chars().next())
                        .filter(|c| !c.is_control());
                    (c.map(Key::Char), n)
                }
            };
            self.bytes.drain(..count);
            if let Some(key) = key {
                out.push(key);
            }
        }
        out
    }
}
fn mouse(raw: &[u8]) -> Option<Key> {
    if raw.first() != Some(&b'<') {
        return None;
    }
    let released = *raw.last()? == b'm';
    if !released && *raw.last()? != b'M' {
        return None;
    }
    let mut fields = std::str::from_utf8(&raw[1..raw.len() - 1]).ok()?.split(';');
    let button = fields.next()?.parse().ok()?;
    let column: u16 = fields.next()?.parse().ok()?;
    let row: u16 = fields.next()?.parse().ok()?;
    if fields.next().is_some() || column == 0 || row == 0 {
        return None;
    }
    Some(Key::Mouse {
        button,
        column: column - 1,
        row: row - 1,
        released,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_utf8_and_mouse() {
        let mut d = Decoder::default();
        assert!(d.feed(b"\x1b[<0;12;").is_empty());
        assert_eq!(
            d.feed(b"4M"),
            vec![Key::Mouse {
                button: 0,
                column: 11,
                row: 3,
                released: false
            }]
        );
        assert!(d.feed(&[0xc3]).is_empty());
        assert_eq!(d.feed(&[0xa9]), vec![Key::Char('é')]);
    }
    #[test]
    fn escape_and_hostile_sequences_are_bounded() {
        let mut d = Decoder::default();
        assert!(d.feed(b"\x1b").is_empty());
        assert_eq!(d.idle(), vec![Key::Escape]);
        assert!(d.feed(&vec![b'x'; 8193]).is_empty());
        assert_eq!(d.feed(b"\x1b[999~k"), vec![Key::Char('k')]);
    }
    #[test]
    fn fragmented_paste_end_is_retained() {
        let mut d = Decoder::default();
        assert!(d.feed(b"\x1b[200~hello\x1b[20").is_empty());
        assert_eq!(d.feed(b"1~"), vec![Key::Paste("hello".into())]);
    }
    #[test]
    fn paste_cannot_dispatch_shortcuts() {
        let mut d = Decoder::default();
        assert!(d.feed(b"\x1b[200~:quit\n").is_empty());
        assert_eq!(d.feed(b"\x1b[201~"), vec![Key::Paste(":quit".into())]);
    }
}
