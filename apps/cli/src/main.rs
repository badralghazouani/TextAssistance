//! `writer`: command-line front end for the writing assistant. It exercises
//! the same pipeline the macOS app will use, so it doubles as a test harness.

mod engine;
mod secrets;

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use engine::{Engine, ParagraphReport};
use writer_llm::ProviderKind;
use writer_protocol::{AppContext, AppKind, RewriteMode};
use writer_store::Store;
use writer_store::config::{Config, Paths};

#[derive(Parser)]
#[command(
    name = "writer",
    version,
    about = "AI writing assistant (Claude or OpenAI)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check a file (or stdin) for spelling, grammar and style problems.
    Check {
        /// File to check; reads stdin when omitted.
        file: Option<PathBuf>,
        #[command(flatten)]
        opts: RunOpts,
        /// Print the corrected text instead of the list of suggestions.
        #[arg(long)]
        apply: bool,
        /// Print suggestions as JSON.
        #[arg(long, conflicts_with = "apply")]
        json: bool,
        /// Ignore cached results and ask the model again.
        #[arg(long)]
        no_cache: bool,
    },
    /// Rewrite a file (or stdin) in another style.
    Rewrite {
        file: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = Mode::Clearer)]
        mode: Mode,
        #[command(flatten)]
        opts: RunOpts,
    },
    /// Show this month's API spend.
    Usage,
    /// Show or change settings.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Store or remove an API key in the macOS Keychain.
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },
}

#[derive(clap::Args)]
struct RunOpts {
    /// Provider to use instead of the configured one.
    #[arg(long)]
    provider: Option<ProviderKind>,
    /// Kind of app the text is for, which sets the register.
    #[arg(long, value_enum, default_value_t = App::Document)]
    app: App,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the current settings.
    Show,
    /// Change one setting: provider, check_model, rewrite_model, monthly_cap_usd, locale.
    Set { key: String, value: String },
    /// Print where the config file and database are stored.
    Path,
}

#[derive(Subcommand)]
enum KeyAction {
    /// Save a key (prompted, or read from stdin).
    Set { provider: ProviderKind },
    /// Remove a saved key.
    Delete { provider: ProviderKind },
}

#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    Clearer,
    Shorter,
    Formal,
    Friendly,
}

impl From<Mode> for RewriteMode {
    fn from(m: Mode) -> Self {
        match m {
            Mode::Clearer => RewriteMode::Clearer,
            Mode::Shorter => RewriteMode::Shorter,
            Mode::Formal => RewriteMode::Formal,
            Mode::Friendly => RewriteMode::Friendly,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum App {
    Email,
    Chat,
    Document,
    Code,
}

impl From<App> for AppKind {
    fn from(a: App) -> Self {
        match a {
            App::Email => AppKind::Email,
            App::Chat => AppKind::Chat,
            App::Document => AppKind::Document,
            App::Code => AppKind::Code,
        }
    }
}

#[tokio::main]
async fn main() {
    if let Err(err) = run(Cli::parse()).await {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    let paths = Paths::detect()?;
    let mut config = Config::load(&paths.config_file)?;

    match cli.command {
        Command::Check {
            file,
            opts,
            apply,
            json,
            no_cache,
        } => {
            let text = read_input(file.as_ref())?;
            let store = Store::open(&paths.database)?;
            let ctx = context(&config, &opts);
            if let Some(p) = opts.provider {
                select_provider(&mut config, p);
            }
            let provider = build_provider(&config)?;
            let engine = Engine {
                provider: provider.as_ref(),
                store: &store,
                check_model: config.check_model(),
                monthly_cap_usd: config.monthly_cap_usd,
                use_cache: !no_cache,
            };
            let (reports, totals) = engine.check(&text, &ctx).await?;

            if apply {
                print!("{}", apply_all(&text, &reports));
            } else if json {
                println!("{}", serde_json::to_string_pretty(&reports)?);
            } else {
                print_reports(&reports);
            }
            let count: usize = reports.iter().map(|r| r.suggestions.len()).sum();
            eprintln!(
                "{count} suggestion(s) · {} call(s), {} from cache · cost ${:.5} · {}",
                totals.api_calls,
                totals.cache_hits,
                totals.cost_usd,
                month_line(&store, &config)?
            );
        }
        Command::Rewrite { file, mode, opts } => {
            let text = read_input(file.as_ref())?;
            let store = Store::open(&paths.database)?;
            let ctx = context(&config, &opts);
            if let Some(p) = opts.provider {
                select_provider(&mut config, p);
            }
            let provider = build_provider(&config)?;
            let engine = Engine {
                provider: provider.as_ref(),
                store: &store,
                check_model: config.check_model(),
                monthly_cap_usd: config.monthly_cap_usd,
                use_cache: false,
            };
            let (rewritten, cost) = engine.rewrite(&text, mode.into(), &ctx).await?;
            println!("{rewritten}");
            eprintln!("cost ${cost:.5} · {}", month_line(&store, &config)?);
        }
        Command::Usage => {
            let store = Store::open(&paths.database)?;
            let report = store.month_report()?;
            if report.is_empty() {
                println!("No API calls this month.");
            }
            for row in &report {
                println!(
                    "{:<20} {:>6} calls {:>10} in {:>10} out  ${:.4}",
                    row.model, row.requests, row.input_tokens, row.output_tokens, row.cost_usd
                );
            }
            println!("{}", month_line(&store, &config)?);
        }
        Command::Config { action } => match action {
            ConfigAction::Show => print!("{}", toml_string(&config)),
            ConfigAction::Path => {
                println!("config:   {}", paths.config_file.display());
                println!("database: {}", paths.database.display());
            }
            ConfigAction::Set { key, value } => {
                set_config(&mut config, &key, &value)?;
                config.save(&paths.config_file)?;
                print!("{}", toml_string(&config));
            }
        },
        Command::Key { action } => match action {
            KeyAction::Set { provider } => {
                let key = if std::io::stdin().is_terminal() {
                    rpassword::prompt_password(format!("{provider} API key: "))?
                } else {
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    buf
                };
                let key = key.trim();
                if key.is_empty() {
                    bail!("no key given");
                }
                secrets::store_key(provider, key)?;
                println!("Saved the {provider} key to the keychain.");
            }
            KeyAction::Delete { provider } => {
                secrets::delete_key(provider)?;
                println!("Removed the {provider} key.");
            }
        },
    }
    Ok(())
}

fn read_input(file: Option<&PathBuf>) -> Result<String> {
    let text = match file {
        Some(path) => {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        None => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };
    if text.trim().is_empty() {
        bail!("nothing to check: the input is empty");
    }
    Ok(text)
}

fn context(config: &Config, opts: &RunOpts) -> AppContext {
    AppContext {
        app_kind: opts.app.into(),
        locale: config.locale.clone(),
    }
}

/// Switching provider for one run drops model overrides meant for the other one.
fn select_provider(config: &mut Config, provider: ProviderKind) {
    if config.provider != provider {
        config.provider = provider;
        config.check_model = None;
        config.rewrite_model = None;
    }
}

fn build_provider(config: &Config) -> Result<Box<dyn writer_llm::LlmProvider>> {
    let key = secrets::api_key(config.provider)?;
    Ok(writer_llm::provider(
        config.provider,
        key,
        config.check_model(),
        config.rewrite_model(),
    ))
}

fn set_config(config: &mut Config, key: &str, value: &str) -> Result<()> {
    let optional = |v: &str| (!v.is_empty() && v != "default").then(|| v.to_owned());
    match key {
        "provider" => {
            let provider = value.parse().map_err(anyhow::Error::msg)?;
            select_provider(config, provider);
        }
        "check_model" => config.check_model = optional(value),
        "rewrite_model" => config.rewrite_model = optional(value),
        "monthly_cap_usd" => {
            config.monthly_cap_usd = value
                .parse()
                .ok()
                .filter(|v: &f64| *v >= 0.0)
                .context("monthly_cap_usd must be a number ≥ 0 (0 turns the cap off)")?
        }
        "locale" => config.locale = value.to_owned(),
        other => bail!(
            "unknown setting `{other}` (provider, check_model, rewrite_model, monthly_cap_usd, locale)"
        ),
    }
    for model in [config.check_model(), config.rewrite_model()] {
        if writer_llm::pricing::price_for(&model).is_none() {
            eprintln!(
                "warning: no price known for `{model}`, so its cost won't count toward the cap. Known: {}",
                writer_llm::pricing::known_models()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    Ok(())
}

fn toml_string(config: &Config) -> String {
    let mut out = String::new();
    out.push_str(&format!("provider = \"{}\"\n", config.provider));
    out.push_str(&format!("check_model = \"{}\"\n", config.check_model()));
    out.push_str(&format!("rewrite_model = \"{}\"\n", config.rewrite_model()));
    out.push_str(&format!("monthly_cap_usd = {}\n", config.monthly_cap_usd));
    out.push_str(&format!("locale = \"{}\"\n", config.locale));
    out
}

fn month_line(store: &Store, config: &Config) -> Result<String> {
    let spent = store.month_spend()?;
    Ok(if config.monthly_cap_usd > 0.0 {
        format!(
            "this month ${spent:.4} of ${:.2} cap",
            config.monthly_cap_usd
        )
    } else {
        format!("this month ${spent:.4} (no cap)")
    })
}

fn print_reports(reports: &[ParagraphReport]) {
    let color = std::io::stdout().is_terminal();
    let paint = |code: &str, s: &str| {
        if color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_owned()
        }
    };
    let mut out = std::io::stdout().lock();
    for report in reports {
        for s in &report.suggestions {
            let _ = writeln!(
                out,
                "{}:{}  {:<11} {} → {}  {}",
                report.line + 1,
                s.start + 1,
                format!("{:?}", s.kind).to_lowercase(),
                paint("31", &format!("\"{}\"", s.original)),
                paint("32", &format!("\"{}\"", s.replacement)),
                paint("2", &s.explanation),
            );
        }
    }
}

fn apply_all(text: &str, reports: &[ParagraphReport]) -> String {
    text.split('\n')
        .enumerate()
        .map(
            |(line, paragraph)| match reports.iter().find(|r| r.line == line) {
                Some(r) => engine::apply(paragraph, &r.suggestions),
                None => paragraph.to_owned(),
            },
        )
        .collect::<Vec<_>>()
        .join("\n")
}
