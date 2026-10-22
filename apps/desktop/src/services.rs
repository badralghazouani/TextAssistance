//! macOS Services: "Check with TextAssistance" in the right-click menu of
//! selected text (Services submenu). Info.plist declares the service; this
//! registers the object that receives it.

#[cfg(target_os = "macos")]
pub use mac::register;

/// Services are a macOS feature; elsewhere there is nothing to register.
#[cfg(not(target_os = "macos"))]
pub fn register(_on_text: fn(String)) {}

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::OnceLock;

    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_app_kit::{NSApplication, NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::{NSObjectProtocol, NSString};

    static ON_TEXT: OnceLock<fn(String)> = OnceLock::new();

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and this class
        // doesn't implement Drop.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "TextAssistanceServiceProvider"]
        struct ServiceProvider;

        unsafe impl NSObjectProtocol for ServiceProvider {}

        impl ServiceProvider {
            /// Matches `NSMessage = checkText` in Info.plist.
            #[unsafe(method(checkText:userData:error:))]
            fn check_text(
                &self,
                pboard: &NSPasteboard,
                _user_data: Option<&NSString>,
                _error: *mut *mut NSString,
            ) {
                // SAFETY: reading a string from the pasteboard AppKit handed us.
                let text = unsafe { pboard.stringForType(NSPasteboardTypeString) };
                if let (Some(text), Some(on_text)) = (text, ON_TEXT.get()) {
                    on_text(text.to_string());
                }
            }
        }
    );

    impl ServiceProvider {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(());
            // SAFETY: NSObject's designated initializer.
            unsafe { msg_send![super(this), init] }
        }
    }

    /// Starts receiving "Check with TextAssistance" requests. Call once, on the
    /// main thread, after the app has started.
    pub fn register(on_text: fn(String)) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        if ON_TEXT.set(on_text).is_err() {
            return;
        }
        let provider = ServiceProvider::new(mtm);
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: the provider implements the method named in Info.plist.
        unsafe { app.setServicesProvider(Some(&provider)) };
        // AppKit doesn't keep a strong reference; the provider lives as long as the app.
        std::mem::forget(provider);
    }
}
