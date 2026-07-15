use super::builder::ElementPredicate;
use super::is_css_whitespace;
use crate::Reader;
use crate::query::compiler::SelectorParseError;

#[derive(Debug, PartialEq, Clone)]
pub enum Combinator {
    // u4: Last Element Depth (size of stack)
    Child,       // `>`
    Descendant,  // ` `
    NextSibling, // `+`

    // BUG: By definition of this Combinator it's a SelectAll query
    SubsequentSibling, // `~`

    // I'm pretty sure this does not apply to the scope of the project.
    Namespace, // `|`
}

impl Combinator {
    fn next<'a>(reader: &mut Reader<'a>) -> Option<Self> {
        if let Some(token) = reader.peek()
            && !matches!(token, b'>' | b' ' | b'+' | b'~' | b'|')
        {
            return None;
        }

        match reader.next()? {
            b'>' => Some(Self::Child),
            b' ' => Some(Self::Descendant),
            b'+' => Some(Self::NextSibling),
            b'~' => Some(Self::SubsequentSibling),
            b'|' => Some(Self::Namespace),
            _ => panic!("Not possible root"),
        }
    }
}

impl<'a> Combinator {
    pub fn try_from(reader: &mut Reader<'a>) -> Option<Self> {
        let mut combinator: Option<Self> = None;
        while let Some(next_combinator) = Combinator::next(reader) {
            match combinator {
                Option::None => combinator = Some(next_combinator),
                Some(c) if c == Self::Descendant && next_combinator != Self::Descendant => {
                    combinator = Some(next_combinator);
                }
                _ => (),
            }
        }

        combinator
    }

    pub(crate) fn evaluate(&self, last_depth: u16, current_depth: u16) -> bool {
        match self {
            Combinator::Child => last_depth + 1 == current_depth,
            Combinator::Descendant => last_depth == 0 || current_depth != last_depth,

            // BUG: I need to know if it's the element right after
            // TODO: After first Fail it goes back
            Combinator::NextSibling => last_depth == current_depth,

            // BUG: I need to know if it's found a match before, so I know if it's ON/OFF
            Combinator::SubsequentSibling => true,

            Combinator::Namespace => panic!("Why are you using Namespace Selector ???"),
        }
    }
}

pub struct Lexer {}
impl Lexer {
    #[cfg(test)]
    pub fn next<'query>(
        reader: &mut Reader<'query>,
    ) -> Option<(Combinator, ElementPredicate<'query>)> {
        Self::try_next(reader, false).unwrap()
    }

    pub fn try_next<'query>(
        reader: &mut Reader<'query>,
        seen_selector: bool,
    ) -> Result<Option<(Combinator, ElementPredicate<'query>)>, SelectorParseError> {
        if reader.eof() {
            return Ok(None);
        }

        let combinator = Self::parse_combinator(reader, seen_selector)?;
        let element = ElementPredicate::try_from(reader)?;

        Ok(Some((combinator, element)))
    }

    fn parse_combinator<'query>(
        reader: &mut Reader<'query>,
        seen_selector: bool,
    ) -> Result<Combinator, SelectorParseError> {
        let mut saw_whitespace = false;
        while let Some(token) = reader.peek() {
            if !is_css_whitespace(token) {
                break;
            }
            saw_whitespace = true;
            reader.skip();
        }

        match reader.peek() {
            None => Err(SelectorParseError::new(
                "missing selector after combinator",
                reader.get_position(),
            )),
            Some(b'>') => {
                reader.skip();
                while let Some(token) = reader.peek() {
                    if !is_css_whitespace(token) {
                        break;
                    }
                    reader.skip();
                }
                Ok(Combinator::Child)
            }
            Some(b'+') => Err(SelectorParseError::new(
                "unsupported combinator '+'",
                reader.get_position(),
            )),
            Some(b'~') => Err(SelectorParseError::new(
                "unsupported combinator '~'",
                reader.get_position(),
            )),
            Some(b'|') => Err(SelectorParseError::new(
                "unsupported combinator '|'",
                reader.get_position(),
            )),
            Some(_) if saw_whitespace || !seen_selector => Ok(Combinator::Descendant),
            Some(_) => Ok(Combinator::Descendant),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AttributeSelections, ClassSelections};

    #[test]
    fn test_whitespace_only_returns_none() {
        let mut reader = Reader::new("   \n\t  ");
        assert_eq!(Lexer::try_next(&mut reader, false).unwrap(), None);
    }

    #[test]
    fn test_leading_whitespace_uses_descendant_combinator() {
        let mut reader = Reader::new("   article#main.hero");
        let (combinator, element) = Lexer::try_next(&mut reader, false).unwrap().unwrap();

        assert_eq!(combinator, Combinator::Descendant);
        assert_eq!(
            element,
            ElementPredicate {
                name: Some("article"),
                id: Some("main"),
                classes: ClassSelections::from_static(&["hero"]),
                attributes: AttributeSelections::from_static(&[]),
            }
        );
    }

    #[test]
    fn test_basic_element_selection_with_combinator() {
        let mut reader = Reader::new("element#id.class > other#other_id.other_class");
        let (first_combinator, first_element) = Lexer::next(&mut reader).unwrap();
        let (second_combinator, second_element) = Lexer::next(&mut reader).unwrap();

        assert_eq!(first_combinator, Combinator::Descendant);

        assert_eq!(
            first_element,
            ElementPredicate {
                name: Some("element"),
                id: Some("id"),
                classes: ClassSelections::from_static(&["class"]),
                attributes: AttributeSelections::from_static(&[])
            }
        );

        assert_eq!(second_combinator, Combinator::Child);

        assert_eq!(
            second_element,
            ElementPredicate {
                name: Some("other"),
                id: Some("other_id"),
                classes: ClassSelections::from_static(&["other_class"]),
                attributes: AttributeSelections::from_static(&[]),
            }
        );
    }

    #[test]
    fn test_unsupported_combinator_leading_selector() {
        let mut reader = Reader::new("~ element#id.class > other#other_id.other_class");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "unsupported combinator '~'");
    }

    #[test]
    fn test_child_combinator_after_seen_selector() {
        let mut reader = Reader::new("> span.highlight");
        let (combinator, element) = Lexer::try_next(&mut reader, true).unwrap().unwrap();

        assert_eq!(combinator, Combinator::Child);
        assert_eq!(
            element,
            ElementPredicate {
                name: Some("span"),
                id: None,
                classes: ClassSelections::from_static(&["highlight"]),
                attributes: AttributeSelections::from_static(&[]),
            }
        );
    }

    #[test]
    fn test_missing_selector_after_child_combinator() {
        let mut reader = Reader::new(">   ");
        let error = Lexer::try_next(&mut reader, true).unwrap_err();

        assert_eq!(error.message(), "missing selector element");
    }

    #[test]
    fn test_unsupported_adjacent_sibling_combinator_after_selector() {
        let mut reader = Reader::new("+ a");
        let error = Lexer::try_next(&mut reader, true).unwrap_err();

        assert_eq!(error.message(), "unsupported combinator '+'");
    }

    #[test]
    fn test_unsupported_namespace_combinator_after_selector() {
        let mut reader = Reader::new("| a");
        let error = Lexer::try_next(&mut reader, true).unwrap_err();

        assert_eq!(error.message(), "unsupported combinator '|'");
    }

    #[test]
    fn test_illegal_character_bang() {
        let mut reader = Reader::new("!");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn test_illegal_character_at() {
        let mut reader = Reader::new("@");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn test_illegal_character_dollar() {
        let mut reader = Reader::new("$");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn test_illegal_character_percent() {
        let mut reader = Reader::new("%");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn test_illegal_character_caret() {
        let mut reader = Reader::new("^");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn test_illegal_character_ampersand() {
        let mut reader = Reader::new("&");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn test_illegal_character_open_paren() {
        let mut reader = Reader::new("(");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn test_illegal_character_close_paren() {
        let mut reader = Reader::new(")");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn test_illegal_character_question_mark() {
        let mut reader = Reader::new("?");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn test_illegal_character_backtick() {
        let mut reader = Reader::new("`");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn test_illegal_character_less_than() {
        let mut reader = Reader::new("<");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn test_illegal_character_colon() {
        let mut reader = Reader::new(":");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn test_illegal_character_semicolon() {
        let mut reader = Reader::new(";");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "illegal selector token");
    }

    // ── Child combinator decomposition ──────────────────────────

    // Verify full decomposition: two elements, correct combinators,
    // and the reader is exhausted.

    #[test]
    fn child_combinator_without_spaces_decomposes() {
        let mut reader = Reader::new("main>section");
        let (first_guard, first) = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let (second_guard, second) = Lexer::try_next(&mut reader, true).unwrap().unwrap();

        assert_eq!(first_guard, Combinator::Descendant);
        assert_eq!(first.name, Some("main"));

        assert_eq!(second_guard, Combinator::Child);
        assert_eq!(second.name, Some("section"));

        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn child_combinator_with_left_space_decomposes() {
        let mut reader = Reader::new("main >section");
        let (first_guard, first) = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let (second_guard, second) = Lexer::try_next(&mut reader, true).unwrap().unwrap();

        assert_eq!(first_guard, Combinator::Descendant);
        assert_eq!(first.name, Some("main"));

        assert_eq!(second_guard, Combinator::Child);
        assert_eq!(second.name, Some("section"));

        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn child_combinator_with_right_space_decomposes() {
        let mut reader = Reader::new("main> section");
        let (first_guard, first) = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let (second_guard, second) = Lexer::try_next(&mut reader, true).unwrap().unwrap();

        assert_eq!(first_guard, Combinator::Descendant);
        assert_eq!(first.name, Some("main"));

        assert_eq!(second_guard, Combinator::Child);
        assert_eq!(second.name, Some("section"));

        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn child_combinator_with_both_spaces_decomposes() {
        let mut reader = Reader::new("main > section");
        let (first_guard, first) = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let (second_guard, second) = Lexer::try_next(&mut reader, true).unwrap().unwrap();

        assert_eq!(first_guard, Combinator::Descendant);
        assert_eq!(first.name, Some("main"));

        assert_eq!(second_guard, Combinator::Child);
        assert_eq!(second.name, Some("section"));

        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    // ── Descendant combinator whitespace ────────────────────────

    #[test]
    fn space_descendant_combinator_decomposes() {
        let mut reader = Reader::new("main section");
        let (first_guard, first) = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let (second_guard, second) = Lexer::try_next(&mut reader, true).unwrap().unwrap();

        assert_eq!(first_guard, Combinator::Descendant);
        assert_eq!(first.name, Some("main"));

        assert_eq!(second_guard, Combinator::Descendant);
        assert_eq!(second.name, Some("section"));

        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn tab_descendant_combinator_decomposes() {
        let mut reader = Reader::new("main\tsection");
        let (first_guard, first) = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let (second_guard, second) = Lexer::try_next(&mut reader, true).unwrap().unwrap();

        assert_eq!(first_guard, Combinator::Descendant);
        assert_eq!(first.name, Some("main"));

        assert_eq!(second_guard, Combinator::Descendant);
        assert_eq!(second.name, Some("section"));

        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn newline_descendant_combinator_decomposes() {
        let mut reader = Reader::new("main\nsection");
        let (first_guard, first) = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let (second_guard, second) = Lexer::try_next(&mut reader, true).unwrap().unwrap();

        assert_eq!(first_guard, Combinator::Descendant);
        assert_eq!(first.name, Some("main"));

        assert_eq!(second_guard, Combinator::Descendant);
        assert_eq!(second.name, Some("section"));

        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn carriage_return_descendant_combinator_decomposes() {
        let mut reader = Reader::new("main\rsection");
        let (first_guard, first) = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let (second_guard, second) = Lexer::try_next(&mut reader, true).unwrap().unwrap();

        assert_eq!(first_guard, Combinator::Descendant);
        assert_eq!(first.name, Some("main"));

        assert_eq!(second_guard, Combinator::Descendant);
        assert_eq!(second.name, Some("section"));

        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn form_feed_descendant_combinator_decomposes() {
        let mut reader = Reader::new("main\u{000C}section");
        let (first_guard, first) = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let (second_guard, second) = Lexer::try_next(&mut reader, true).unwrap().unwrap();

        assert_eq!(first_guard, Combinator::Descendant);
        assert_eq!(first.name, Some("main"));

        assert_eq!(second_guard, Combinator::Descendant);
        assert_eq!(second.name, Some("section"));

        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn vertical_tab_is_not_css_whitespace() {
        // 0x0B is NOT CSS whitespace, so it should NOT silently behave as a
        // descendant combinator. The parser must reject it as an illegal token
        // rather than treating it as ordinary whitespace.
        let mut reader = Reader::new("main\u{000B}section");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();
        assert_eq!(error.message(), "illegal selector token");
        // The error position should be near the offending character.
        assert!(error.position() <= 12);
    }
}
