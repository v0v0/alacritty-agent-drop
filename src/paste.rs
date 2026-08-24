const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

#[derive(Debug, PartialEq, Eq)]
pub enum InputEvent {
    Bytes(Vec<u8>),
    Paste(Vec<u8>),
}

#[derive(Default)]
pub struct BracketedPasteParser {
    buffer: Vec<u8>,
    in_paste: bool,
}

impl BracketedPasteParser {
    pub fn feed(&mut self, input: &[u8]) -> Vec<InputEvent> {
        self.buffer.extend_from_slice(input);
        let mut events = Vec::new();

        loop {
            if self.in_paste {
                let Some(end) = find_subslice(&self.buffer, PASTE_END) else {
                    break;
                };

                let payload = self.buffer[..end].to_vec();
                self.buffer.drain(..end + PASTE_END.len());
                self.in_paste = false;
                events.push(InputEvent::Paste(payload));
                continue;
            }

            if let Some(start) = find_subslice(&self.buffer, PASTE_START) {
                if start > 0 {
                    events.push(InputEvent::Bytes(self.buffer[..start].to_vec()));
                }
                self.buffer.drain(..start + PASTE_START.len());
                self.in_paste = true;
                continue;
            }

            let keep = longest_suffix_prefix(&self.buffer, PASTE_START);
            let emit = self.buffer.len().saturating_sub(keep);
            if emit > 0 {
                events.push(InputEvent::Bytes(self.buffer[..emit].to_vec()));
                self.buffer.drain(..emit);
            }
            break;
        }

        events
    }

    pub fn finish(&mut self) -> Vec<InputEvent> {
        if self.buffer.is_empty() && !self.in_paste {
            return Vec::new();
        }

        let mut bytes = Vec::new();
        if self.in_paste {
            bytes.extend_from_slice(PASTE_START);
        }
        bytes.append(&mut self.buffer);
        self.in_paste = false;

        vec![InputEvent::Bytes(bytes)]
    }
}

pub fn write_bracketed_paste<W: std::io::Write>(writer: &mut W, payload: &[u8]) -> std::io::Result<()> {
    writer.write_all(PASTE_START)?;
    writer.write_all(payload)?;
    writer.write_all(PASTE_END)?;
    writer.flush()
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn longest_suffix_prefix(bytes: &[u8], prefix: &[u8]) -> usize {
    let max = bytes.len().min(prefix.len().saturating_sub(1));
    (1..=max)
        .rev()
        .find(|&len| bytes[bytes.len() - len..] == prefix[..len])
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_normal_input() {
        let mut parser = BracketedPasteParser::default();
        assert_eq!(
            parser.feed(b"hello"),
            vec![InputEvent::Bytes(b"hello".to_vec())]
        );
    }

    #[test]
    fn forwards_control_keys_immediately() {
        let mut parser = BracketedPasteParser::default();
        for byte in [0x01_u8, 0x05_u8, 0x12_u8] {
            assert_eq!(
                parser.feed(&[byte]),
                vec![InputEvent::Bytes(vec![byte])],
                "control byte {byte:#04x} must not be buffered"
            );
        }
    }

    #[test]
    fn forwards_arrow_sequences_once_they_diverge_from_paste_prefix() {
        for arrow in [b'A', b'B', b'C', b'D'] {
            let mut parser = BracketedPasteParser::default();
            assert!(parser.feed(b"\x1b").is_empty());
            assert!(parser.feed(b"[").is_empty());
            assert_eq!(
                parser.feed(&[arrow]),
                vec![InputEvent::Bytes(vec![0x1b, b'[', arrow])]
            );
        }
    }

    #[test]
    fn parses_paste_across_chunks() {
        let mut parser = BracketedPasteParser::default();
        assert_eq!(parser.feed(b"abc\x1b[20"), vec![InputEvent::Bytes(b"abc".to_vec())]);
        assert!(parser.feed(b"0~C:\\Users\\me\\shot").is_empty());
        assert_eq!(
            parser.feed(b".png\x1b[201~xyz"),
            vec![
                InputEvent::Paste(b"C:\\Users\\me\\shot.png".to_vec()),
                InputEvent::Bytes(b"xyz".to_vec())
            ]
        );
    }

    #[test]
    fn preserves_unfinished_paste_on_eof() {
        let mut parser = BracketedPasteParser::default();
        assert!(parser.feed(b"\x1b[200~unfinished").is_empty());
        assert_eq!(
            parser.finish(),
            vec![InputEvent::Bytes(b"\x1b[200~unfinished".to_vec())]
        );
    }
}
