//! Published per-token prices, used to show and cap what the app spends.
//!
//! USD per million tokens, as listed on the vendors' pricing pages on
//! 2026-10-22. Check them again before relying on the numbers:
//! - https://platform.claude.com/docs/en/about-claude/pricing
//! - https://developers.openai.com/api/docs/pricing

use writer_protocol::Usage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

const fn price(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Price {
    Price {
        input,
        output,
        cache_read,
        cache_write,
    }
}

/// Standard (non-batch) rates. Claude Haiku 5.5 rates apply to prompts up to
/// 100K tokens, which a paragraph never approaches.
const TABLE: &[(&str, Price)] = &[
    ("claude-haiku-5-5", price(0.10, 0.50, 0.01, 0.125)),
    ("claude-sonnet-5-5", price(2.00, 10.00, 0.20, 2.50)),
    ("claude-opus-5-5", price(4.00, 20.00, 0.20, 5.00)),
    ("gpt-6-luna", price(0.10, 0.50, 0.01, 0.125)),
    ("gpt-6.1-sol", price(2.00, 10.00, 0.10, 2.50)),
    ("gpt-6-astra", price(10.00, 50.00, 1.00, 12.50)),
];

pub fn price_for(model: &str) -> Option<Price> {
    TABLE.iter().find(|(id, _)| *id == model).map(|(_, p)| *p)
}

/// Every model with a known price, for the settings screen and `--help`.
pub fn known_models() -> impl Iterator<Item = &'static str> {
    TABLE.iter().map(|(id, _)| *id)
}

/// Cost of one request in USD, or `None` for a model missing from the table.
pub fn cost_usd(model: &str, usage: Usage) -> Option<f64> {
    let p = price_for(model)?;
    let per_token = |tokens: u64, rate: f64| tokens as f64 * rate / 1_000_000.0;
    Some(
        per_token(usage.input_tokens, p.input)
            + per_token(usage.output_tokens, p.output)
            + per_token(usage.cache_read_tokens, p.cache_read)
            + per_token(usage.cache_write_tokens, p.cache_write),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_million_tokens_costs_the_list_price() {
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Usage::default()
        };
        assert_eq!(cost_usd("claude-sonnet-5-5", usage), Some(12.0));
    }

    #[test]
    fn typical_check_costs_a_fraction_of_a_cent() {
        let usage = Usage {
            input_tokens: 150,
            output_tokens: 150,
            cache_read_tokens: 600,
            cache_write_tokens: 0,
        };
        let cost = cost_usd("claude-haiku-5-5", usage).unwrap();
        assert!((cost - 0.000096).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn unknown_models_have_no_price() {
        assert_eq!(cost_usd("some-other-model", Usage::default()), None);
    }
}
