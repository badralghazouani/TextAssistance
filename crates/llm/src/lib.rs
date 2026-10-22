//! LLM providers behind one trait, so the app can switch between Claude and
//! OpenAI today and a hosted gateway later.
//!
//! Neither vendor publishes an official Rust SDK, so both adapters call the
//! REST APIs directly with `reqwest`.

pub mod claude;
pub mod openai;
pub mod pricing;
mod prompts;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use writer_protocol::{AppContext, RewriteMode, SuggestionKind, Usage};

pub use claude::ClaudeProvider;
pub use openai::OpenAiProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "openai")]
    OpenAi,
}

impl ProviderKind {
    pub fn default_check_model(self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude-haiku-5-5",
            ProviderKind::OpenAi => "gpt-6-luna",
        }
    }

    pub fn default_rewrite_model(self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude-sonnet-5-5",
            ProviderKind::OpenAi => "gpt-6.1-sol",
        }
    }

    /// Environment variable checked for an API key before the keychain.
    pub fn api_key_env(self) -> &'static str {
        match self {
            ProviderKind::Claude => "ANTHROPIC_API_KEY",
            ProviderKind::OpenAi => "OPENAI_API_KEY",
        }
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ProviderKind::Claude => "claude",
            ProviderKind::OpenAi => "openai",
        })
    }
}

impl std::str::FromStr for ProviderKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "claude" | "anthropic" => Ok(ProviderKind::Claude),
            "openai" | "gpt" => Ok(ProviderKind::OpenAi),
            other => Err(format!("unknown provider `{other}` (use claude or openai)")),
        }
    }
}

/// A suggestion as the model returned it: quoted text, not offsets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawSuggestion {
    pub original: String,
    pub replacement: String,
    pub kind: SuggestionKind,
    pub explanation: String,
}

#[derive(Debug, Clone)]
pub struct CheckOutput {
    pub suggestions: Vec<RawSuggestion>,
    pub usage: Usage,
    /// The model that actually served the request.
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct RewriteOutput {
    pub text: String,
    pub usage: Usage,
    pub model: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{provider} API error {status}: {message}")]
    Api {
        provider: ProviderKind,
        status: u16,
        message: String,
    },
    #[error("the model declined the request ({0})")]
    Refused(String),
    #[error("the response was cut off by the output token limit")]
    Truncated,
    #[error("unexpected response: {0}")]
    BadResponse(String),
}

pub type Result<T> = std::result::Result<T, LlmError>;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;

    /// Finds problems in one paragraph.
    async fn check(&self, paragraph: &str, ctx: &AppContext) -> Result<CheckOutput>;

    /// Rewrites `text` in the requested style.
    async fn rewrite(
        &self,
        text: &str,
        mode: RewriteMode,
        ctx: &AppContext,
    ) -> Result<RewriteOutput>;
}

/// Builds the provider for `kind` with the given API key and models.
///
/// `ANTHROPIC_BASE_URL` / `OPENAI_BASE_URL` override the API host, as in the
/// vendors' own SDKs (useful for proxies and for testing against a mock).
pub fn provider(
    kind: ProviderKind,
    api_key: String,
    check_model: String,
    rewrite_model: String,
) -> Box<dyn LlmProvider> {
    let base_url = |var: &str| std::env::var(var).ok().filter(|v| !v.trim().is_empty());
    match kind {
        ProviderKind::Claude => {
            let p = ClaudeProvider::new(api_key, check_model, rewrite_model);
            Box::new(match base_url("ANTHROPIC_BASE_URL") {
                Some(url) => p.with_base_url(url.trim_end_matches('/')),
                None => p,
            })
        }
        ProviderKind::OpenAi => {
            let p = OpenAiProvider::new(api_key, check_model, rewrite_model);
            Box::new(match base_url("OPENAI_BASE_URL") {
                Some(url) => p.with_base_url(url.trim_end_matches(['/']).trim_end_matches("/v1")),
                None => p,
            })
        }
    }
}

/// Parses the `{"suggestions": [...]}` document both providers return.
fn parse_suggestions(json_text: &str) -> Result<Vec<RawSuggestion>> {
    #[derive(Deserialize)]
    struct Envelope {
        suggestions: Vec<RawSuggestion>,
    }
    serde_json::from_str::<Envelope>(json_text)
        .map(|e| e.suggestions)
        .map_err(|e| LlmError::BadResponse(format!("suggestions JSON: {e}")))
}

/// Pulls a readable message out of a vendor error body.
fn api_error(provider: ProviderKind, status: u16, body: &str) -> LlmError {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| body.chars().take(300).collect());
    LlmError::Api {
        provider,
        status,
        message,
    }
}

/// Sends `body` and returns the parsed JSON, mapping non-2xx replies to `LlmError::Api`.
async fn post_json(
    provider: ProviderKind,
    request: reqwest::RequestBuilder,
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    let response = request.json(body).send().await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(api_error(provider, status.as_u16(), &text));
    }
    serde_json::from_str(&text).map_err(|e| LlmError::BadResponse(format!("invalid JSON: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_parses_aliases() {
        assert_eq!("Claude".parse::<ProviderKind>(), Ok(ProviderKind::Claude));
        assert_eq!("openai".parse::<ProviderKind>(), Ok(ProviderKind::OpenAi));
        assert!("gemini".parse::<ProviderKind>().is_err());
    }

    #[test]
    fn api_error_prefers_the_vendor_message() {
        let err = api_error(
            ProviderKind::Claude,
            401,
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
        );
        assert_eq!(err.to_string(), "claude API error 401: invalid x-api-key");
    }

    #[test]
    fn parse_suggestions_rejects_unknown_kinds() {
        let bad = r#"{"suggestions":[{"original":"a","replacement":"b","kind":"vibes","explanation":""}]}"#;
        assert!(parse_suggestions(bad).is_err());
    }
}
