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
        let remaining = &self.source[self.position..];
        self.position += memchr::memchr(character, remaining).unwrap_or(remaining.len());
    }

    pub fn skip(&mut self) {
        if self.position < self.source.len() {
            self.position += 1;
        }
    }

    pub fn eof(&self) -> bool {
        if self.position >= self.source.len() {
            return true;
        }

        self.source[self.position..]
            .iter()
            .all(|b| b.is_ascii_whitespace())
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
}
