pub const MAX_LINE_BYTES: usize = 16 * 1024;

const ESCAPE: u8 = 0x1b;
const BELL: u8 = 0x07;
const DELETE: u8 = 0x7f;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Context {
    #[default]
    Text,
    Escape,
    Intermediate,
    ControlSequence,
    StringCommand,
    StringEscape,
}

#[derive(Default)]
pub struct LineAssembler {
    line: Vec<u8>,
    context: Context,
    carriage_return: bool,
}

impl LineAssembler {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in bytes {
            if let Some(line) = self.accept(byte) {
                lines.push(line);
            }
        }
        lines
    }

    pub fn finish(&mut self) -> Option<String> {
        self.context = Context::Text;
        self.carriage_return = false;
        (!self.line.is_empty()).then(|| self.take_line())
    }

    fn accept(&mut self, byte: u8) -> Option<String> {
        self.context = match self.context {
            Context::Text => return self.accept_text(byte),
            Context::Escape => after_escape(byte),
            Context::Intermediate => after_intermediate(byte),
            Context::ControlSequence if (0x40..=0x7e).contains(&byte) => Context::Text,
            Context::ControlSequence => Context::ControlSequence,
            Context::StringCommand => match byte {
                BELL => Context::Text,
                ESCAPE => Context::StringEscape,
                _ => Context::StringCommand,
            },
            Context::StringEscape => Context::Text,
        };
        None
    }

    fn accept_text(&mut self, byte: u8) -> Option<String> {
        match byte {
            b'\n' => {
                self.carriage_return = false;
                return Some(self.take_line());
            }
            b'\r' => self.carriage_return = true,
            ESCAPE => self.context = Context::Escape,
            b'\t' => self.append(byte),
            control if control < 0x20 || control == DELETE => {}
            printable => self.append(printable),
        }
        (self.line.len() >= MAX_LINE_BYTES).then(|| self.take_line())
    }

    fn append(&mut self, byte: u8) {
        if std::mem::take(&mut self.carriage_return) {
            self.line.clear();
        }
        self.line.push(byte);
    }

    fn take_line(&mut self) -> String {
        let line = String::from_utf8_lossy(&self.line).into_owned();
        self.line.clear();
        line
    }
}

fn after_escape(byte: u8) -> Context {
    match byte {
        b'[' => Context::ControlSequence,
        b']' | b'P' | b'X' | b'^' | b'_' => Context::StringCommand,
        0x20..=0x2f => Context::Intermediate,
        _ => Context::Text,
    }
}

fn after_intermediate(byte: u8) -> Context {
    if (0x20..=0x2f).contains(&byte) {
        Context::Intermediate
    } else {
        Context::Text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(chunks: &[&[u8]]) -> Vec<String> {
        let mut assembler = LineAssembler::default();
        let mut lines: Vec<String> = chunks
            .iter()
            .flat_map(|chunk| assembler.push(chunk))
            .collect();
        lines.extend(assembler.finish());
        lines
    }

    #[test]
    fn splits_on_newlines_and_drops_pty_carriage_returns() {
        assert_eq!(lines_of(&[b"one\r\ntwo\r\n"]), ["one", "two"]);
    }

    #[test]
    fn buffers_partial_lines_until_the_newline_arrives() {
        let mut assembler = LineAssembler::default();

        assert!(assembler.push(b"hal").is_empty());
        assert_eq!(assembler.push(b"f\r"), Vec::<String>::new());
        assert_eq!(assembler.push(b"\nnext"), ["half"]);
        assert_eq!(assembler.finish(), Some("next".to_owned()));
        assert_eq!(assembler.finish(), None);
    }

    #[test]
    fn strips_color_and_cursor_sequences_even_when_split_across_chunks() {
        assert_eq!(
            lines_of(&[b"\x1b[1;3", b"2mready\x1b[0m \x1b[2K\x1b", b"[Gnow\n"]),
            ["ready now"]
        );
    }

    #[test]
    fn strips_title_sequences_and_charset_switches() {
        assert_eq!(
            lines_of(&[b"\x1b]0;vite\x07\x1b(Bserved\x1b]2;x\x1b\\ ok\n"]),
            ["served ok"]
        );
    }

    #[test]
    fn a_bare_carriage_return_overwrites_the_line_like_a_terminal() {
        assert_eq!(lines_of(&[b"10%\r50%\r", b"\x1b[K100%\n"]), ["100%"]);
    }

    #[test]
    fn keeps_multibyte_characters_split_across_chunks() {
        assert_eq!(
            lines_of(&[b"caf\xc3", b"\xa9 \xe2\x9c", b"\x93\n"]),
            ["café ✓"]
        );
    }

    #[test]
    fn very_long_lines_are_flushed_in_bounded_pieces() {
        let long = vec![b'x'; MAX_LINE_BYTES * 2 + 10];

        let lines = lines_of(&[&long]);

        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|line| line.len() <= MAX_LINE_BYTES));
    }
}
