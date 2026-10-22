//! TextAssistance: a menu-bar app that checks the text you are writing in any
//! app. Press the shortcut (⌃⌥G by default) while typing in Teams, Chrome,
//! Brave, Mail…; a popup shows the corrections, and Replace swaps them in.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod commands;
mod platform;
mod services;
mod tray;

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use commands::{ErrorView, SessionView};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, RunEvent, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use writer_store::config::{Config, Paths};

const POPUP: &str = "popup";

/// For callbacks from macOS (the Services menu) that don't carry an app handle.
static APP: OnceLock<AppHandle> = OnceLock::new();

/// The text being worked on, from the shortcut press until Replace or Cancel.
pub struct Session {
    pub view: SessionView,
    pub saved_clipboard: Option<String>,
}

#[derive(Default)]
pub struct AppState {
    pub session: Mutex<Option<Session>>,
}

fn main() {
    let config = Paths::detect()
        .and_then(|p| Config::load(&p.config_file))
        .unwrap_or_default();
    let hotkey = config.hotkey.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    // Called on the main thread (macOS: Carbon's application
                    // event target), which the keystroke code requires. Handling
                    // it right away, inside the key event, also lets the popup
                    // take keyboard focus.
                    if event.state() == ShortcutState::Pressed {
                        on_hotkey(app);
                    }
                })
                .build(),
        )
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_session,
            commands::check,
            commands::rewrite,
            commands::apply,
            commands::cancel,
            commands::copy_text,
            commands::save_api_key,
            commands::open_accessibility_settings,
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Ask for Accessibility access up front; the shortcut needs it.
            platform::accessibility_trusted(true);

            let shortcut: Shortcut = hotkey
                .parse()
                .map_err(|e| format!("invalid hotkey `{hotkey}` in settings: {e}"))?;
            app.global_shortcut().register(shortcut)?;
            tray::build(app, &hotkey)?;

            // Right-click → Services → "Check with TextAssistance".
            let _ = APP.set(app.handle().clone());
            services::register(on_service_text);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the popup only hides it; the app keeps running in the menu bar.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                hide_popup(window.app_handle());
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to start TextAssistance")
        .run(|_app, event| {
            // Stay alive with no window open; only "Quit" (exit code set) ends the app.
            if let RunEvent::ExitRequested { api, code, .. } = event
                && code.is_none()
            {
                api.prevent_exit();
            }
        });
}

/// Runs on the main thread when the global shortcut is pressed.
fn on_hotkey(app: &AppHandle) {
    let Some(popup) = app.get_webview_window(POPUP) else {
        return;
    };
    // Pressing the shortcut again while the popup has focus just closes it.
    if popup.is_visible().unwrap_or(false) && popup.is_focused().unwrap_or(false) {
        commands::cancel(app.clone(), app.state());
        return;
    }

    let session = if !platform::accessibility_trusted(false) {
        accessibility_session()
    } else {
        platform::wait_for_modifiers_released(Duration::from_millis(1500));
        match capture::capture() {
            Ok(captured) => Session {
                view: SessionView {
                    text: captured.text,
                    error: None,
                },
                saved_clipboard: captured.saved_clipboard,
            },
            Err(e) => error_session(&e),
        }
    };
    open_session(app, session);
}

/// Runs on the main thread when "Check with TextAssistance" is chosen from
/// the right-click Services menu; macOS hands over the selected text.
fn on_service_text(text: String) {
    let Some(app) = APP.get() else {
        return;
    };
    let session = if !platform::accessibility_trusted(false) {
        accessibility_session()
    } else {
        let chars = text.chars().count();
        if text.trim().is_empty() {
            error_session(&capture::CaptureError::NoText)
        } else if chars > capture::MAX_CHARS {
            error_session(&capture::CaptureError::TooLong(chars))
        } else {
            Session {
                view: SessionView { text, error: None },
                // Replace pastes through the clipboard, then restores this.
                saved_clipboard: capture::clipboard_text(),
            }
        }
    };
    open_session(app, session);
}

fn accessibility_session() -> Session {
    Session {
        view: SessionView {
            text: String::new(),
            error: Some(ErrorView::new(
                "accessibility",
                "TextAssistance needs Accessibility access to read and replace your text. \
                 Turn it on in System Settings → Privacy & Security → Accessibility, \
                 then try again.",
            )),
        },
        saved_clipboard: None,
    }
}

fn error_session(e: &capture::CaptureError) -> Session {
    let kind = match e {
        capture::CaptureError::NoText | capture::CaptureError::TooLong(_) => "no_text",
        _ => "other",
    };
    Session {
        view: SessionView {
            text: String::new(),
            error: Some(ErrorView::new(kind, e.to_string())),
        },
        saved_clipboard: None,
    }
}

/// Stores the session, shows the popup and hands it the text.
fn open_session(app: &AppHandle, session: Session) {
    let view = session.view.clone();
    *app.state::<AppState>().session.lock().unwrap() = Some(session);
    show_popup(app);
    let _ = app.emit_to(POPUP, "session", view);
}

/// Shows the popup next to the mouse pointer, kept on screen.
fn show_popup(app: &AppHandle) {
    let Some(popup) = app.get_webview_window(POPUP) else {
        return;
    };
    if let Ok(cursor) = app.cursor_position() {
        // Before the first show some platforms report a zero size; fall back
        // to the size set in tauri.conf.json.
        let scale = popup.scale_factor().unwrap_or(1.0);
        let (w, h) = match popup.outer_size() {
            Ok(size) if size.width > 100 && size.height > 100 => {
                (size.width as f64, size.height as f64)
            }
            _ => (560.0 * scale, 460.0 * scale),
        };
        let mut x = cursor.x - 40.0;
        let mut y = cursor.y + 24.0;
        if let Ok(Some(monitor)) = app.monitor_from_point(cursor.x, cursor.y) {
            let origin = monitor.position();
            let area = monitor.size();
            let (left, top) = (origin.x as f64, origin.y as f64);
            let (right, bottom) = (left + area.width as f64, top + area.height as f64);
            if y + h > bottom {
                y = cursor.y - h - 16.0;
            }
            x = x.clamp(left + 8.0, (right - w - 8.0).max(left));
            y = y.clamp(top + 8.0, (bottom - h - 8.0).max(top));
        }
        let _ = popup.set_position(PhysicalPosition::new(x, y));
    }
    #[cfg(target_os = "macos")]
    let _ = app.show();
    let _ = popup.show();
    let _ = popup.set_focus();
}

/// Hides the popup and hands focus back to the app the text came from.
pub fn hide_popup(app: &AppHandle) {
    if let Some(popup) = app.get_webview_window(POPUP) {
        let _ = popup.hide();
    }
    #[cfg(target_os = "macos")]
    let _ = app.hide();
}
