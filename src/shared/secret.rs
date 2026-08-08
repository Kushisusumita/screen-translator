//! Storage for API tokens.
//!
//! The config file lives in `%AppData%`, which is readable by anything running
//! as the user. A translation API key is a billable credential, so it is not
//! written there in the clear: on Windows it is sealed with DPAPI
//! (`CryptProtectData`), which ties the ciphertext to the current user account.
//!
//! This is not protection against malware already running as the user — nothing
//! stored locally can be. It stops the far more common accident: a config file
//! copied to another machine, pasted into an issue, or picked up by a backup.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Marks values that went through DPAPI. Anything else is treated as a key the
/// user pasted into the file by hand and is re-sealed on the next save.
const SEALED_PREFIX: &str = "dpapi:";

/// An API token. Never printed by `Debug` or `Display`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            f.write_str("Secret(empty)")
        } else {
            f.write_str("Secret(***)")
        }
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.0.is_empty() {
            return s.serialize_str("");
        }
        match seal(&self.0) {
            Some(sealed) => s.serialize_str(&format!("{SEALED_PREFIX}{sealed}")),
            None => s.serialize_str(&self.0),
        }
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        match raw.strip_prefix(SEALED_PREFIX) {
            // A key sealed by another user account cannot be opened here. Treat
            // it as absent rather than failing the whole config parse.
            Some(sealed) => Ok(Secret(unseal(sealed).unwrap_or_default())),
            None => Ok(Secret(raw)),
        }
    }
}

// ── Platform sealing ─────────────────────────────────────────────────────────

#[cfg(windows)]
fn seal(plain: &str) -> Option<String> {
    use base64::Engine as _;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    unsafe {
        let mut input = plain.as_bytes().to_vec();
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: input.len() as u32,
            pbData: input.as_mut_ptr(),
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();

        CryptProtectData(
            &in_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
        .ok()?;

        let bytes = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(out_blob.pbData as *mut _));

        Some(base64::engine::general_purpose::STANDARD.encode(bytes))
    }
}

#[cfg(windows)]
fn unseal(sealed: &str) -> Option<String> {
    use base64::Engine as _;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut input = base64::engine::general_purpose::STANDARD
        .decode(sealed)
        .ok()?;

    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: input.len() as u32,
            pbData: input.as_mut_ptr(),
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();

        CryptUnprotectData(
            &in_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
        .ok()?;

        let bytes = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(out_blob.pbData as *mut _));

        String::from_utf8(bytes).ok()
    }
}

/// No sealing available — the token is stored as typed. macOS should route this
/// through the Keychain; until then the settings UI says so out loud.
#[cfg(not(windows))]
fn seal(_plain: &str) -> Option<String> {
    None
}

#[cfg(not(windows))]
fn unseal(_sealed: &str) -> Option<String> {
    None
}

/// Whether tokens are encrypted at rest on this platform.
pub const fn sealing_available() -> bool {
    cfg!(windows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_leaks_the_value() {
        let s = Secret::new("sk-super-secret-value");
        assert_eq!(format!("{s:?}"), "Secret(***)");
    }

    #[cfg(windows)]
    #[test]
    fn seal_roundtrips() {
        let sealed = seal("hello-🌸-token").expect("DPAPI available");
        assert_eq!(unseal(&sealed).as_deref(), Some("hello-🌸-token"));
    }

    #[test]
    fn unsealable_value_reads_as_empty_rather_than_failing() {
        let toml = format!("key = \"{SEALED_PREFIX}not-valid-base64!!\"");
        #[derive(serde::Deserialize)]
        struct Holder {
            key: Secret,
        }
        let h: Holder = toml::from_str(&toml).expect("parse must not fail");
        assert!(h.key.is_empty());
    }
}
