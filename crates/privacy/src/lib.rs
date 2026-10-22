//! Replaces personal data with placeholders before text leaves the device,
//! and maps the model's answer back onto the original text.
//!
//! `Call me at +33 6 12 34 56 78` is sent as `Call me at ⟦PHONE_1⟧`. The
//! mapping never leaves the device.

use std::sync::LazyLock;

use regex::Regex;
use writer_core::{Span, utf16};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiiKind {
    Email,
    Card,
    Iban,
    Phone,
}

impl PiiKind {
    fn label(self) -> &'static str {
        match self {
            PiiKind::Email => "EMAIL",
            PiiKind::Card => "CARD",
            PiiKind::Iban => "IBAN",
            PiiKind::Phone => "PHONE",
        }
    }
}

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}").unwrap()
});
static CARD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:\d[ -]?){12,18}\d\b").unwrap());
static IBAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]){11,30}\b").unwrap());
static PHONE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\+\d{1,3}[ .-]?)?(?:\(\d{1,4}\)[ .-]?)?\d{1,4}(?:[ .-]?\d{1,4}){2,6}").unwrap()
});

#[derive(Debug, Clone)]
struct Replacement {
    placeholder: String,
    original: String,
    /// UTF-16 span of the placeholder in the redacted text.
    redacted: Span,
}

/// Text with personal data swapped for placeholders, plus the way back.
#[derive(Debug, Clone)]
pub struct Redacted {
    pub text: String,
    replacements: Vec<Replacement>,
}

impl Redacted {
    /// `true` when nothing was redacted.
    pub fn is_clean(&self) -> bool {
        self.replacements.is_empty()
    }

    /// Number of values replaced by placeholders.
    pub fn count(&self) -> usize {
        self.replacements.len()
    }

    /// Maps a span of the redacted text onto the original text.
    /// Returns `None` if the span touches a placeholder.
    pub fn span_to_original(&self, span: Span) -> Option<Span> {
        let mut delta: i64 = 0;
        for r in &self.replacements {
            if r.redacted.end <= span.start {
                delta += utf16::len(&r.original) as i64 - utf16::len(&r.placeholder) as i64;
            } else if r.redacted.start >= span.end {
                break;
            } else {
                return None;
            }
        }
        let shift = |p: u32| u32::try_from(p as i64 + delta).ok();
        Some(Span::new(shift(span.start)?, shift(span.end)?))
    }

    /// Puts the original values back into model output (e.g. a rewrite).
    pub fn restore(&self, text: &str) -> String {
        self.replacements.iter().fold(text.to_owned(), |acc, r| {
            acc.replace(&r.placeholder, &r.original)
        })
    }
}

/// Detects emails, payment cards, IBANs and phone numbers and replaces them.
pub fn redact(text: &str) -> Redacted {
    type Detector = (PiiKind, &'static LazyLock<Regex>, fn(&str) -> bool);
    // Earlier detectors win where matches overlap (a card number is also digits).
    let detectors: [Detector; 4] = [
        (PiiKind::Email, &EMAIL, |_| true),
        (PiiKind::Card, &CARD, luhn_valid),
        (PiiKind::Iban, &IBAN, iban_valid),
        (PiiKind::Phone, &PHONE, phone_plausible),
    ];
    let mut found: Vec<(usize, usize, PiiKind)> = Vec::new();
    for (kind, re, valid) in detectors {
        for m in re.find_iter(text) {
            let overlaps = found.iter().any(|&(s, e, _)| m.start() < e && s < m.end());
            if !overlaps && valid(m.as_str()) {
                found.push((m.start(), m.end(), kind));
            }
        }
    }
    found.sort_by_key(|&(start, _, _)| start);

    let mut out = String::with_capacity(text.len());
    let mut replacements = Vec::with_capacity(found.len());
    let mut counters = [0u32; 4];
    let mut last = 0;
    for (start, end, kind) in found {
        out.push_str(&text[last..start]);
        let n = &mut counters[kind as usize];
        *n += 1;
        let placeholder = format!("⟦{}_{}⟧", kind.label(), n);
        let at = utf16::len(&out);
        replacements.push(Replacement {
            redacted: Span::new(at, at + utf16::len(&placeholder)),
            placeholder: placeholder.clone(),
            original: text[start..end].to_owned(),
        });
        out.push_str(&placeholder);
        last = end;
    }
    out.push_str(&text[last..]);
    Redacted {
        text: out,
        replacements,
    }
}

fn digits(s: &str) -> Vec<u32> {
    s.chars().filter_map(|c| c.to_digit(10)).collect()
}

fn luhn_valid(s: &str) -> bool {
    let d = digits(s);
    if !(13..=19).contains(&d.len()) {
        return false;
    }
    let sum: u32 = d
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &x)| {
            if i % 2 == 0 {
                x
            } else if x * 2 > 9 {
                x * 2 - 9
            } else {
                x * 2
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

fn iban_valid(s: &str) -> bool {
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !(15..=34).contains(&compact.len()) {
        return false;
    }
    let (head, tail) = compact.split_at(4);
    let mut remainder: u64 = 0;
    for c in tail.chars().chain(head.chars()) {
        let Some(value) = c.to_digit(36) else {
            return false;
        };
        let width = if value >= 10 { 100 } else { 10 };
        remainder = (remainder * width + value as u64) % 97;
    }
    remainder == 1
}

fn phone_plausible(s: &str) -> bool {
    (8..=15).contains(&digits(s).len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use writer_core::Locator;

    #[test]
    fn clean_text_is_untouched() {
        let r = redact("The meeting is on Monday at 10.");
        assert!(r.is_clean());
        assert_eq!(r.text, "The meeting is on Monday at 10.");
    }

    #[test]
    fn redacts_each_kind() {
        let r = redact(
            "Mail jane.doe@example.co.uk, call +33 6 12 34 56 78, \
             card 4111 1111 1111 1111, IBAN FR76 3000 6000 0112 3456 7890 189.",
        );
        assert_eq!(
            r.text,
            "Mail ⟦EMAIL_1⟧, call ⟦PHONE_1⟧, card ⟦CARD_1⟧, IBAN ⟦IBAN_1⟧."
        );
        assert_eq!(r.count(), 4);
    }

    #[test]
    fn invalid_checksums_are_not_cards_or_ibans() {
        assert!(luhn_valid("4111 1111 1111 1111"));
        assert!(!luhn_valid("4111 1111 1111 1112"));
        assert!(iban_valid("FR76 3000 6000 0112 3456 7890 189"));
        assert!(!iban_valid("FR76 3000 6000 0112 3456 7890 188"));
    }

    #[test]
    fn short_numbers_and_years_stay() {
        let r = redact("In 2024 we sold 300 units for 12.50 each.");
        assert!(r.is_clean());
    }

    #[test]
    fn spans_map_back_around_placeholders() {
        let original = "Write to bob@example.com, he dont reply.";
        let r = redact(original);
        let span = Locator::new(&r.text).find("dont").unwrap();
        let mapped = r.span_to_original(span).unwrap();
        assert_eq!(mapped.slice(original), Some("dont"));
    }

    #[test]
    fn spans_touching_a_placeholder_are_dropped() {
        let r = redact("Ping bob@example.com now.");
        let span = Locator::new(&r.text).find("⟦EMAIL_1⟧ now").unwrap();
        assert_eq!(r.span_to_original(span), None);
    }

    #[test]
    fn restore_puts_values_back() {
        let r = redact("Send it to bob@example.com.");
        assert_eq!(
            r.restore("Please send it to ⟦EMAIL_1⟧."),
            "Please send it to bob@example.com."
        );
    }
}
