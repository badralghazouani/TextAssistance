//! Prompts and the output schema. These strings must stay byte-for-byte stable
//! between requests so the providers can cache them.

use serde_json::{Value, json};
use writer_protocol::AppContext;

pub const CHECK_SYSTEM: &str = "\
You are the proofreading engine of a writing assistant. You receive one paragraph \
and return the problems in it as JSON.

Rules:
- Report spelling, grammar and punctuation mistakes. Report clarity, tone or \
concision issues only when the improvement is clear and worth the writer's attention.
- `original` must be copied exactly, character for character, from the paragraph. \
Quote the shortest stretch that contains the problem, but include enough \
surrounding words that it reads naturally on its own.
- `replacement` is the text that should take the place of `original`.
- `explanation` is one short sentence, in the same language as the paragraph.
- Write suggestions in the language of the paragraph. Do not translate.
- Text like ⟦EMAIL_1⟧ is a placeholder for private data. Never quote it, change \
it or mention it.
- Respect the register implied by the app (email, chat, document, code comment).
- Return at most 10 suggestions, most important first. If the paragraph is fine, \
return an empty list.";

pub const REWRITE_SYSTEM: &str = "\
You are the rewriting engine of a writing assistant. Rewrite the text you are \
given as instructed.

Rules:
- Reply with the rewritten text only: no preamble, quotes or notes.
- Keep the meaning, the facts and the language of the original. Do not translate.
- Keep line breaks and lists where they help.
- Text like ⟦EMAIL_1⟧ is a placeholder for private data. Keep every placeholder \
exactly as written.";

pub fn check_user_message(paragraph: &str, ctx: &AppContext) -> String {
    format!(
        "<context>app: {:?}; locale: {}</context>\n<paragraph>\n{}\n</paragraph>",
        ctx.app_kind, ctx.locale, paragraph
    )
}

pub fn rewrite_user_message(text: &str, instruction: &str, ctx: &AppContext) -> String {
    format!(
        "<context>app: {:?}; locale: {}</context>\n<instruction>{}</instruction>\n<text>\n{}\n</text>",
        ctx.app_kind, ctx.locale, instruction, text
    )
}

/// JSON schema for check results. Every object lists all its properties as
/// required and forbids extra ones, which both vendors' strict modes need.
pub fn suggestions_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "suggestions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "original": { "type": "string" },
                        "replacement": { "type": "string" },
                        "kind": {
                            "type": "string",
                            "enum": ["spelling", "grammar", "punctuation", "clarity", "tone", "concision"]
                        },
                        "explanation": { "type": "string" }
                    },
                    "required": ["original", "replacement", "kind", "explanation"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["suggestions"],
        "additionalProperties": false
    })
}
