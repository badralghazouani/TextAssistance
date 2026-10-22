//! Local persistence: settings file, usage log for the spending cap, and a
//! cache of check results so unchanged paragraphs are never paid for twice.

pub mod config;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use writer_protocol::Usage;

pub use config::Config;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not read or write {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config file {path}: {message}")]
    Config { path: PathBuf, message: String },
    #[error("no home directory found to store settings in")]
    NoHome,
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Check,
    Rewrite,
}

impl RequestKind {
    fn as_str(self) -> &'static str {
        match self {
            RequestKind::Check => "check",
            RequestKind::Rewrite => "rewrite",
        }
    }
}

/// One row of the monthly usage report.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSpend {
    pub model: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| StoreError::Io {
                path: dir.to_owned(),
                source,
            })?;
        }
        Store::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Store> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Store> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS usage (
                 id          INTEGER PRIMARY KEY,
                 ts          INTEGER NOT NULL,
                 provider    TEXT NOT NULL,
                 model       TEXT NOT NULL,
                 kind        TEXT NOT NULL,
                 input       INTEGER NOT NULL,
                 output      INTEGER NOT NULL,
                 cache_read  INTEGER NOT NULL,
                 cache_write INTEGER NOT NULL,
                 cost_usd    REAL NOT NULL
             );
             CREATE INDEX IF NOT EXISTS usage_ts ON usage (ts);
             CREATE TABLE IF NOT EXISTS check_cache (
                 key     TEXT PRIMARY KEY,
                 value   TEXT NOT NULL,
                 created INTEGER NOT NULL
             );",
        )?;
        Ok(Store { conn })
    }

    /// Logs one API request. `cost_usd` is 0 for models without a known price.
    pub fn record_usage(
        &self,
        provider: &str,
        model: &str,
        kind: RequestKind,
        usage: Usage,
    ) -> Result<f64> {
        self.record_usage_at(now(), provider, model, kind, usage)
    }

    fn record_usage_at(
        &self,
        ts: i64,
        provider: &str,
        model: &str,
        kind: RequestKind,
        usage: Usage,
    ) -> Result<f64> {
        let cost = writer_llm::pricing::cost_usd(model, usage).unwrap_or(0.0);
        self.conn.execute(
            "INSERT INTO usage (ts, provider, model, kind, input, output, cache_read, cache_write, cost_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                ts,
                provider,
                model,
                kind.as_str(),
                usage.input_tokens as i64,
                usage.output_tokens as i64,
                usage.cache_read_tokens as i64,
                usage.cache_write_tokens as i64,
                cost
            ],
        )?;
        Ok(cost)
    }

    /// Total spent in the current calendar month (local time), in USD.
    pub fn month_spend(&self) -> Result<f64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(cost_usd), 0) FROM usage
             WHERE strftime('%Y-%m', ts, 'unixepoch', 'localtime')
                 = strftime('%Y-%m', 'now', 'localtime')",
            [],
            |row| row.get(0),
        )?)
    }

    /// This month's spend broken down by model, most expensive first.
    pub fn month_report(&self) -> Result<Vec<ModelSpend>> {
        let mut stmt = self.conn.prepare(
            "SELECT model, COUNT(*), SUM(input + cache_read + cache_write), SUM(output), SUM(cost_usd)
             FROM usage
             WHERE strftime('%Y-%m', ts, 'unixepoch', 'localtime')
                 = strftime('%Y-%m', 'now', 'localtime')
             GROUP BY model
             ORDER BY SUM(cost_usd) DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ModelSpend {
                model: row.get(0)?,
                requests: row.get::<_, i64>(1)? as u64,
                input_tokens: row.get::<_, i64>(2)? as u64,
                output_tokens: row.get::<_, i64>(3)? as u64,
                cost_usd: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn cache_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM check_cache WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn cache_put(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO check_cache (key, value, created) VALUES (?1, ?2, ?3)",
            params![key, value, now()],
        )?;
        Ok(())
    }

    /// Drops cached results older than `max_age_days`.
    pub fn cache_prune(&self, max_age_days: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM check_cache WHERE created < ?1",
            [now() - max_age_days * 86_400],
        )?)
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            ..Usage::default()
        }
    }

    #[test]
    fn records_cost_and_sums_this_month() {
        let store = Store::open_in_memory().unwrap();
        let cost = store
            .record_usage(
                "claude",
                "claude-sonnet-5-5",
                RequestKind::Rewrite,
                usage(1_000_000, 0),
            )
            .unwrap();
        assert_eq!(cost, 2.0);
        store
            .record_usage(
                "openai",
                "gpt-6-luna",
                RequestKind::Check,
                usage(0, 1_000_000),
            )
            .unwrap();
        assert!((store.month_spend().unwrap() - 2.5).abs() < 1e-9);
    }

    #[test]
    fn older_months_do_not_count() {
        let store = Store::open_in_memory().unwrap();
        let last_year = now() - 400 * 86_400;
        store
            .record_usage_at(
                last_year,
                "claude",
                "claude-opus-5-5",
                RequestKind::Rewrite,
                usage(1_000_000, 0),
            )
            .unwrap();
        assert_eq!(store.month_spend().unwrap(), 0.0);
    }

    #[test]
    fn report_groups_by_model() {
        let store = Store::open_in_memory().unwrap();
        for _ in 0..3 {
            store
                .record_usage(
                    "claude",
                    "claude-haiku-5-5",
                    RequestKind::Check,
                    usage(100, 50),
                )
                .unwrap();
        }
        store
            .record_usage(
                "claude",
                "claude-sonnet-5-5",
                RequestKind::Rewrite,
                usage(800, 600),
            )
            .unwrap();
        let report = store.month_report().unwrap();
        assert_eq!(report.len(), 2);
        assert_eq!(report[0].model, "claude-sonnet-5-5");
        assert_eq!(report[1].requests, 3);
        assert_eq!(report[1].input_tokens, 300);
    }

    #[test]
    fn unknown_models_are_logged_at_zero_cost() {
        let store = Store::open_in_memory().unwrap();
        let cost = store
            .record_usage("openai", "mystery-model", RequestKind::Check, usage(10, 10))
            .unwrap();
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn cache_round_trip() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.cache_get("k").unwrap(), None);
        store.cache_put("k", "v1").unwrap();
        store.cache_put("k", "v2").unwrap();
        assert_eq!(store.cache_get("k").unwrap().as_deref(), Some("v2"));
        assert_eq!(store.cache_prune(30).unwrap(), 0);
    }
}
