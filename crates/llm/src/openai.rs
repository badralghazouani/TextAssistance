//! OpenAI Responses API adapter (`POST /v1/responses`).

use async_trait::async_trait;
use serde_json::{Value, json};
use writer_protocol::{AppContext, RewriteMode, Usage};

use crate::prompts::{self, CHECK_SYSTEM, REWRITE_SYSTEM};
use crate::{
    CheckOutput, LlmError, LlmProvider, ProviderKind, Result, RewriteOutput, parse_suggestions,
    post_json,
};

pub struct OpenAiProvider {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    check_model: String,
    rewrite_model: String,
}

impl OpenAiProvider {
    pub fn new(api_key: String, check_model: String, rewrite_model: String) -> OpenAiProvider {
        OpenAiProvider {
            http: reqwest::Client::new(),
            api_key,
            base_url: "https://api.openai.com".to_owned(),
            check_model,
            rewrite_model,
        }
    }

    /// Points the adapter at another host, e.g. a mock server in tests.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> OpenAiProvider {
        self.base_url = base_url.into();
        self
    }

    pub fn check_body(&self, paragraph: &str, ctx: &AppContext) -> Value {
        json!({
            "model": self.check_model,
            "instructions": CHECK_SYSTEM,
            "input": prompts::check_user_message(paragraph, ctx),
            "max_output_tokens": 1024,
            "reasoning": { "effort": "none" },
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "suggestions",
                    "schema": prompts::suggestions_schema(),
                    "strict": true
                }
            },
            // Don't keep the user's text on OpenAI's side.
            "store": false
        })
    }

    pub fn rewrite_body(&self, text: &str, mode: RewriteMode, ctx: &AppContext) -> Value {
        json!({
            "model": self.rewrite_model,
            "instructions": REWRITE_SYSTEM,
            "input": prompts::rewrite_user_message(text, mode.instruction(), ctx),
            "max_output_tokens": 4096,
            "reasoning": { "effort": "low" },
            "store": false
        })
    }

    fn request(&self) -> reqwest::RequestBuilder {
        self.http
            .post(format!("{}/v1/responses", self.base_url))
            .bearer_auth(&self.api_key)
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAi
    }

    async fn check(&self, paragraph: &str, ctx: &AppContext) -> Result<CheckOutput> {
        let body = self.check_body(paragraph, ctx);
        let response = post_json(ProviderKind::OpenAi, self.request(), &body).await?;
        parse_check(&response)
    }

    async fn rewrite(
        &self,
        text: &str,
        mode: RewriteMode,
        ctx: &AppContext,
    ) -> Result<RewriteOutput> {
        let body = self.rewrite_body(text, mode, ctx);
        let response = post_json(ProviderKind::OpenAi, self.request(), &body).await?;
        parse_rewrite(&response)
    }
}

pub fn parse_check(response: &Value) -> Result<CheckOutput> {
    let text = output_text(response)?;
    Ok(CheckOutput {
        suggestions: parse_suggestions(&text)?,
        usage: usage(response),
        model: served_model(response),
    })
}

pub fn parse_rewrite(response: &Value) -> Result<RewriteOutput> {
    Ok(RewriteOutput {
        text: output_text(response)?.trim().to_owned(),
        usage: usage(response),
        model: served_model(response),
    })
}

/// Joins the `output_text` parts of every message item after checking status.
fn output_text(response: &Value) -> Result<String> {
    if response["status"] == "incomplete" {
        return match response["incomplete_details"]["reason"].as_str() {
            Some("max_output_tokens") => Err(LlmError::Truncated),
            Some("content_filter") => Err(LlmError::Refused("content_filter".to_owned())),
            other => Err(LlmError::BadResponse(format!(
                "incomplete response ({})",
                other.unwrap_or("no reason")
            ))),
        };
    }
    let items = response["output"]
        .as_array()
        .ok_or_else(|| LlmError::BadResponse("missing `output`".to_owned()))?;
    let mut text = String::new();
    for part in items
        .iter()
        .filter(|item| item["type"] == "message")
        .filter_map(|item| item["content"].as_array())
        .flatten()
    {
        match part["type"].as_str() {
            Some("output_text") => text.push_str(part["text"].as_str().unwrap_or_default()),
            Some("refusal") => {
                let reason = part["refusal"].as_str().unwrap_or("refusal");
                return Err(LlmError::Refused(reason.to_owned()));
            }
            _ => {}
        }
    }
    if text.is_empty() {
        return Err(LlmError::BadResponse("no text in response".to_owned()));
    }
    Ok(text)
}

fn usage(response: &Value) -> Usage {
    let u = &response["usage"];
    let total_input = u["input_tokens"].as_u64().unwrap_or(0);
    // OpenAI counts cached tokens inside `input_tokens`; split them out.
    let cached = u["input_tokens_details"]["cached_tokens"]
        .as_u64()
        .unwrap_or(0)
        .min(total_input);
    Usage {
        input_tokens: total_input - cached,
        output_tokens: u["output_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: cached,
        cache_write_tokens: 0,
    }
}

fn served_model(response: &Value) -> String {
    response["model"].as_str().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> OpenAiProvider {
        OpenAiProvider::new(
            "sk-test".to_owned(),
            "gpt-6-luna".to_owned(),
            "gpt-6.1-sol".to_owned(),
        )
    }

    #[test]
    fn check_body_requests_strict_json() {
        let body = provider().check_body("I has a cat.", &AppContext::default());
        assert_eq!(body["model"], "gpt-6-luna");
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["text"]["format"]["strict"], true);
        assert_eq!(body["reasoning"]["effort"], "none");
        assert_eq!(body["store"], false);
    }

    #[test]
    fn parses_a_check_response_and_splits_cached_tokens() {
        let response = json!({
            "model": "gpt-6-luna",
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": r#"{"suggestions":[{"original":"teh","replacement":"the","kind":"spelling","explanation":"Typo."}]}"#
                }]
            }],
            "usage": {
                "input_tokens": 700,
                "input_tokens_details": { "cached_tokens": 600 },
                "output_tokens": 25
            }
        });
        let out = parse_check(&response).unwrap();
        assert_eq!(out.suggestions[0].replacement, "the");
        assert_eq!(out.usage.input_tokens, 100);
        assert_eq!(out.usage.cache_read_tokens, 600);
    }

    #[test]
    fn incomplete_and_refused_responses_are_errors() {
        let cut = json!({
            "status": "incomplete",
            "incomplete_details": { "reason": "max_output_tokens" },
            "output": []
        });
        assert!(matches!(parse_rewrite(&cut), Err(LlmError::Truncated)));

        let refused = json!({
            "status": "completed",
            "output": [{
                "type": "message",
                "content": [{ "type": "refusal", "refusal": "I can't help with that." }]
            }]
        });
        assert!(matches!(parse_check(&refused), Err(LlmError::Refused(_))));
    }

    #[tokio::test]
    async fn sends_a_bearer_token() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "gpt-6.1-sol",
                "status": "completed",
                "output": [{
                    "type": "message",
                    "content": [{ "type": "output_text", "text": "Dear team," }]
                }],
                "usage": { "input_tokens": 50, "output_tokens": 4 }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let out = provider()
            .with_base_url(server.uri())
            .rewrite("hey team", RewriteMode::Formal, &AppContext::default())
            .await
            .unwrap();
        assert_eq!(out.text, "Dear team,");
    }
}
