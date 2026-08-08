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

/// Marks values that went through this platform's sealing. Anything else is
/// treated as a key the user pasted into the file by hand and is re-sealed on
/// the next save.
const SEALED_PREFIX: &str = if cfg!(windows) { "dpapi:" } else { "keyring:" };

/// Both spellings are recognised on read. A config file carried between
/// platforms cannot be unsealed either way — the point is to recognise it as
/// sealed rather than hand the ciphertext to a translation API as a token.
const SEALED_PREFIXES: [&str; 2] = ["dpapi:", "keyring:"];

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
        match SEALED_PREFIXES
            .iter()
            .find_map(|p| raw.strip_prefix(p))
        {
            // A key sealed by another user account — or on another platform —
            // cannot be opened here. Treat it as absent rather than failing the
            // whole config parse.
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

#[cfg(not(windows))]
fn seal(plain: &str) -> Option<String> {
    keychain::seal(plain)
}

#[cfg(not(windows))]
fn unseal(sealed: &str) -> Option<String> {
    keychain::unseal(sealed)
}

/// The macOS Keychain and the Linux Secret Service stand in for DPAPI.
///
/// Neither seals a blob the way `CryptProtectData` does, so the shape is
/// inverted: one random key lives in the OS store, and the config file holds
/// tokens encrypted with it. The result is the same — a config file copied to
/// another machine carries nothing usable — and the keychain gains exactly one
/// entry rather than one per token.
#[cfg(not(windows))]
mod keychain {
    use base64::Engine as _;
    use chacha20poly1305::aead::{Aead, KeyInit};
    use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
    use once_cell::sync::Lazy;
    use rand::RngCore;
    use tracing::warn;

    const SERVICE: &str = "Sakura Screen Translator";
    const ACCOUNT: &str = "config-encryption-key";
    const NONCE_LEN: usize = 12;

    /// Read once: every save and load would otherwise hit the OS store, and on
    /// macOS a locked keychain prompts the user each time.
    static CIPHER: Lazy<Option<ChaCha20Poly1305>> = Lazy::new(load_or_create_cipher);

    fn load_or_create_cipher() -> Option<ChaCha20Poly1305> {
        let entry = match keyring::Entry::new(SERVICE, ACCOUNT) {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "No OS credential store; tokens stay unencrypted at rest");
                return None;
            }
        };

        if let Ok(stored) = entry.get_password() {
            if let Some(key) = base64::engine::general_purpose::STANDARD
                .decode(&stored)
                .ok()
                .filter(|k| k.len() == 32)
            {
                return Some(ChaCha20Poly1305::new(Key::from_slice(&key)));
            }
            warn!("The stored encryption key is unusable; replacing it");
        }

        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        let encoded = base64::engine::general_purpose::STANDARD.encode(key);
        if let Err(e) = entry.set_password(&encoded) {
            warn!(error = %e, "Could not store the encryption key; tokens stay unencrypted");
            return None;
        }
        Some(ChaCha20Poly1305::new(Key::from_slice(&key)))
    }

    pub fn seal(plain: &str) -> Option<String> {
        let cipher = CIPHER.as_ref()?;
        let mut nonce = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce), plain.as_bytes()).ok()?;

        let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&ciphertext);
        Some(base64::engine::general_purpose::STANDARD.encode(blob))
    }

    pub fn unseal(sealed: &str) -> Option<String> {
        let cipher = CIPHER.as_ref()?;
        let blob = base64::engine::general_purpose::STANDARD.decode(sealed).ok()?;
        if blob.len() <= NONCE_LEN {
            return None;
        }
        let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
        let plain = cipher.decrypt(Nonce::from_slice(nonce), ciphertext).ok()?;
        String::from_utf8(plain).ok()
    }

    /// Whether the OS store answered at all.
    pub fn available() -> bool {
        CIPHER.is_some()
    }
}

/// Whether tokens are encrypted at rest.
///
/// On Windows DPAPI is always there. Elsewhere it depends on the OS credential
/// store answering — a Linux box with no Secret Service running has none — so
/// the settings window asks rather than assumes.
pub fn sealing_available() -> bool {
    #[cfg(windows)]
    {
        true
    }
    #[cfg(not(windows))]
    {
        keychain::available()
    }
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
