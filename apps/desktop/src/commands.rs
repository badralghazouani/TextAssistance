//! Calls from the popup (`ui/app.js`) into Rust.

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, State};
use writer_engine::secrets::{self, MissingKey};
use writer_engine::{Engine, ParagraphReport};
use writer_llm::ProviderKind;
use writer_protocol::{AppContext, RewriteMode};
use writer_store::Store;
use writer_store::config::{Config, Paths};

use crate::{AppState, capture, platform, tray};

/// What the popup shows when it opens: the captured text, or why there is none.
#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    pub text: String,
    pub error: Option<ErrorView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorView {
    /// `no_text`, `no_key`, `accessibility` or `other`; the popup offers a fix for each.
    pub kind: &'static str,
    pub message: String,
    pub provider: Option<ProviderKind>,
}

impl ErrorView {
    pub fn new(kind: &'static str, message: impl Into<String>) -> ErrorView {
        ErrorView {
            kind,
            message: message.into(),
            provider: None,
        }
    }
}

impl From<anyhow::Error> for ErrorView {
    fn from(e: anyhow::Error) -> Self {
        match e.downcast_ref::<MissingKey>() {
            Some(missing) => ErrorView {
                kind: "no_key",
                message: format!(
                    "Add your {} API key to start checking.",
                    label(missing.provider)
                ),
                provider: Some(missing.provider),
            },
            None => ErrorView::new("other", format!("{e:#}")),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CheckView {
    pub paragraphs: Vec<ParagraphReport>,
    pub corrected: String,
    pub footer: String,
}

#[derive(Debug, Serialize)]
pub struct RewriteView {
    pub text: String,
    pub footer: String,
}

#[tauri::command]
pub fn get_session(state: State<'_, AppState>) -> Option<SessionView> {
    state
        .session
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.view.clone())
}

#[tauri::command]
pub async fn check(app: AppHandle, text: String) -> Result<CheckView, ErrorView> {
    let view = run_blocking(move || {
        let (config, store, provider) = open()?;
        let engine = Engine {
            provider: provider.as_ref(),
            store: &store,
            check_model: config.check_model(),
            monthly_cap_usd: config.monthly_cap_usd,
            use_cache: true,
        };
        let (paragraphs, totals) = block_on(engine.check(&text, &context(&config)))?;
        Ok(CheckView {
            corrected: writer_engine::apply_reports(&text, &paragraphs),
            paragraphs,
            footer: footer(&config, &store, totals.cost_usd)?,
        })
    })
    .await?;
    tray::refresh_usage(&app);
    Ok(view)
}

#[tauri::command]
pub async fn rewrite(
    app: AppHandle,
    text: String,
    mode: RewriteMode,
) -> Result<RewriteView, ErrorView> {
    let view = run_blocking(move || {
        let (config, store, provider) = open()?;
        let engine = Engine {
            provider: provider.as_ref(),
            store: &store,
            check_model: config.check_model(),
            monthly_cap_usd: config.monthly_cap_usd,
            use_cache: false,
        };
        let (text, cost) = block_on(engine.rewrite(&text, mode, &context(&config)))?;
        Ok(RewriteView {
            text,
            footer: footer(&config, &store, cost)?,
        })
    })
    .await?;
    tray::refresh_usage(&app);
    Ok(view)
}

/// Hides the popup, returns to the app the text came from and pastes `text`
/// over the original.
#[tauri::command]
pub fn apply(app: AppHandle, state: State<'_, AppState>, text: String) {
    let saved = state
        .session
        .lock()
        .unwrap()
        .take()
        .and_then(|s| s.saved_clipboard);
    crate::hide_popup(&app);
    std::thread::spawn(move || {
        // Let the previous app become active again before pasting into it.
        std::thread::sleep(Duration::from_millis(300));
        let _ = app.run_on_main_thread(move || {
            if let Err(e) = capture::paste(&text, saved.as_deref()) {
                eprintln!("paste failed: {e}");
            }
        });
    });
}

#[tauri::command]
pub fn cancel(app: AppHandle, state: State<'_, AppState>) {
    state.session.lock().unwrap().take();
    crate::hide_popup(&app);
}

#[tauri::command]
pub fn copy_text(text: String) -> Result<(), ErrorView> {
    capture::copy(&text).map_err(|e| ErrorView::new("other", e.to_string()))
}

#[tauri::command]
pub fn save_api_key(provider: ProviderKind, key: String) -> Result<(), ErrorView> {
    let key = key.trim();
    if key.is_empty() {
        return Err(ErrorView::new("no_key", "Paste the key first."));
    }
    secrets::store_key(provider, key)?;
    // Use the provider the key was just added for.
    let paths = Paths::detect().map_err(anyhow::Error::from)?;
    let mut config = Config::load(&paths.config_file).map_err(anyhow::Error::from)?;
    if config.provider != provider {
        config.provider = provider;
        config.check_model = None;
        config.rewrite_model = None;
        config
            .save(&paths.config_file)
            .map_err(anyhow::Error::from)?;
    }
    Ok(())
}

#[tauri::command]
pub fn open_accessibility_settings() {
    platform::open_accessibility_settings();
}

pub fn label(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Claude => "Claude",
        ProviderKind::OpenAi => "OpenAI",
    }
}

type Opened = (Config, Store, Box<dyn writer_llm::LlmProvider>);

fn open() -> anyhow::Result<Opened> {
    let paths = Paths::detect()?;
    let config = Config::load(&paths.config_file)?;
    let store = Store::open(&paths.database)?;
    let key = secrets::api_key(config.provider)?;
    let provider = writer_llm::provider(
        config.provider,
        key,
        config.check_model(),
        config.rewrite_model(),
    );
    Ok((config, store, provider))
}

fn context(config: &Config) -> AppContext {
    AppContext {
        locale: config.locale.clone(),
        ..AppContext::default()
    }
}

fn footer(config: &Config, store: &Store, cost: f64) -> anyhow::Result<String> {
    let spent = store.month_spend()?;
    let month = if config.monthly_cap_usd > 0.0 {
        format!("${spent:.2} of ${:.2} this month", config.monthly_cap_usd)
    } else {
        format!("${spent:.2} this month")
    };
    Ok(format!("{} · ${cost:.4} · {month}", label(config.provider)))
}

/// The engine keeps a SQLite connection, which can't move between threads
/// mid-request, so each request runs on its own thread and runtime.
async fn run_blocking<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, ErrorView> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| ErrorView::new("other", e.to_string()))?
        .map_err(ErrorView::from)
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to start a runtime")
        .block_on(future)
}
