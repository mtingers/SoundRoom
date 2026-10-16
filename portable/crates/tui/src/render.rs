//! Fixed-size, escaped cell canvas and differential ANSI output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub fg: u8,
    pub bg: u8,
    pub bold: bool,
}
impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: 252,
            bg: 234,
            bold: false,
        }
    }
}
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<Cell>,
}
impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        let width = width.clamp(20, 500);
        let height = height.clamp(8, 200);
        Self {
            width,
            height,
            cells: vec![Cell::default(); width * height],
        }
    }
    pub fn put(&mut self, x: usize, y: usize, ch: char, fg: u8, bg: u8, bold: bool) {
        if x < self.width && y < self.height {
            self.cells[y * self.width + x] = Cell {
                ch: if ch.is_ascii_graphic() || ch == ' ' {
                    ch
                } else {
                    '?'
                },
                fg,
                bg,
                bold,
            };
        }
    }
    pub fn text(&mut self, x: usize, y: usize, text: &str, fg: u8, bg: u8, bold: bool) {
        for (i, ch) in text.chars().enumerate() {
            if x + i >= self.width {
                break;
            }
            self.put(x + i, y, ch, fg, bg, bold);
        }
    }
    pub fn fill(&mut self, x: usize, y: usize, width: usize, ch: char, fg: u8, bg: u8) {
        for i in 0..width.min(self.width.saturating_sub(x)) {
            self.put(x + i, y, ch, fg, bg, false);
        }
    }
    pub fn diff(&self, previous: Option<&Canvas>) -> String {
        let compatible = previous.filter(|p| p.width == self.width && p.height == self.height);
        let mut output = String::new();
        let mut style = None;
        for y in 0..self.height {
            let mut x = 0;
            while x < self.width {
                let i = y * self.width + x;
                if compatible.is_some_and(|p| p.cells[i] == self.cells[i]) {
                    x += 1;
                    continue;
                }
                output.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
                while x < self.width {
                    let i = y * self.width + x;
                    let c = self.cells[i];
                    if compatible.is_some_and(|p| p.cells[i] == c) {
                        break;
                    }
                    let next = (c.fg, c.bg, c.bold);
                    if style != Some(next) {
                        output.push_str(&format!(
                            "\x1b[{};38;5;{};48;5;{}m",
                            if c.bold { 1 } else { 22 },
                            c.fg,
                            c.bg
                        ));
                        style = Some(next);
                    }
                    output.push(c.ch);
                    x += 1;
                }
            }
        }
        output
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_text_cannot_inject_terminal_sequences() {
        let mut c = Canvas::new(20, 8);
        c.text(0, 0, "voice\x1b[2J\n", 200, 234, false);
        assert_eq!(c.cells[5].ch, '?');
        let s = c.diff(None);
        assert!(!s.contains("voice\x1b[2J"));
    }
    #[test]
    fn stable_frame_emits_no_output_and_one_change_is_bounded() {
        let mut a = Canvas::new(200, 60);
        assert_eq!(a.diff(Some(&a)), "");
        let b = Canvas::new(200, 60);
        a.put(199, 59, 'x', 100, 234, false);
        assert!(a.diff(Some(&b)).len() < 60);
    }
}
