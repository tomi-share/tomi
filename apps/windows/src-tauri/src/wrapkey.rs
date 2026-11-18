//! The key that wraps the secrets of the local database.
//!
//! The core's store seals the device keys, the local key and the space keys inside the
//! database under a 32-byte *wrapping key* supplied by the host (Android Keystore on the
//! phone, DPAPI here; `spec/api-v1.md` §7.1 and `crates/core/src/runtime/store.rs`). Without
//! it the database file reveals none of them.
//!
//! Here that key is random (the operating system generator), lives in memory only while the
//! app runs, and is kept on disk sealed by Windows DPAPI for the current user
//! (`CryptProtectData`): the files of the app copied to another PC, or opened under another
//! Windows account, cannot be unsealed. This is the documented gap with the spec (no
//! hardware-backed key, no per-use user presence): DPAPI protects against the theft of the
//! files, not against another process running as the same user.
//!
//! No cryptographic primitive is implemented here: the sealing is Windows' own, the random
//! bytes are the OS's.

use std::path::Path;

use zeroize::Zeroizing;

/// Length of the wrapping key expected by the core's store.
pub const KEY_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WrapKeyError {
    /// The database exists but its key file does not: a new key would lock the data for
    /// good, so none is made.
    #[error("the wrapping key file is missing")]
    Missing,
    /// The file is there but Windows cannot unseal it (another user or PC, or damaged).
    #[error("the wrapping key cannot be unsealed")]
    Unreadable,
    /// Reading or writing the file failed (kind only).
    #[error("io: {0}")]
    Io(String),
    /// The OS random generator failed.
    #[error("random generator unavailable")]
    Randomness,
    /// Sealing needs Windows (only the other platforms' stub reports it).
    #[error("DPAPI is not available on this platform")]
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
}

impl WrapKeyError {
    /// Stable machine code (no path, no key material).
    pub fn code(&self) -> &'static str {
        match self {
            WrapKeyError::Missing => "wrap_key_missing",
            WrapKeyError::Unreadable => "wrap_key_unreadable",
            WrapKeyError::Io(_) => "wrap_key_io",
            WrapKeyError::Randomness => "randomness",
            WrapKeyError::Unsupported => "unsupported",
        }
    }
}

/// The key, zeroed when dropped.
pub type WrapKey = Zeroizing<[u8; KEY_LEN]>;

/// Reads the key from `path` or, when the file does not exist and `may_create` is set,
/// makes a new one and stores it there. `may_create` is false when a database already
/// exists: it was sealed under a key that must not be silently replaced.
pub fn load_or_create(path: &Path, may_create: bool) -> Result<WrapKey, WrapKeyError> {
    match std::fs::read(path) {
        Ok(blob) => open_blob(&blob),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if may_create {
                create(path)
            } else {
                Err(WrapKeyError::Missing)
            }
        }
        Err(e) => Err(WrapKeyError::Io(format!("{:?}", e.kind()))),
    }
}

fn open_blob(blob: &[u8]) -> Result<WrapKey, WrapKeyError> {
    let plain = imp::unseal(blob)?;
    if plain.len() != KEY_LEN {
        return Err(WrapKeyError::Unreadable);
    }
    // Copy straight into the zeroized destination: `try_into()` would leave a temporary
    // `[u8; KEY_LEN]` (a `Copy` type) on the stack that nothing wipes.
    let mut key: WrapKey = Zeroizing::new([0u8; KEY_LEN]);
    key.copy_from_slice(plain.as_slice());
    Ok(key)
}

fn create(path: &Path) -> Result<WrapKey, WrapKeyError> {
    let mut key: WrapKey = Zeroizing::new([0u8; KEY_LEN]);
    getrandom::fill(key.as_mut_slice()).map_err(|_| WrapKeyError::Randomness)?;
    let blob = imp::seal(key.as_slice())?;
    let io = |e: std::io::Error| WrapKeyError::Io(format!("{:?}", e.kind()));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    // Atomic: a crash leaves either no file or the whole one.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &blob).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)?;
    Ok(key)
}

#[cfg(windows)]
mod imp {
    //! `CryptProtectData` / `CryptUnprotectData`, the only `unsafe` of the app.
    //!
    //! Justification (docs/DEV.md: no `unsafe` without a written reason): the DPAPI has no
    //! safe binding in the standard library and a keyring crate would store the key
    //! elsewhere (Credential Manager) without being safer. Each call passes pointers to
    //! buffers that outlive it, checks the result, copies the output and frees the buffer
    //! Windows allocated (`LocalFree`), wiping it first when it held the plaintext key.

    use super::WrapKeyError;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    use zeroize::{Zeroize, Zeroizing};

    /// Extra secret mixed into the sealing: a program that only calls
    /// `CryptUnprotectData` on the file, without knowing it, gets nothing. It is not a
    /// secret from this program's own binary, and does not stop a process that runs it.
    const ENTROPY: &[u8] = b"eu.shaffe.tomi/wrap-key/v1";

    fn blob_of(data: &[u8]) -> Result<CRYPT_INTEGER_BLOB, WrapKeyError> {
        let len = u32::try_from(data.len()).map_err(|_| WrapKeyError::Unreadable)?;
        Ok(CRYPT_INTEGER_BLOB {
            cbData: len,
            // DPAPI never writes to its input blobs; the type is `*mut` for the C API.
            pbData: data.as_ptr().cast_mut(),
        })
    }

    /// Copies the buffer Windows allocated into a `Vec` and frees it.
    ///
    /// # Safety
    /// `out` must have been filled by a successful `CryptProtectData` /
    /// `CryptUnprotectData` call and not used since.
    unsafe fn take(out: CRYPT_INTEGER_BLOB, wipe: bool) -> Zeroizing<Vec<u8>> {
        if out.pbData.is_null() {
            return Zeroizing::new(Vec::new());
        }
        let len = out.cbData as usize;
        let copy = Zeroizing::new(std::slice::from_raw_parts(out.pbData, len).to_vec());
        if wipe {
            std::slice::from_raw_parts_mut(out.pbData, len).zeroize();
        }
        LocalFree(out.pbData.cast());
        copy
    }

    pub fn seal(plain: &[u8]) -> Result<Vec<u8>, WrapKeyError> {
        let input = blob_of(plain)?;
        let entropy = blob_of(ENTROPY)?;
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: `input` and `entropy` point to live buffers for the whole call, `out` is
        // a valid destination; the optional pointers are null. The result is checked
        // before `take`, which owns the output buffer from then on.
        let sealed = unsafe {
            let ok = CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            );
            if ok == 0 {
                return Err(WrapKeyError::Unreadable);
            }
            take(out, false)
        };
        Ok(sealed.to_vec())
    }

    pub fn unseal(sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, WrapKeyError> {
        let input = blob_of(sealed)?;
        let entropy = blob_of(ENTROPY)?;
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: as in `seal`. The output holds the plaintext key: `take` wipes the
        // buffer Windows allocated before freeing it.
        unsafe {
            let ok = CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            );
            if ok == 0 {
                return Err(WrapKeyError::Unreadable);
            }
            Ok(take(out, true))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::WrapKeyError;
    use zeroize::Zeroizing;

    pub fn seal(_plain: &[u8]) -> Result<Vec<u8>, WrapKeyError> {
        Err(WrapKeyError::Unsupported)
    }

    pub fn unseal(_sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, WrapKeyError> {
        Err(WrapKeyError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_key_is_not_replaced_when_a_database_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wrap.key");
        assert_eq!(
            load_or_create(&path, false).unwrap_err(),
            WrapKeyError::Missing
        );
        assert!(!path.exists(), "no key was made behind the data's back");
    }

    #[test]
    fn error_codes_carry_no_detail() {
        assert_eq!(WrapKeyError::Missing.code(), "wrap_key_missing");
        assert_eq!(WrapKeyError::Io("NotFound".into()).code(), "wrap_key_io");
    }

    #[cfg(windows)]
    mod dpapi {
        use super::*;

        #[test]
        fn the_key_is_made_once_then_found_again() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("sub").join("wrap.key");
            let first = load_or_create(&path, true).unwrap();
            let again = load_or_create(&path, false).unwrap();
            assert_eq!(first.as_slice(), again.as_slice());
            assert_eq!(first.len(), KEY_LEN);
            assert!(first.iter().any(|b| *b != 0), "not all zeros");
            assert!(!path.with_extension("tmp").exists(), "no leftover");
        }

        #[test]
        fn the_file_on_disk_does_not_hold_the_key() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("wrap.key");
            let key = load_or_create(&path, true).unwrap();
            let blob = std::fs::read(&path).unwrap();
            assert!(blob.len() > KEY_LEN, "a DPAPI blob has a header");
            assert!(
                !blob.windows(KEY_LEN).any(|w| w == key.as_slice()),
                "the plaintext key is not in the file"
            );
        }

        #[test]
        fn two_keys_are_different() {
            let dir = tempfile::tempdir().unwrap();
            let a = load_or_create(&dir.path().join("a.key"), true).unwrap();
            let b = load_or_create(&dir.path().join("b.key"), true).unwrap();
            assert_ne!(a.as_slice(), b.as_slice());
        }

        #[test]
        fn a_damaged_file_is_unreadable_not_replaced() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("wrap.key");
            load_or_create(&path, true).unwrap();
            let mut blob = std::fs::read(&path).unwrap();
            let last = blob.len() - 1;
            blob[last] ^= 0xFF;
            std::fs::write(&path, &blob).unwrap();
            // Even with `may_create`: an existing file is never overwritten by a new key.
            assert_eq!(
                load_or_create(&path, true).unwrap_err(),
                WrapKeyError::Unreadable
            );
            assert_eq!(std::fs::read(&path).unwrap(), blob);
        }

        #[test]
        fn a_file_that_is_not_a_dpapi_blob_is_unreadable() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("wrap.key");
            std::fs::write(&path, [7u8; KEY_LEN]).unwrap();
            assert_eq!(
                load_or_create(&path, true).unwrap_err(),
                WrapKeyError::Unreadable
            );
        }

        #[test]
        fn sealing_round_trips_with_a_fresh_salt_each_time() {
            let sealed = imp::seal(&[9u8; KEY_LEN]).unwrap();
            assert_eq!(imp::unseal(&sealed).unwrap().as_slice(), &[9u8; KEY_LEN]);
            // Sealed twice, two different blobs (DPAPI adds its own random salt).
            assert_ne!(sealed, imp::seal(&[9u8; KEY_LEN]).unwrap());
        }
    }
}
