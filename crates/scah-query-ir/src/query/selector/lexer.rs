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
    /// Later element siblings under the same parent (same document depth).
    SubsequentSibling, // `~`

    // I'm pretty sure this does not apply to the scope of the project.
    Namespace, // `|`
}

impl Combinator {
    fn next(reader: &mut Reader<'_>) -> Option<Self> {
        match reader.peek()? {
            b'>' => {
                reader.skip();
                Some(Self::Child)
            }
            b'+' => {
                reader.skip();
                Some(Self::NextSibling)
            }
            b'~' => {
                reader.skip();
                Some(Self::SubsequentSibling)
            }
            b'|' => {
                reader.skip();
                Some(Self::Namespace)
            }
            byte if is_css_whitespace(byte) => {
                reader.skip();
                Some(Self::Descendant)
            }
            _ => None,
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
            Combinator::NextSibling | Combinator::SubsequentSibling => last_depth == current_depth,
            Combinator::Namespace => panic!("Why are you using Namespace Selector ???"),
        }
    }
}

pub struct Lexer {}
impl Lexer {
    #[cfg(test)]
    pub fn next<'query>(
        reader: &mut Reader<'query>,
        seen_selector: bool,
    ) -> Option<(Combinator, ElementPredicate<'query>)> {
        Self::try_next(reader, seen_selector).unwrap()
    }

    pub fn try_next<'query>(
        reader: &mut Reader<'query>,
        seen_selector: bool,
    ) -> Result<Option<(Combinator, ElementPredicate<'query>)>, SelectorParseError> {
        let Some(combinator) = Self::parse_combinator(reader, seen_selector)? else {
            return Ok(None);
        };

        let element = ElementPredicate::try_from(reader)?;
        Ok(Some((combinator, element)))
    }

    fn skip_css_whitespace(reader: &mut Reader<'_>) {
        while let Some(token) = reader.peek() {
            if !is_css_whitespace(token) {
                break;
            }
            reader.skip();
        }
    }

    fn parse_combinator<'query>(
        reader: &mut Reader<'query>,
        seen_selector: bool,
    ) -> Result<Option<Combinator>, SelectorParseError> {
        Self::skip_css_whitespace(reader);

        match reader.peek() {
            None => Ok(None),
            Some(b'>') => {
                reader.skip();
                Self::skip_css_whitespace(reader);
                Ok(Some(Combinator::Child))
            }
            Some(b'+') => {
                if !seen_selector {
                    return Err(SelectorParseError::new(
                        "combinator '+' requires a preceding selector",
                        reader.get_position(),
                    ));
                }
                reader.skip();
                Self::skip_css_whitespace(reader);
                Ok(Some(Combinator::NextSibling))
            }
            Some(b'~') => {
                if !seen_selector {
                    return Err(SelectorParseError::new(
                        "combinator '~' requires a preceding selector",
                        reader.get_position(),
                    ));
                }
                reader.skip();
                Self::skip_css_whitespace(reader);
                Ok(Some(Combinator::SubsequentSibling))
            }
            Some(b'|') => Err(SelectorParseError::new(
                "unsupported combinator '|'",
                reader.get_position(),
            )),
            Some(_) => Ok(Some(Combinator::Descendant)),
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
        let (first_combinator, first_element) = Lexer::next(&mut reader, false).unwrap();
        let (second_combinator, second_element) = Lexer::next(&mut reader, true).unwrap();

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
    fn test_leading_subsequent_sibling_requires_selector() {
        let mut reader = Reader::new("~ element#id.class > other#other_id.other_class");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(
            error.message(),
            "combinator '~' requires a preceding selector"
        );
    }

    #[test]
    fn test_child_combinator_after_seen_selector() {
        let mut reader = Reader::new("> span.highlight");
        let (combinator, element) = Lexer::try_next(&mut reader, false).unwrap().unwrap();

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
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "missing selector element");
    }

    #[test]
    fn test_leading_adjacent_sibling_requires_selector() {
        let mut reader = Reader::new("+ a");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(
            error.message(),
            "combinator '+' requires a preceding selector"
        );
    }

    #[test]
    fn test_unsupported_namespace_combinator_after_selector() {
        let mut reader = Reader::new("| a");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();

        assert_eq!(error.message(), "unsupported combinator '|'");
    }

    // ── Sibling combinators ─────────────────────────────────────

    fn assert_sibling_pair(selector: &str, expected: Combinator) {
        let mut reader = Reader::new(selector);
        let (first_guard, first) = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let (second_guard, second) = Lexer::try_next(&mut reader, true).unwrap().unwrap();

        assert_eq!(first_guard, Combinator::Descendant);
        assert_eq!(first.name, Some("h1"));
        assert_eq!(second_guard, expected);
        assert_eq!(second.name, Some("p"));
        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn adjacent_sibling_with_spaces_decomposes() {
        assert_sibling_pair("h1 + p", Combinator::NextSibling);
    }

    #[test]
    fn adjacent_sibling_without_spaces_decomposes() {
        assert_sibling_pair("h1+p", Combinator::NextSibling);
    }

    #[test]
    fn adjacent_sibling_with_extra_spaces_decomposes() {
        assert_sibling_pair("h1   +   p", Combinator::NextSibling);
    }

    #[test]
    fn subsequent_sibling_with_spaces_decomposes() {
        assert_sibling_pair("h1 ~ p", Combinator::SubsequentSibling);
    }

    #[test]
    fn subsequent_sibling_without_spaces_decomposes() {
        assert_sibling_pair("h1~p", Combinator::SubsequentSibling);
    }

    #[test]
    fn subsequent_sibling_with_extra_spaces_decomposes() {
        assert_sibling_pair("h1   ~   p", Combinator::SubsequentSibling);
    }

    #[test]
    fn mixed_child_and_subsequent_sibling_decomposes() {
        let mut reader = Reader::new("main > div ~ p > span");
        let steps = [
            (Combinator::Descendant, "main"),
            (Combinator::Child, "div"),
            (Combinator::SubsequentSibling, "p"),
            (Combinator::Child, "span"),
        ];
        let mut seen = false;
        for (expected_guard, expected_name) in steps {
            let (guard, element) = Lexer::try_next(&mut reader, seen).unwrap().unwrap();
            assert_eq!(guard, expected_guard);
            assert_eq!(element.name, Some(expected_name));
            seen = true;
        }
        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn missing_selector_after_adjacent_sibling_is_rejected() {
        let mut reader = Reader::new("a +");
        let _ = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let error = Lexer::try_next(&mut reader, true).unwrap_err();
        assert_eq!(error.message(), "missing selector element");
    }

    #[test]
    fn missing_selector_after_subsequent_sibling_is_rejected() {
        let mut reader = Reader::new("a ~");
        let _ = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let error = Lexer::try_next(&mut reader, true).unwrap_err();
        assert_eq!(error.message(), "missing selector element");
    }

    #[test]
    fn adjacent_then_subsequent_combinator_sequence_is_rejected() {
        let mut reader = Reader::new("a + ~ b");
        let _ = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let error = Lexer::try_next(&mut reader, true).unwrap_err();
        assert_eq!(error.message(), "missing selector element");
    }

    #[test]
    fn subsequent_then_adjacent_combinator_sequence_is_rejected() {
        let mut reader = Reader::new("a ~ + b");
        let _ = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        let error = Lexer::try_next(&mut reader, true).unwrap_err();
        assert_eq!(error.message(), "missing selector element");
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
    }

    // ── Trailing vertical tab: Fix 1 tests ───────────────────────

    // Case 1: vertical tab after valid CSS whitespace
    #[test]
    fn trailing_vertical_tab_after_css_whitespace_is_rejected() {
        let mut reader = Reader::new("main \u{000B}");
        let first = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        assert_eq!(first.1.name, Some("main"));
        let error = Lexer::try_next(&mut reader, true).unwrap_err();
        assert_eq!(error.message(), "illegal selector token");
    }

    // Case 2: vertical tab alone
    #[test]
    fn vertical_tab_alone_is_rejected() {
        let mut reader = Reader::new("\u{000B}");
        let error = Lexer::try_next(&mut reader, false).unwrap_err();
        assert_eq!(error.message(), "illegal selector token");
    }

    // Case 3: valid CSS whitespace followed by vertical tab
    #[test]
    fn tab_vertical_tab_not_treated_as_css_whitespace() {
        let mut reader = Reader::new("main\t\u{000B}");
        let first = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        assert_eq!(first.1.name, Some("main"));
        let error = Lexer::try_next(&mut reader, true).unwrap_err();
        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn newline_vertical_tab_not_treated_as_css_whitespace() {
        let mut reader = Reader::new("main\n\u{000B}");
        let first = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        assert_eq!(first.1.name, Some("main"));
        let error = Lexer::try_next(&mut reader, true).unwrap_err();
        assert_eq!(error.message(), "illegal selector token");
    }

    #[test]
    fn form_feed_vertical_tab_not_treated_as_css_whitespace() {
        let mut reader = Reader::new("main\u{000C}\u{000B}");
        let first = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        assert_eq!(first.1.name, Some("main"));
        let error = Lexer::try_next(&mut reader, true).unwrap_err();
        assert_eq!(error.message(), "illegal selector token");
    }

    // Case 4: legitimate trailing CSS whitespace terminates successfully
    #[test]
    fn space_trailing_whitespace_terminates() {
        let mut reader = Reader::new("main ");
        let _ = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn tab_trailing_whitespace_terminates() {
        let mut reader = Reader::new("main\t");
        let _ = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn newline_trailing_whitespace_terminates() {
        let mut reader = Reader::new("main\n");
        let _ = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn carriage_return_trailing_whitespace_terminates() {
        let mut reader = Reader::new("main\r");
        let _ = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    #[test]
    fn form_feed_trailing_whitespace_terminates() {
        let mut reader = Reader::new("main\u{000C}");
        let _ = Lexer::try_next(&mut reader, false).unwrap().unwrap();
        assert!(Lexer::try_next(&mut reader, true).unwrap().is_none());
    }

    // ── Combinator::try_from tests ────────────────────────────────

    #[test]
    fn combinator_try_from_space_is_descendant() {
        let mut reader = Reader::new(" ");
        assert_eq!(
            Combinator::try_from(&mut reader),
            Some(Combinator::Descendant)
        );
    }

    #[test]
    fn combinator_try_from_tab_is_descendant() {
        let mut reader = Reader::new("\t");
        assert_eq!(
            Combinator::try_from(&mut reader),
            Some(Combinator::Descendant)
        );
    }

    #[test]
    fn combinator_try_from_newline_is_descendant() {
        let mut reader = Reader::new("\n");
        assert_eq!(
            Combinator::try_from(&mut reader),
            Some(Combinator::Descendant)
        );
    }

    #[test]
    fn combinator_try_from_cr_is_descendant() {
        let mut reader = Reader::new("\r");
        assert_eq!(
            Combinator::try_from(&mut reader),
            Some(Combinator::Descendant)
        );
    }

    #[test]
    fn combinator_try_from_form_feed_is_descendant() {
        let mut reader = Reader::new("\u{000C}");
        assert_eq!(
            Combinator::try_from(&mut reader),
            Some(Combinator::Descendant)
        );
    }

    #[test]
    fn combinator_try_from_whitespace_then_child_is_child() {
        let mut reader = Reader::new(" \t > ");
        assert_eq!(Combinator::try_from(&mut reader), Some(Combinator::Child));
    }

    #[test]
    fn combinator_try_from_plus_is_next_sibling() {
        let mut reader = Reader::new("+");
        assert_eq!(
            Combinator::try_from(&mut reader),
            Some(Combinator::NextSibling)
        );
    }

    #[test]
    fn combinator_try_from_tilde_is_subsequent_sibling() {
        let mut reader = Reader::new("~");
        assert_eq!(
            Combinator::try_from(&mut reader),
            Some(Combinator::SubsequentSibling)
        );
    }

    #[test]
    fn combinator_try_from_pipe_is_namespace() {
        let mut reader = Reader::new("|");
        assert_eq!(
            Combinator::try_from(&mut reader),
            Some(Combinator::Namespace)
        );
    }

    #[test]
    fn combinator_try_from_vertical_tab_is_none() {
        let mut reader = Reader::new("\u{000B}");
        assert_eq!(Combinator::try_from(&mut reader), None);
    }

    #[test]
    fn combinator_try_from_unknown_byte_is_none_and_unconsumed() {
        let mut reader = Reader::new("x");
        assert_eq!(Combinator::try_from(&mut reader), None);
        assert_eq!(reader.peek(), Some(b'x'));
    }
}
