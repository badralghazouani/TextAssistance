//! Platform-independent text engine: paragraph tracking, edit diffing,
//! suggestion anchoring and request scheduling. No I/O lives here.

pub mod anchor;
pub mod debounce;
pub mod doc;
pub mod locate;
pub mod utf16;

pub use anchor::{Edit, Span};
pub use debounce::Debouncer;
pub use doc::{Document, Paragraph};
pub use locate::Locator;

/// Stable 64-bit hash used for paragraph change detection and cache keys.
pub fn hash_text(text: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(text.as_bytes())
}
