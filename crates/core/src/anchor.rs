//! Keeps suggestion spans attached to the right text while the user keeps typing.

use crate::utf16;

/// A single contiguous edit, in UTF-16 code units of the old text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edit {
    pub start: u32,
    pub deleted: u32,
    pub inserted: u32,
}

impl Edit {
    /// Computes the smallest single edit that turns `old` into `new`
    /// (common prefix and suffix are kept). Returns `None` if they are equal.
    pub fn between(old: &str, new: &str) -> Option<Edit> {
        if old == new {
            return None;
        }
        let old_chars: Vec<char> = old.chars().collect();
        let new_chars: Vec<char> = new.chars().collect();

        let prefix = old_chars
            .iter()
            .zip(&new_chars)
            .take_while(|(a, b)| a == b)
            .count();
        let max_suffix = old_chars.len().min(new_chars.len()) - prefix;
        let suffix = old_chars
            .iter()
            .rev()
            .zip(new_chars.iter().rev())
            .take(max_suffix)
            .take_while(|(a, b)| a == b)
            .count();

        let units = |chars: &[char]| chars.iter().map(|c| c.len_utf16() as u32).sum::<u32>();
        Some(Edit {
            start: units(&old_chars[..prefix]),
            deleted: units(&old_chars[prefix..old_chars.len() - suffix]),
            inserted: units(&new_chars[prefix..new_chars.len() - suffix]),
        })
    }
}

/// A half-open span `[start, end)` in UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Span {
        debug_assert!(start <= end);
        Span { start, end }
    }

    /// Moves the span through `edit`. Returns `None` when the edit touches the
    /// span itself, because the suggestion no longer applies to what is there.
    pub fn rebase(self, edit: Edit) -> Option<Span> {
        let edit_end = edit.start + edit.deleted;
        if edit_end <= self.start {
            // Entirely before the span (including an insertion right at its start).
            let shift = |p: u32| p - edit.deleted + edit.inserted;
            Some(Span::new(shift(self.start), shift(self.end)))
        } else if edit.start >= self.end {
            Some(self)
        } else {
            None
        }
    }

    /// Returns the text covered by this span, or `None` if it is out of range.
    pub fn slice(self, text: &str) -> Option<&str> {
        let start = utf16::to_byte(text, self.start)?;
        let end = utf16::to_byte(text, self.end)?;
        text.get(start..end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn between_finds_minimal_edit() {
        assert_eq!(Edit::between("abc", "abc"), None);
        assert_eq!(
            Edit::between("hello world", "hello brave world"),
            Some(Edit {
                start: 6,
                deleted: 0,
                inserted: 6
            })
        );
        assert_eq!(
            Edit::between("teh cat", "the cat"),
            Some(Edit {
                start: 1,
                deleted: 2,
                inserted: 2
            })
        );
        // Repeated characters must not make prefix and suffix overlap.
        assert_eq!(
            Edit::between("aaa", "aa"),
            Some(Edit {
                start: 2,
                deleted: 1,
                inserted: 0
            })
        );
    }

    #[test]
    fn between_counts_utf16_units() {
        assert_eq!(
            Edit::between("a😀b", "a😀😀b"),
            Some(Edit {
                start: 3,
                deleted: 0,
                inserted: 2
            })
        );
    }

    #[test]
    fn rebase_shifts_spans_after_an_edit() {
        let span = Span::new(10, 14);
        let insert_before = Edit {
            start: 2,
            deleted: 0,
            inserted: 3,
        };
        assert_eq!(span.rebase(insert_before), Some(Span::new(13, 17)));

        let delete_before = Edit {
            start: 0,
            deleted: 4,
            inserted: 0,
        };
        assert_eq!(span.rebase(delete_before), Some(Span::new(6, 10)));

        let insert_at_start = Edit {
            start: 10,
            deleted: 0,
            inserted: 1,
        };
        assert_eq!(span.rebase(insert_at_start), Some(Span::new(11, 15)));
    }

    #[test]
    fn rebase_keeps_spans_before_an_edit() {
        let span = Span::new(2, 5);
        let after = Edit {
            start: 5,
            deleted: 3,
            inserted: 1,
        };
        assert_eq!(span.rebase(after), Some(span));
    }

    #[test]
    fn rebase_drops_spans_the_edit_touches() {
        let span = Span::new(2, 5);
        assert_eq!(
            span.rebase(Edit {
                start: 3,
                deleted: 0,
                inserted: 1
            }),
            None
        );
        assert_eq!(
            span.rebase(Edit {
                start: 0,
                deleted: 3,
                inserted: 0
            }),
            None
        );
        assert_eq!(
            span.rebase(Edit {
                start: 4,
                deleted: 4,
                inserted: 4
            }),
            None
        );
    }

    #[test]
    fn rebased_span_still_covers_the_same_text() {
        let old = "I has a cat.";
        let span = Span::new(2, 5); // "has"
        let new = "Today I has a cat.";
        let edit = Edit::between(old, new).unwrap();
        let moved = span.rebase(edit).unwrap();
        assert_eq!(moved.slice(new), Some("has"));
    }
}
