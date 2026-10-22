//! Anthropic Messages API adapter (`POST /v1/messages`).

use async_trait::async_trait;
use serde_json::{Value, json};
use writer_protocol::{AppContext, RewriteMode, Usage};

use crate::prompts::{self, CHECK_SYSTEM, REWRITE_SYSTEM};
use crate::{
    CheckOutput, LlmError, LlmProvider, ProviderKind, Result, RewriteOutput, parse_suggestions,
    post_json,
};

const API_VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

pub struct ClaudeProvider {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    check_model: String,
    rewrite_model: String,
}

impl ClaudeProvider {
    pub fn new(api_key: String, check_model: String, rewrite_model: String) -> ClaudeProvider {
        ClaudeProvider {
            http: reqwest::Client::new(),
            api_key,
            base_url: "https://api.anthropic.com".to_owned(),
            check_model,
            rewrite_model,
        }
    }

    /// Points the adapter at another host, e.g. a mock server in tests.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> ClaudeProvider {
        self.base_url = base_url.into();
        self
    }

    pub fn check_body(&self, paragraph: &str, ctx: &AppContext) -> Value {
        let mut body = json!({
            "model": self.check_model,
            "max_tokens": 1024,
            "system": cached_system(CHECK_SYSTEM),
            "output_config": {
                "effort": "low",
                "format": { "type": "json_schema", "schema": prompts::suggestions_schema() }
            },
            "messages": [{ "role": "user", "content": prompts::check_user_message(paragraph, ctx) }]
        });
        // Haiku accepts thinking off at low effort, which keeps checks fast and
        // cheap. Newer Sonnet and Opus models reject it, so they keep the default.
        if self.check_model.starts_with("claude-haiku") {
            body["thinking"] = json!({ "type": "disabled" });
        }
        body
    }

    pub fn rewrite_body(&self, text: &str, mode: RewriteMode, ctx: &AppContext) -> Value {
        let mut body = json!({
            "model": self.rewrite_model,
            "max_tokens": 4096,
            "system": cached_system(REWRITE_SYSTEM),
            "output_config": { "effort": "low" },
            "messages": [{
                "role": "user",
                "content": prompts::rewrite_user_message(text, mode.instruction(), ctx)
            }]
        });
        if supports_fallbacks(&self.rewrite_model) {
            body["fallbacks"] = json!("default");
        }
        body
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut request = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION);
        if body.get("fallbacks").is_some() {
            request = request.header("anthropic-beta", FALLBACK_BETA);
        }
        request
    }
}

#[async_trait]
impl LlmProvider for ClaudeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }

    async fn check(&self, paragraph: &str, ctx: &AppContext) -> Result<CheckOutput> {
        let body = self.check_body(paragraph, ctx);
        let response = post_json(ProviderKind::Claude, self.request(&body), &body).await?;
        parse_check(&response)
    }

    async fn rewrite(
        &self,
        text: &str,
        mode: RewriteMode,
        ctx: &AppContext,
    ) -> Result<RewriteOutput> {
        let body = self.rewrite_body(text, mode, ctx);
        let response = post_json(ProviderKind::Claude, self.request(&body), &body).await?;
        parse_rewrite(&response)
    }
}

/// System prompt as a cacheable block, so repeat checks read it from cache.
fn cached_system(text: &str) -> Value {
    json!([{ "type": "text", "text": text, "cache_control": { "type": "ephemeral" } }])
}

/// Models that accept the server-side refusal fallback (`fallbacks: "default"`).
fn supports_fallbacks(model: &str) -> bool {
    matches!(
        model,
        "claude-sonnet-5-5" | "claude-opus-5-5" | "claude-opus-5" | "claude-fable-5-1"
    )
}

pub fn parse_check(response: &Value) -> Result<CheckOutput> {
    let text = final_text(response)?;
    Ok(CheckOutput {
        suggestions: parse_suggestions(&text)?,
        usage: usage(response),
        model: served_model(response),
    })
}

pub fn parse_rewrite(response: &Value) -> Result<RewriteOutput> {
    Ok(RewriteOutput {
        text: final_text(response)?.trim().to_owned(),
        usage: usage(response),
        model: served_model(response),
    })
}

/// Joins the text blocks after checking why generation stopped.
fn final_text(response: &Value) -> Result<String> {
    match response["stop_reason"].as_str() {
        Some("refusal") => {
            let category = response["stop_details"]["category"]
                .as_str()
                .unwrap_or("no category");
            return Err(LlmError::Refused(category.to_owned()));
        }
        Some("max_tokens") => return Err(LlmError::Truncated),
        _ => {}
    }
    let blocks = response["content"]
        .as_array()
        .ok_or_else(|| LlmError::BadResponse("missing `content`".to_owned()))?;
    let text: String = blocks
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    if text.is_empty() {
        return Err(LlmError::BadResponse("no text in response".to_owned()));
    }
    Ok(text)
}

fn usage(response: &Value) -> Usage {
    let u = &response["usage"];
    let n = |key: &str| u[key].as_u64().unwrap_or(0);
    Usage {
        input_tokens: n("input_tokens"),
        output_tokens: n("output_tokens"),
        cache_read_tokens: n("cache_read_input_tokens"),
        cache_write_tokens: n("cache_creation_input_tokens"),
    }
}

fn served_model(response: &Value) -> String {
    response["model"].as_str().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use writer_protocol::SuggestionKind;

    fn provider() -> ClaudeProvider {
        ClaudeProvider::new(
            "test-key".to_owned(),
            "claude-haiku-5-5".to_owned(),
            "claude-sonnet-5-5".to_owned(),
        )
    }

    #[test]
    fn check_body_uses_structured_output_and_no_sampling_params() {
        let body = provider().check_body("I has a cat.", &AppContext::default());
        assert_eq!(body["model"], "claude-haiku-5-5");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["thinking"]["type"], "disabled");
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert!(body.get("temperature").is_none());
        assert!(body.get("fallbacks").is_none());
    }

    #[test]
    fn non_haiku_check_models_keep_default_thinking() {
        let p = ClaudeProvider::new(
            "k".to_owned(),
            "claude-sonnet-5-5".to_owned(),
            "claude-sonnet-5-5".to_owned(),
        );
        let body = p.check_body("x", &AppContext::default());
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn rewrite_body_opts_into_fallbacks_on_sonnet() {
        let body = provider().rewrite_body("hi", RewriteMode::Formal, &AppContext::default());
        assert_eq!(body["model"], "claude-sonnet-5-5");
        assert_eq!(body["fallbacks"], "default");
    }

    #[test]
    fn parses_a_check_response() {
        let response = json!({
            "model": "claude-haiku-5-5",
            "stop_reason": "end_turn",
            "content": [{
                "type": "text",
                "text": r#"{"suggestions":[{"original":"I has","replacement":"I have","kind":"grammar","explanation":"Use “have” with “I”."}]}"#
            }],
            "usage": {
                "input_tokens": 40,
                "output_tokens": 30,
                "cache_read_input_tokens": 600,
                "cache_creation_input_tokens": 0
            }
        });
        let out = parse_check(&response).unwrap();
        assert_eq!(out.suggestions.len(), 1);
        assert_eq!(out.suggestions[0].kind, SuggestionKind::Grammar);
        assert_eq!(out.usage.cache_read_tokens, 600);
        assert_eq!(out.model, "claude-haiku-5-5");
    }

    #[test]
    fn refusals_and_truncation_are_errors() {
        let refused = json!({
            "stop_reason": "refusal",
            "stop_details": { "type": "refusal", "category": "cyber" },
            "content": []
        });
        assert!(matches!(parse_check(&refused), Err(LlmError::Refused(c)) if c == "cyber"));

        let cut = json!({ "stop_reason": "max_tokens", "content": [] });
        assert!(matches!(parse_rewrite(&cut), Err(LlmError::Truncated)));
    }

    #[test]
    fn rewrite_skips_non_text_blocks() {
        let response = json!({
            "model": "claude-sonnet-5-5",
            "stop_reason": "end_turn",
            "content": [
                { "type": "thinking", "thinking": "" },
                { "type": "text", "text": "  Hello there.  " }
            ],
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        });
        assert_eq!(parse_rewrite(&response).unwrap().text, "Hello there.");
    }

    #[tokio::test]
    async fn sends_the_expected_headers() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "test-key"))
            .and(header("anthropic-version", API_VERSION))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "claude-haiku-5-5",
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": r#"{"suggestions":[]}"# }],
                "usage": { "input_tokens": 1, "output_tokens": 1 }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let out = provider()
            .with_base_url(server.uri())
            .check("Fine text.", &AppContext::default())
            .await
            .unwrap();
        assert!(out.suggestions.is_empty());
    }

    #[tokio::test]
    async fn http_errors_carry_the_api_message() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "type": "error",
                "error": { "type": "rate_limit_error", "message": "slow down" }
            })))
            .mount(&server)
            .await;

        let err = provider()
            .with_base_url(server.uri())
            .check("x", &AppContext::default())
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Api { status: 429, .. }), "{err}");
    }
}
