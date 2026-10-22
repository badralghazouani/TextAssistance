//! The check and rewrite pipeline: redact, look up the cache, enforce the
//! spending cap, call the model, and anchor its answer onto the original text.
//!
//! The macOS app will reuse this once it exists; it lives here until then.

use anyhow::{Context, Result, bail};
use writer_core::{Document, Locator, hash_text, utf16};
use writer_llm::{LlmProvider, RawSuggestion};
use writer_protocol::{AppContext, RewriteMode, Suggestion};
use writer_store::{RequestKind, Store};

/// Bump when the prompts change so stale cached results are not reused.
const PROMPT_VERSION: u32 = 1;

pub struct Engine<'a> {
    pub provider: &'a dyn LlmProvider,
    pub store: &'a Store,
    pub check_model: String,
    /// 0 disables the cap.
    pub monthly_cap_usd: f64,
    pub use_cache: bool,
}

/// Suggestions for one paragraph, with spans relative to that paragraph.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ParagraphReport {
    /// Zero-based line number of the paragraph in the input.
    pub line: usize,
    pub text: String,
    pub suggestions: Vec<Suggestion>,
}

#[derive(Debug, Default)]
pub struct Totals {
    pub cost_usd: f64,
    pub api_calls: usize,
    pub cache_hits: usize,
}

impl Engine<'_> {
    pub async fn check(
        &self,
        text: &str,
        ctx: &AppContext,
    ) -> Result<(Vec<ParagraphReport>, Totals)> {
        let mut doc = Document::new();
        let dirty = doc.update(text);
        let mut reports = Vec::new();
        let mut totals = Totals::default();

        for (line, paragraph) in doc.paragraphs().iter().enumerate() {
            if !dirty.contains(&paragraph.id) {
                continue;
            }
            let redacted = writer_privacy::redact(&paragraph.text);
            let key = self.cache_key(&redacted.text, ctx);

            let cached = if self.use_cache {
                self.store
                    .cache_get(&key)?
                    .and_then(|json| serde_json::from_str::<Vec<RawSuggestion>>(&json).ok())
            } else {
                None
            };
            let raw = match cached {
                Some(raw) => {
                    totals.cache_hits += 1;
                    raw
                }
                None => {
                    self.ensure_under_cap()?;
                    let out = self
                        .provider
                        .check(&redacted.text, ctx)
                        .await
                        .with_context(|| format!("checking line {}", line + 1))?;
                    let model = if out.model.is_empty() {
                        &self.check_model
                    } else {
                        &out.model
                    };
                    totals.cost_usd += self.store.record_usage(
                        &self.provider.kind().to_string(),
                        model,
                        RequestKind::Check,
                        out.usage,
                    )?;
                    totals.api_calls += 1;
                    self.store
                        .cache_put(&key, &serde_json::to_string(&out.suggestions)?)?;
                    out.suggestions
                }
            };

            let suggestions = anchor(&paragraph.text, &redacted, raw);
            if !suggestions.is_empty() {
                reports.push(ParagraphReport {
                    line,
                    text: paragraph.text.clone(),
                    suggestions,
                });
            }
        }
        Ok((reports, totals))
    }

    pub async fn rewrite(
        &self,
        text: &str,
        mode: RewriteMode,
        ctx: &AppContext,
    ) -> Result<(String, f64)> {
        self.ensure_under_cap()?;
        let redacted = writer_privacy::redact(text);
        let out = self.provider.rewrite(&redacted.text, mode, ctx).await?;
        let cost = self.store.record_usage(
            &self.provider.kind().to_string(),
            &out.model,
            RequestKind::Rewrite,
            out.usage,
        )?;
        Ok((redacted.restore(&out.text), cost))
    }

    fn ensure_under_cap(&self) -> Result<()> {
        if self.monthly_cap_usd > 0.0 {
            let spent = self.store.month_spend()?;
            if spent >= self.monthly_cap_usd {
                bail!(
                    "monthly cap reached: ${spent:.2} spent of ${:.2}. \
                     Raise it with `writer config set monthly_cap_usd <amount>`.",
                    self.monthly_cap_usd
                );
            }
        }
        Ok(())
    }

    fn cache_key(&self, redacted: &str, ctx: &AppContext) -> String {
        let material = format!(
            "{PROMPT_VERSION}\u{1f}{}\u{1f}{}\u{1f}{:?}\u{1f}{}\u{1f}{redacted}",
            self.provider.kind(),
            self.check_model,
            ctx.app_kind,
            ctx.locale,
        );
        format!("{:016x}", hash_text(&material))
    }
}

/// Turns the model's quoted suggestions into spans on the original paragraph.
/// Quotes that are not found, touch private data, or change nothing are dropped.
pub fn anchor(
    original: &str,
    redacted: &writer_privacy::Redacted,
    raw: Vec<RawSuggestion>,
) -> Vec<Suggestion> {
    let mut locator = Locator::new(&redacted.text);
    let mut out: Vec<Suggestion> = raw
        .into_iter()
        .filter(|s| s.original != s.replacement)
        .filter_map(|s| {
            let span = locator.find(&s.original)?;
            let span = redacted.span_to_original(span)?;
            Some(Suggestion {
                start: span.start,
                end: span.end,
                kind: s.kind,
                original: span.slice(original)?.to_owned(),
                replacement: redacted.restore(&s.replacement),
                explanation: s.explanation,
            })
        })
        .collect();
    out.sort_by_key(|s| s.start);
    out
}

/// Applies non-overlapping suggestions to a paragraph (first one wins on overlap).
pub fn apply(paragraph: &str, suggestions: &[Suggestion]) -> String {
    let mut chosen: Vec<&Suggestion> = Vec::new();
    for s in suggestions {
        if chosen.iter().all(|c| s.start >= c.end || s.end <= c.start) {
            chosen.push(s);
        }
    }
    chosen.sort_by_key(|s| std::cmp::Reverse(s.start));
    let mut text = paragraph.to_owned();
    for s in chosen {
        if let (Some(start), Some(end)) =
            (utf16::to_byte(&text, s.start), utf16::to_byte(&text, s.end))
        {
            text.replace_range(start..end, &s.replacement);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use writer_llm::{CheckOutput, ProviderKind, RewriteOutput};
    use writer_protocol::{SuggestionKind, Usage};

    /// Returns a fixed answer and counts calls, so no network is needed.
    struct FakeProvider {
        suggestions: Vec<RawSuggestion>,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmProvider for FakeProvider {
        fn kind(&self) -> ProviderKind {
            ProviderKind::Claude
        }

        async fn check(&self, paragraph: &str, _: &AppContext) -> writer_llm::Result<CheckOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert!(!paragraph.contains('@'), "private data must be redacted");
            Ok(CheckOutput {
                suggestions: self.suggestions.clone(),
                usage: Usage {
                    input_tokens: 1_000,
                    output_tokens: 1_000,
                    ..Usage::default()
                },
                model: "claude-haiku-5-5".to_owned(),
            })
        }

        async fn rewrite(
            &self,
            text: &str,
            _: RewriteMode,
            _: &AppContext,
        ) -> writer_llm::Result<RewriteOutput> {
            Ok(RewriteOutput {
                text: format!("Kindly: {text}"),
                usage: Usage::default(),
                model: "claude-sonnet-5-5".to_owned(),
            })
        }
    }

    fn raw(original: &str, replacement: &str) -> RawSuggestion {
        RawSuggestion {
            original: original.to_owned(),
            replacement: replacement.to_owned(),
            kind: SuggestionKind::Grammar,
            explanation: String::new(),
        }
    }

    fn engine<'a>(provider: &'a FakeProvider, store: &'a Store, cap: f64) -> Engine<'a> {
        Engine {
            provider,
            store,
            check_model: "claude-haiku-5-5".to_owned(),
            monthly_cap_usd: cap,
            use_cache: true,
        }
    }

    #[tokio::test]
    async fn checks_redacted_text_and_maps_spans_back() {
        let provider = FakeProvider {
            suggestions: vec![raw("he dont", "he doesn't"), raw("nowhere", "x")],
            calls: AtomicUsize::new(0),
        };
        let store = Store::open_in_memory().unwrap();
        let text = "Email bob@example.com, he dont reply.";
        let (reports, totals) = engine(&provider, &store, 0.0)
            .check(text, &AppContext::default())
            .await
            .unwrap();

        assert_eq!(reports.len(), 1);
        let s = &reports[0].suggestions;
        assert_eq!(s.len(), 1, "unmatched quotes are dropped");
        assert_eq!(s[0].original, "he dont");
        assert_eq!(apply(text, s), "Email bob@example.com, he doesn't reply.");
        assert_eq!(totals.api_calls, 1);
        assert!(totals.cost_usd > 0.0);
    }

    #[tokio::test]
    async fn second_check_of_the_same_text_is_free() {
        let provider = FakeProvider {
            suggestions: vec![raw("teh", "the")],
            calls: AtomicUsize::new(0),
        };
        let store = Store::open_in_memory().unwrap();
        let e = engine(&provider, &store, 0.0);
        e.check("teh end", &AppContext::default()).await.unwrap();
        let (reports, totals) = e.check("teh end", &AppContext::default()).await.unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(totals.cache_hits, 1);
        assert_eq!(reports[0].suggestions[0].replacement, "the");
    }

    #[tokio::test]
    async fn cap_blocks_calls_once_reached() {
        let provider = FakeProvider {
            suggestions: vec![],
            calls: AtomicUsize::new(0),
        };
        let store = Store::open_in_memory().unwrap();
        store
            .record_usage(
                "claude",
                "claude-sonnet-5-5",
                RequestKind::Rewrite,
                Usage {
                    output_tokens: 1_000_000,
                    ..Usage::default()
                },
            )
            .unwrap();
        let err = engine(&provider, &store, 5.0)
            .check("Some text.", &AppContext::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("monthly cap reached"), "{err}");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn rewrite_restores_private_data() {
        let provider = FakeProvider {
            suggestions: vec![],
            calls: AtomicUsize::new(0),
        };
        let store = Store::open_in_memory().unwrap();
        let (text, _) = engine(&provider, &store, 0.0)
            .rewrite(
                "mail bob@example.com",
                RewriteMode::Formal,
                &AppContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(text, "Kindly: mail bob@example.com");
    }

    #[test]
    fn apply_skips_overlapping_suggestions() {
        let s = |start, end, replacement: &str| Suggestion {
            start,
            end,
            kind: SuggestionKind::Spelling,
            original: String::new(),
            replacement: replacement.to_owned(),
            explanation: String::new(),
        };
        let text = "teh cat sat";
        let out = apply(text, &[s(0, 3, "the"), s(1, 5, "XX"), s(8, 11, "sits")]);
        assert_eq!(out, "the cat sits");
    }
}
