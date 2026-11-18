//! Source of randomness of the pairing machines.
//!
//! In production this is the OS CSPRNG. The known-answer tests (`pairing::kat`) replace it
//! with a script of fixed values, drawn in a documented order. A script never falls back to
//! the OS: an exhausted or mis-sized script is an error, so a test can not silently become
//! non-deterministic.

use std::collections::VecDeque;

use zeroize::Zeroize;

use crate::error::{CoreError, Result};

pub(crate) struct Entropy {
    script: Option<VecDeque<Vec<u8>>>,
}

impl Entropy {
    /// The OS CSPRNG.
    pub(crate) fn os() -> Self {
        Self { script: None }
    }

    /// Fixed values, handed out one chunk per [`fill`](Self::fill) call, in order.
    pub(crate) fn scripted(chunks: Vec<Vec<u8>>) -> Self {
        Self {
            script: Some(chunks.into()),
        }
    }

    /// Fills `out` with random bytes (or with the next scripted chunk, which must have the
    /// same length).
    pub(crate) fn fill(&mut self, out: &mut [u8]) -> Result<()> {
        match &mut self.script {
            None => getrandom::fill(out).map_err(|_| CoreError::Randomness),
            Some(queue) => {
                let mut chunk = queue.pop_front().ok_or_else(|| CoreError::Internal {
                    reason: "entropy script exhausted".into(),
                })?;
                let ok = chunk.len() == out.len();
                if ok {
                    out.copy_from_slice(&chunk);
                }
                chunk.zeroize();
                if ok {
                    Ok(())
                } else {
                    Err(CoreError::Internal {
                        reason: "entropy script chunk has the wrong length".into(),
                    })
                }
            }
        }
    }

    /// `N` random bytes. The caller wraps secrets in `Zeroizing`.
    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        self.fill(&mut out)?;
        Ok(out)
    }
}

impl Drop for Entropy {
    fn drop(&mut self) {
        if let Some(queue) = &mut self.script {
            for chunk in queue.iter_mut() {
                chunk.zeroize();
            }
        }
    }
}

impl std::fmt::Debug for Entropy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Entropy(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_is_served_in_order_and_never_falls_back() {
        let mut e = Entropy::scripted(vec![vec![1, 2], vec![3; 4]]);
        assert_eq!(e.array::<2>().unwrap(), [1, 2]);
        assert_eq!(e.array::<4>().unwrap(), [3; 4]);
        assert_eq!(e.array::<1>().unwrap_err().code(), "internal");
        let mut wrong = Entropy::scripted(vec![vec![1, 2, 3]]);
        assert_eq!(wrong.array::<2>().unwrap_err().code(), "internal");
    }

    #[test]
    fn os_entropy_fills() {
        let mut e = Entropy::os();
        let a = e.array::<32>().unwrap();
        let b = e.array::<32>().unwrap();
        assert_ne!(a, b);
        assert_eq!(format!("{e:?}"), "Entropy(<redacted>)");
    }
}
