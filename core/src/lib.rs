//! Nudge core engine.
//!
//! Everything platform apps need lives behind this crate's public API:
//! networking, crypto, discovery and file handling. No UI code here.

/// Protocol version spoken by this build. Bumped on any wire-format change.
pub const PROTOCOL_VERSION: u16 = 1;

/// Crate version, for logs and the `--version` output of tools.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
