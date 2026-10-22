//! Turns the exact text a model quoted into a span.
//!
//! Language models count characters badly, so the providers return the quoted
//! `original` text instead of offsets and the span is found here.

use crate::{Span, utf16};

pub struct Locator<'a> {
    text: &'a str,
    /// Byte offset where the next search starts; suggestions usually arrive in order.
    cursor: usize,
    used: Vec<(usize, usize)>,
}

impl<'a> Locator<'a> {
    pub fn new(text: &'a str) -> Locator<'a> {
        Locator {
            text,
            cursor: 0,
            used: Vec::new(),
        }
    }

    /// Finds the next occurrence of `original` that does not overlap a span
    /// already returned. Returns `None` for empty or absent quotes.
    pub fn find(&mut self, original: &str) -> Option<Span> {
        if original.is_empty() {
            return None;
        }
        let (start, end) = self
            .search(self.cursor, original)
            .or_else(|| self.search(0, original))?;
        self.cursor = end;
        self.used.push((start, end));
        Some(Span::new(
            utf16::from_byte(self.text, start),
            utf16::from_byte(self.text, end),
        ))
    }

    fn search(&self, from: usize, needle: &str) -> Option<(usize, usize)> {
        self.text[from..]
            .match_indices(needle)
            .map(|(i, m)| (from + i, from + i + m.len()))
            .find(|&(s, e)| !self.used.iter().any(|&(us, ue)| s < ue && us < e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_quotes_in_order() {
        let text = "I has a apple and a pear.";
        let mut loc = Locator::new(text);
        let has = loc.find("has").unwrap();
        let a = loc.find("a apple").unwrap();
        assert_eq!(has.slice(text), Some("has"));
        assert_eq!(a.slice(text), Some("a apple"));
    }

    #[test]
    fn repeated_quotes_get_distinct_spans() {
        let text = "its fine, its fine";
        let mut loc = Locator::new(text);
        let first = loc.find("its").unwrap();
        let second = loc.find("its").unwrap();
        assert_eq!(first, Span::new(0, 3));
        assert_eq!(second, Span::new(10, 13));
        assert_eq!(loc.find("its"), None);
    }

    #[test]
    fn falls_back_to_searching_from_the_start() {
        let text = "alpha beta";
        let mut loc = Locator::new(text);
        loc.find("beta").unwrap();
        assert_eq!(loc.find("alpha"), Some(Span::new(0, 5)));
    }

    #[test]
    fn missing_or_empty_quotes_are_rejected() {
        let mut loc = Locator::new("hello");
        assert_eq!(loc.find("world"), None);
        assert_eq!(loc.find(""), None);
    }

    #[test]
    fn spans_are_utf16() {
        let text = "Café 😀 teh";
        let span = Locator::new(text).find("teh").unwrap();
        assert_eq!(span, Span::new(8, 11));
    }
}
