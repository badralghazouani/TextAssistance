//! The menu-bar icon and its menu.

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Manager, Wry};
use tauri_plugin_autostart::ManagerExt;
use writer_llm::ProviderKind;
use writer_store::Store;
use writer_store::config::{Config, Paths};

use crate::platform;

/// Menu items that change after the app starts.
struct TrayItems {
    claude: CheckMenuItem<Wry>,
    openai: CheckMenuItem<Wry>,
    usage: MenuItem<Wry>,
}

pub fn build(app: &App, hotkey: &str) -> tauri::Result<()> {
    let config = load_config();
    let hint = MenuItem::with_id(
        app,
        "hint",
        format!(
            "Select or click in text, then press {}",
            pretty_hotkey(hotkey)
        ),
        false,
        None::<&str>,
    )?;
    let claude = CheckMenuItem::with_id(
        app,
        "provider_claude",
        "Claude",
        true,
        config.provider == ProviderKind::Claude,
        None::<&str>,
    )?;
    let openai = CheckMenuItem::with_id(
        app,
        "provider_openai",
        "ChatGPT (OpenAI)",
        true,
        config.provider == ProviderKind::OpenAi,
        None::<&str>,
    )?;
    let provider = Submenu::with_items(app, "AI provider", true, &[&claude, &openai])?;
    let usage = MenuItem::with_id(app, "usage", usage_label(), false, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Start at login",
        true,
        app.autolaunch().is_enabled().unwrap_or(false),
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(app, "settings", "Open settings file…", true, None::<&str>)?;
    let accessibility = MenuItem::with_id(
        app,
        "accessibility",
        "Accessibility permission…",
        cfg!(target_os = "macos"),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit TextAssistance", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &hint,
            &PredefinedMenuItem::separator(app)?,
            &provider,
            &usage,
            &PredefinedMenuItem::separator(app)?,
            &autostart,
            &settings,
            &accessibility,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    app.manage(TrayItems {
        claude: claude.clone(),
        openai: openai.clone(),
        usage,
    });

    TrayIconBuilder::with_id("main")
        .icon(tauri::image::Image::from_bytes(include_bytes!(
            "../icons/tray.png"
        ))?)
        .icon_as_template(true)
        .tooltip("TextAssistance")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "provider_claude" => set_provider(app, ProviderKind::Claude),
            "provider_openai" => set_provider(app, ProviderKind::OpenAi),
            "autostart" => {
                let launcher = app.autolaunch();
                let enable = !launcher.is_enabled().unwrap_or(false);
                let _ = if enable {
                    launcher.enable()
                } else {
                    launcher.disable()
                };
                let _ = autostart.set_checked(launcher.is_enabled().unwrap_or(false));
            }
            "settings" => {
                if let Ok(paths) = Paths::detect() {
                    if !paths.config_file.exists() {
                        let _ = load_config().save(&paths.config_file);
                    }
                    platform::open_file(&paths.config_file);
                }
            }
            "accessibility" => {
                if !platform::accessibility_trusted(true) {
                    platform::open_accessibility_settings();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

/// Updates "Spent this month" after a check or rewrite.
pub fn refresh_usage(app: &AppHandle) {
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.usage.set_text(usage_label());
    }
}

fn set_provider(app: &AppHandle, provider: ProviderKind) {
    if let Ok(paths) = Paths::detect() {
        let mut config = load_config();
        if config.provider != provider {
            config.provider = provider;
            config.check_model = None;
            config.rewrite_model = None;
            let _ = config.save(&paths.config_file);
        }
    }
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.claude.set_checked(provider == ProviderKind::Claude);
        let _ = items.openai.set_checked(provider == ProviderKind::OpenAi);
    }
}

fn load_config() -> Config {
    Paths::detect()
        .and_then(|p| Config::load(&p.config_file))
        .unwrap_or_default()
}

fn usage_label() -> String {
    let config = load_config();
    let spent = Paths::detect()
        .and_then(|p| Store::open(&p.database))
        .and_then(|s| s.month_spend())
        .unwrap_or(0.0);
    if config.monthly_cap_usd > 0.0 {
        format!(
            "Spent this month: ${spent:.2} of ${:.2}",
            config.monthly_cap_usd
        )
    } else {
        format!("Spent this month: ${spent:.2}")
    }
}

/// "Ctrl+Alt+G" → "⌃⌥G" on macOS, unchanged elsewhere.
pub fn pretty_hotkey(hotkey: &str) -> String {
    if !cfg!(target_os = "macos") {
        return hotkey.to_owned();
    }
    hotkey
        .split('+')
        .map(|part| match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => "⌃".to_owned(),
            "alt" | "option" => "⌥".to_owned(),
            "shift" => "⇧".to_owned(),
            "cmd" | "command" | "super" | "meta" | "commandorcontrol" | "cmdorctrl" => {
                "⌘".to_owned()
            }
            other => other.trim_start_matches("key").to_uppercase(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_read_naturally() {
        let pretty = pretty_hotkey("Ctrl+Alt+G");
        if cfg!(target_os = "macos") {
            assert_eq!(pretty, "⌃⌥G");
        } else {
            assert_eq!(pretty, "Ctrl+Alt+G");
        }
    }
}
