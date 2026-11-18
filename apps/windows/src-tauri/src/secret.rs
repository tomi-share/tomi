//! A short secret text that is shown to the user and must never reach a log: the pairing code
//! and link, the verification code (`spec/pairing-v1.md` §12).
//!
//! [`SecretText`] is wiped from memory when dropped, hides itself from `Debug` (a stray
//! `{:?}` or a failed assertion cannot print it) and has no `Display`. It serialises as the
//! plain string on purpose: that is the one way it leaves Rust, toward the screen that has to
//! show it.

use serde::{Serialize, Serializer};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct SecretText(Zeroizing<String>);

impl SecretText {
    pub fn new(text: impl Into<String>) -> Self {
        Self(Zeroizing::new(text.into()))
    }

    /// The text, for the screen that shows it. Not for a log, not for the clipboard.
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl PartialEq for SecretText {
    fn eq(&self, other: &Self) -> bool {
        self.expose() == other.expose()
    }
}

impl Eq for SecretText {}

impl std::fmt::Debug for SecretText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretText(<redacted>)")
    }
}

impl Serialize for SecretText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.expose())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_text_hides_itself_and_serialises_as_the_plain_string() {
        let s = SecretText::new("482 913");
        assert_eq!(format!("{s:?}"), "SecretText(<redacted>)");
        assert!(!format!("{:?}", Some(&s)).contains("482"));
        assert_eq!(serde_json::to_string(&s).unwrap(), "\"482 913\"");
        assert_eq!(s.expose(), "482 913");
        assert_eq!(s, s.clone());
        assert_ne!(s, SecretText::new("000 000"));
    }
}
