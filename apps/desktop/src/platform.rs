//! OS-specific pieces: the macOS Accessibility permission (needed to send
//! the copy and paste keystrokes) and waiting for the hotkey to be released.

use std::time::Duration;

/// Whether the app may send keystrokes to other apps. With `prompt`, macOS
/// shows its "allow TextAssistance to control your computer" dialog.
pub fn accessibility_trusted(prompt: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::accessibility_trusted(prompt)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = prompt;
        true
    }
}

/// Opens the system page where Accessibility access is granted.
pub fn open_accessibility_settings() {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

/// Waits until the user lets go of the hotkey's modifier keys, so they don't
/// mix with the Cmd+C we send (Ctrl+Alt+Cmd+C would not copy).
pub fn wait_for_modifiers_released(timeout: Duration) {
    #[cfg(target_os = "macos")]
    {
        let start = std::time::Instant::now();
        while mac::modifiers_down() && start.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(15));
        }
        // Give the key-up events a moment to reach the frontmost app.
        std::thread::sleep(Duration::from_millis(40));
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::thread::sleep(timeout.min(Duration::from_millis(300)));
    }
}

/// Opens a file with the default app for it (TextEdit for the settings file on macOS).
pub fn open_file(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg("-t")
        .arg(path)
        .spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(path)
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

#[cfg(target_os = "macos")]
mod mac {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::CFString;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceFlagsState(state_id: i32) -> u64;
    }

    /// `kCGEventSourceStateHIDSystemState`: the physical keyboard.
    const HID_SYSTEM_STATE: i32 = 1;
    /// Shift, Control, Option and Command (`kCGEventFlagMask*`).
    const MODIFIER_MASK: u64 = 0x0002_0000 | 0x0004_0000 | 0x0008_0000 | 0x0010_0000;

    pub fn accessibility_trusted(prompt: bool) -> bool {
        let key = CFString::from_static_string("AXTrustedCheckOptionPrompt");
        let value = if prompt {
            CFBoolean::true_value()
        } else {
            CFBoolean::false_value()
        };
        let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);
        // SAFETY: `options` is a valid CFDictionary that outlives the call.
        unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
    }

    pub fn modifiers_down() -> bool {
        // SAFETY: plain C call with a valid state id, no pointers involved.
        unsafe { CGEventSourceFlagsState(HID_SYSTEM_STATE) & MODIFIER_MASK != 0 }
    }
}
