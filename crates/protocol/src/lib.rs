//! Wire types shared by every client and, later, the SaaS gateway.
//!
//! All text offsets are UTF-16 code units, because that is what macOS,
//! Windows, iOS, Android and JavaScript use for their text APIs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionKind {
    Spelling,
    Grammar,
    Punctuation,
    Clarity,
    Tone,
    Concision,
}

/// A suggestion anchored to a span of a paragraph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    /// Start of the span, in UTF-16 code units from the start of the paragraph.
    pub start: u32,
    /// End of the span (exclusive), in UTF-16 code units.
    pub end: u32,
    pub kind: SuggestionKind,
    pub original: String,
    pub replacement: String,
    pub explanation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppKind {
    Email,
    Chat,
    #[default]
    Document,
    Code,
}

/// Where the text is being written, so the model can match the register.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppContext {
    pub app_kind: AppKind,
    /// BCP 47 language tag such as `en-US` or `fr-FR`.
    pub locale: String,
}

impl Default for AppContext {
    fn default() -> Self {
        Self {
            app_kind: AppKind::Document,
            locale: "en-US".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewriteMode {
    Clearer,
    Shorter,
    Formal,
    Friendly,
}

impl RewriteMode {
    pub fn instruction(self) -> &'static str {
        match self {
            RewriteMode::Clearer => "Rewrite the text so it is clearer and easier to read.",
            RewriteMode::Shorter => "Rewrite the text to be more concise without losing meaning.",
            RewriteMode::Formal => "Rewrite the text in a professional, formal tone.",
            RewriteMode::Friendly => "Rewrite the text in a warm, friendly tone.",
        }
    }
}

/// Token usage reported by a provider for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Uncached input tokens.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_kind_uses_snake_case_on_the_wire() {
        let json = serde_json::to_string(&SuggestionKind::Concision).unwrap();
        assert_eq!(json, "\"concision\"");
    }

    #[test]
    fn usage_add_sums_every_field() {
        let mut total = Usage::default();
        total.add(Usage {
            input_tokens: 1,
            output_tokens: 2,
            cache_read_tokens: 3,
            cache_write_tokens: 4,
        });
        total.add(Usage {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 30,
            cache_write_tokens: 40,
        });
        assert_eq!(
            total,
            Usage {
                input_tokens: 11,
                output_tokens: 22,
                cache_read_tokens: 33,
                cache_write_tokens: 44,
            }
        );
    }
}
