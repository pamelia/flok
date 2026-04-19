use std::io::Write;

pub(crate) struct Clipboard {
    inner: Option<arboard::Clipboard>,
    pub(crate) last_copied_text: Option<String>,
}

impl Clipboard {
    pub(crate) fn new() -> Self {
        Self { inner: None, last_copied_text: None }
    }

    pub(crate) fn copy(&mut self, text: &str) -> bool {
        self.last_copied_text = Some(text.to_string());
        if text.is_empty() {
            return true;
        }

        if self.inner.is_none() {
            self.inner = arboard::Clipboard::new().ok();
        }

        if let Some(clipboard) = self.inner.as_mut() {
            match clipboard.set_text(text) {
                Ok(()) => return true,
                Err(error) => {
                    tracing::debug!(%error, "native clipboard write failed; falling back to OSC 52");
                    self.inner = None;
                }
            }
        }

        write_osc52_to_stderr(text)
    }
}

fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }

    out
}

fn osc52_envelope(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
}

fn write_osc52_to_stderr(text: &str) -> bool {
    let envelope = osc52_envelope(text);
    let mut stderr = std::io::stderr();
    stderr.write_all(envelope.as_bytes()).is_ok() && stderr.flush().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encode_empty() {
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn base64_encode_hello() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn osc52_envelope_format() {
        assert_eq!(osc52_envelope("hello"), "\x1b]52;c;aGVsbG8=\x07");
    }

    #[test]
    fn copy_empty_returns_true() {
        let mut clipboard = Clipboard::new();
        assert!(clipboard.copy(""));
    }
}
