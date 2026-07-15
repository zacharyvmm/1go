use std::ops::Range;

pub struct Reader<'a> {
    source: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            source: input.as_bytes(),
            position: 0,
        }
    }

    pub fn from_bytes(input: &'a [u8]) -> Self {
        Self {
            source: input,
            position: 0,
        }
    }

    #[inline]
    pub fn get_position(&self) -> usize {
        self.position
    }

    #[inline]
    pub fn slice(&self, range: Range<usize>) -> &'a str {
        // SAFETY: The source was originally a &str, and structural characters are ASCII.
        // Should be careful about slicing in the middle of a UTF-8 character.
        unsafe { std::str::from_utf8_unchecked(&self.source[range]) }
    }

    #[inline]
    pub fn peek(&self) -> Option<u8> {
        self.source.get(self.position).copied()
    }

    /// Peek at the byte `offset` positions ahead of the cursor without
    /// advancing. Returns `None` when the position is past the end of input.
    #[inline]
    pub fn peek_at(&self, offset: usize) -> Option<u8> {
        self.source.get(self.position + offset).copied()
    }

    #[inline]
    pub fn next_while_list(&mut self, characters: &[u8]) {
        let len = self.source.len();
        while self.position < len && characters.contains(&self.source[self.position]) {
            self.position += 1;
        }
    }

    #[inline]
    pub fn next_while(&mut self, character: u8) {
        let len = self.source.len();
        while self.position < len && self.source[self.position] == character {
            self.position += 1;
        }
    }

    #[inline]
    pub fn next_until_list(&mut self, characters: &[u8]) {
        let len = self.source.len();
        while self.position < len && !characters.contains(&self.source[self.position]) {
            self.position += 1;
        }
    }

    pub fn next_until(&mut self, character: u8) {
        let len = self.source.len();
        while self.position < len && self.source[self.position] != character {
            self.position += 1;
        }
    }

    /// Advance past characters until an unescaped `delimiter` byte, skipping
    /// over any `delimiter` that is preceded by an odd run of `escape` bytes.
    ///
    /// Uses a delimiter-first strategy: find the next delimiter, then scan
    /// backward through the immediately preceding escape run to decide
    /// whether it is escaped. This avoids per-byte escape-parity tracking
    /// on the hot path, which matters for ordinary (no-backslash) values.
    ///
    /// Odd-length escape runs escape the delimiter; even-length runs do not.
    /// Any non-escape byte resets the escape-run parity.
    ///
    /// On return the cursor sits at the unescaped delimiter, or at
    /// `source.len()` if no unescaped delimiter was found.
    #[inline]
    pub fn next_until_unescaped(&mut self, delimiter: u8, escape: u8) {
        let len = self.source.len();

        while self.position < len {
            // Find the next delimiter byte
            let slice = &self.source[self.position..];
            match slice.iter().position(|&b| b == delimiter) {
                None => {
                    self.position = len;
                    return;
                }
                Some(offset) => {
                    let candidate = self.position + offset;

                    // Count consecutive escape bytes immediately before the delimiter
                    let mut esc_count = 0usize;
                    let mut scan = candidate;
                    while scan > 0 && self.source[scan - 1] == escape {
                        esc_count += 1;
                        scan -= 1;
                    }

                    if esc_count.is_multiple_of(2) {
                        // Even run: delimiter is unescaped
                        self.position = candidate;
                        return;
                    }

                    // Odd run: delimiter is escaped; skip past it and continue
                    self.position = candidate + 1;
                }
            }
        }
    }

    pub fn skip(&mut self) {
        if self.position < self.source.len() {
            self.position += 1;
        }
    }

    #[inline]
    pub fn eof(&self) -> bool {
        if self.position >= self.source.len() {
            return true;
        }

        self.source[self.position..]
            .iter()
            .all(|b| b.is_ascii_whitespace())
    }

    /// Check whether the bytes immediately before the cursor match `suffix`,
    /// without interpreting them as UTF-8. Safe to call when the cursor sits
    /// in the middle of a multibyte character.
    #[inline]
    pub fn preceding_bytes_eq(&self, suffix: &[u8]) -> bool {
        let position = self.position;
        position >= suffix.len() && &self.source[position - suffix.len()..position] == suffix
    }

    pub fn match_ignore_case(&self, s: &str) -> bool {
        if self.position + s.len() > self.source.len() {
            return false;
        }
        let slice = &self.source[self.position..self.position + s.len()];
        slice.eq_ignore_ascii_case(s.as_bytes())
    }
}

impl<'a> Iterator for Reader<'a> {
    type Item = u8;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.position < self.source.len() {
            let b = self.source[self.position];
            self.position += 1;
            Some(b)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_iterator() {
        let my_string = String::from("Hello World");
        let mut reader = Reader::new(&my_string);

        assert_eq!(reader.get_position(), 0);
        assert_eq!(reader.next(), Some(b'H'));

        assert_eq!(reader.get_position(), 1);
        assert_eq!(reader.next(), Some(b'e'));

        assert_eq!(reader.get_position(), 2);
        assert_eq!(reader.next(), Some(b'l'));

        assert_eq!(reader.get_position(), 3);
        assert_eq!(reader.next(), Some(b'l'));

        assert_eq!(reader.get_position(), 4);
        assert_eq!(reader.next(), Some(b'o'));

        assert_eq!(reader.get_position(), 5);
        assert_eq!(reader.next(), Some(b' '));

        assert_eq!(reader.get_position(), 6);
        assert_eq!(reader.next(), Some(b'W'));

        assert_eq!(reader.get_position(), 7);
        assert_eq!(reader.next(), Some(b'o'));

        assert_eq!(reader.get_position(), 8);
        assert_eq!(reader.next(), Some(b'r'));

        assert_eq!(reader.get_position(), 9);
        assert_eq!(reader.next(), Some(b'l'));

        assert_eq!(reader.get_position(), 10);
        assert_eq!(reader.peek(), Some(b'd'));

        assert_eq!(reader.get_position(), 10);
        assert_eq!(reader.next(), Some(b'd'));

        assert_eq!(reader.slice(0..5), "Hello");
    }

    #[test]
    fn next_until_stops_at_delimiter() {
        let mut reader = Reader::new("abc<def");

        reader.next_until(b'<');

        assert_eq!(reader.get_position(), 3);
        assert_eq!(reader.peek(), Some(b'<'));
    }

    #[test]
    fn next_until_moves_to_end_when_delimiter_is_absent() {
        let mut reader = Reader::new("abcdef");

        reader.next_until(b'<');

        assert_eq!(reader.get_position(), 6);
        assert_eq!(reader.peek(), None);
    }

    #[test]
    fn next_until_does_not_advance_when_delimiter_is_current() {
        let mut reader = Reader::new("<abcdef");

        reader.next_until(b'<');

        assert_eq!(reader.get_position(), 0);
        assert_eq!(reader.peek(), Some(b'<'));
    }

    #[test]
    fn next_until_handles_long_spans() {
        let mut input = "a".repeat(16 * 1024);
        input.push('<');
        input.push_str("tail");
        let mut reader = Reader::new(&input);

        reader.next_until(b'<');

        assert_eq!(reader.get_position(), 16 * 1024);
        assert_eq!(reader.peek(), Some(b'<'));
    }

    // ── next_until_unescaped ───────────────────────────────────

    // Delimiter-first scanning: candidate delimiters are validated by counting
    // the immediately preceding escape run.

    #[test]
    fn next_until_unescaped_stops_at_unescaped_delimiter() {
        let mut reader = Reader::new(r#"abc"def"#);

        reader.next_until_unescaped(b'"', b'\\');

        assert_eq!(reader.get_position(), 3);
        assert_eq!(reader.peek(), Some(b'"'));
    }

    #[test]
    fn next_until_unescaped_skips_odd_escape_run() {
        // abc\"def"ghi  →  \" is an odd run, so the first quote is escaped;
        // the later quote at position 10 is unescaped.
        let mut reader = Reader::new(r#"abc\"def"ghi"#);

        reader.next_until_unescaped(b'"', b'\\');

        // Content before the unescaped quote includes the escaped quote.
        assert_eq!(reader.slice(0..reader.get_position()), r#"abc\"def"#);
        assert_eq!(reader.peek(), Some(b'"'));
    }

    #[test]
    fn next_until_unescaped_stops_after_even_escape_run() {
        // abc\\"def  →  \\ is even, so the quote closes.
        let mut reader = Reader::new(r#"abc\\"def"#);

        reader.next_until_unescaped(b'"', b'\\');

        assert_eq!(reader.slice(0..reader.get_position()), r#"abc\\"#);
        assert_eq!(reader.peek(), Some(b'"'));
    }

    #[test]
    fn next_until_unescaped_skips_triple_escape_run() {
        // abc\\\"def"ghi  →  \\\ is odd (3), so the first quote (pos 6) is
        // escaped; the second quote at position 10 is unescaped.
        let mut reader = Reader::new(r#"abc\\\"def"ghi"#);

        reader.next_until_unescaped(b'"', b'\\');

        assert_eq!(reader.get_position(), 10);
        assert_eq!(reader.peek(), Some(b'"'));
    }

    #[test]
    fn next_until_unescaped_reaches_eof_when_only_delimiter_is_escaped() {
        // abc\"def  →  only quote is escaped; no unescaped quote exists.
        let mut reader = Reader::new(r#"abc\"def"#);

        reader.next_until_unescaped(b'"', b'\\');

        assert_eq!(reader.get_position(), 8);
        assert_eq!(reader.peek(), None);
    }

    #[test]
    fn next_until_unescaped_non_escape_byte_resets_parity() {
        // \a"  →  \ sets parity true, a resets it false, " closes.
        let mut reader = Reader::new(r#"\a""#);

        reader.next_until_unescaped(b'"', b'\\');

        assert_eq!(reader.get_position(), 2);
        assert_eq!(reader.peek(), Some(b'"'));
    }

    #[test]
    fn next_until_unescaped_delimiter_without_escape_byte() {
        // No escape bytes at all — stops at first delimiter.
        let mut reader = Reader::new("data:more");
        reader.next_until_unescaped(b':', b'\\');

        assert_eq!(reader.get_position(), 4);
        assert_eq!(reader.peek(), Some(b':'));
    }
}
