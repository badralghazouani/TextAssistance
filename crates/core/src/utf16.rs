//! Conversions between Rust's UTF-8 byte offsets and UTF-16 code-unit offsets.

/// Length of `s` in UTF-16 code units.
pub fn len(s: &str) -> u32 {
    s.chars().map(|c| c.len_utf16() as u32).sum()
}

/// Converts a UTF-8 byte offset into a UTF-16 offset.
///
/// Panics if `byte` is not on a char boundary, like slicing would.
pub fn from_byte(s: &str, byte: usize) -> u32 {
    len(&s[..byte])
}

/// Converts a UTF-16 offset into a UTF-8 byte offset.
///
/// Returns `None` if the offset is past the end or falls inside a surrogate pair.
pub fn to_byte(s: &str, offset: u32) -> Option<usize> {
    let mut units = 0u32;
    for (byte, c) in s.char_indices() {
        if units == offset {
            return Some(byte);
        }
        if units > offset {
            return None;
        }
        units += c.len_utf16() as u32;
    }
    (units == offset).then_some(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_offsets_match_bytes() {
        assert_eq!(len("hello"), 5);
        assert_eq!(from_byte("hello", 3), 3);
        assert_eq!(to_byte("hello", 3), Some(3));
        assert_eq!(to_byte("hello", 5), Some(5));
        assert_eq!(to_byte("hello", 6), None);
    }

    #[test]
    fn accented_and_astral_characters() {
        // "é" is 2 bytes / 1 unit, "😀" is 4 bytes / 2 units.
        let s = "é😀x";
        assert_eq!(len(s), 4);
        assert_eq!(from_byte(s, 2), 1);
        assert_eq!(from_byte(s, 6), 3);
        assert_eq!(to_byte(s, 1), Some(2));
        assert_eq!(to_byte(s, 3), Some(6));
        // Offset 2 is inside the emoji's surrogate pair.
        assert_eq!(to_byte(s, 2), None);
    }
}
