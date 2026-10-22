//! Reads the text you are writing in another app and puts a new version back,
//! using the clipboard and the standard copy / select-all / paste shortcuts.
//! This works the same in Teams, Chrome, Brave, Mail, Slack, Notes and others.
//!
//! On macOS every function here must run on the main thread: the keystroke
//! library looks up the keyboard layout (so it also works on AZERTY), and
//! macOS only allows that from the main thread.

use std::time::{Duration, Instant};

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

/// Longer selections are almost always a whole web page, not a message.
pub const MAX_CHARS: usize = 8_000;

#[cfg(target_os = "macos")]
const SHORTCUT_MODIFIER: Key = Key::Meta;
#[cfg(not(target_os = "macos"))]
const SHORTCUT_MODIFIER: Key = Key::Control;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error(
        "No text found. Click inside the text you're writing (or select it), then press the shortcut again."
    )]
    NoText,
    #[error(
        "That's {0} characters — too long to check at once. Select the part you want to check, then press the shortcut."
    )]
    TooLong(usize),
    #[error("Couldn't use the clipboard: {0}")]
    Clipboard(String),
    #[error("Couldn't send keystrokes: {0}")]
    Keys(String),
}

impl From<arboard::Error> for CaptureError {
    fn from(e: arboard::Error) -> Self {
        CaptureError::Clipboard(e.to_string())
    }
}

pub struct Captured {
    pub text: String,
    /// What was on the clipboard before, so it can be put back.
    pub saved_clipboard: Option<String>,
}

/// Copies the selection, or the whole field when nothing is selected.
pub fn capture() -> Result<Captured, CaptureError> {
    let mut clipboard = Clipboard::new()?;
    let saved = clipboard.get_text().ok();
    let marker = format!(
        "\u{2063}textassistance-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    clipboard.set_text(marker.clone())?;

    let mut enigo = keyboard()?;
    press_shortcut(&mut enigo, 'c')?;
    let mut text = wait_for_clipboard_change(&mut clipboard, &marker, Duration::from_millis(350));
    if text.is_none() {
        // Nothing selected: take everything in the field the cursor is in.
        press_shortcut(&mut enigo, 'a')?;
        std::thread::sleep(Duration::from_millis(60));
        press_shortcut(&mut enigo, 'c')?;
        text = wait_for_clipboard_change(&mut clipboard, &marker, Duration::from_millis(500));
    }
    restore(&mut clipboard, saved.as_deref());

    let text = text
        .filter(|t| !t.trim().is_empty())
        .ok_or(CaptureError::NoText)?;
    let chars = text.chars().count();
    if chars > MAX_CHARS {
        return Err(CaptureError::TooLong(chars));
    }
    Ok(Captured {
        text,
        saved_clipboard: saved,
    })
}

/// Pastes `text` over the current selection (the text captured earlier),
/// then puts the user's clipboard back.
pub fn paste(text: &str, saved_clipboard: Option<&str>) -> Result<(), CaptureError> {
    let mut clipboard = Clipboard::new()?;
    clipboard.set_text(text.to_owned())?;
    std::thread::sleep(Duration::from_millis(50));
    let mut enigo = keyboard()?;
    press_shortcut(&mut enigo, 'v')?;
    // The target app reads the clipboard when it handles the keystroke.
    std::thread::sleep(Duration::from_millis(500));
    restore(&mut clipboard, saved_clipboard);
    Ok(())
}

pub fn copy(text: &str) -> Result<(), CaptureError> {
    Clipboard::new()?.set_text(text.to_owned())?;
    Ok(())
}

fn keyboard() -> Result<Enigo, CaptureError> {
    Enigo::new(&Settings::default()).map_err(|e| CaptureError::Keys(e.to_string()))
}

fn press_shortcut(enigo: &mut Enigo, key: char) -> Result<(), CaptureError> {
    let keys = |e: enigo::InputError| CaptureError::Keys(e.to_string());
    enigo
        .key(SHORTCUT_MODIFIER, Direction::Press)
        .map_err(keys)?;
    let result = enigo.key(Key::Unicode(key), Direction::Click).map_err(keys);
    // Always release the modifier, even if the key itself failed.
    enigo
        .key(SHORTCUT_MODIFIER, Direction::Release)
        .map_err(keys)?;
    result
}

fn wait_for_clipboard_change(
    clipboard: &mut Clipboard,
    marker: &str,
    timeout: Duration,
) -> Option<String> {
    let start = Instant::now();
    loop {
        if let Ok(text) = clipboard.get_text()
            && text != marker
        {
            return Some(text);
        }
        if start.elapsed() >= timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn restore(clipboard: &mut Clipboard, saved: Option<&str>) {
    let _ = match saved {
        Some(text) => clipboard.set_text(text.to_owned()),
        None => clipboard.clear(),
    };
}
