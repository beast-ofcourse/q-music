//! Hash, HMAC, base64 and encryption helpers behind `utils`.
//!
//! Every function here is pure and takes bytes, so the correctness of the
//! whole `utils` surface can be proven against published test vectors without
//! starting a JavaScript runtime.
//!
//! Encryption detail worth stating plainly: the upstream guide documents
//! `utils.encrypt(data, key)` returning `{ success, data }` but never names the
//! construction. None of the three shipped fixture packages calls it, so the
//! wire format is **this host's choice**, not a verified match: AES-256-GCM
//! with a SHA-256-derived key, a random 12-byte nonce prefixed to the
//! ciphertext, base64 encoded. Round-tripping through this host works; a blob
//! produced by a different host implementation will not decrypt here. That
//! limitation is recorded in `EXTENSIONS.md`.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// AES-GCM nonce length, in bytes.
pub const NONCE_LEN: usize = 12;
/// Largest key `generateKey` will produce; no real secret needs more.
pub const MAX_GENERATED_KEY_LEN: usize = 64;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("base64 input is not valid base64")]
    InvalidBase64,

    #[error("decrypted bytes are not valid UTF-8")]
    InvalidUtf8,

    #[error("ciphertext is too short to contain a nonce")]
    TruncatedCiphertext,

    #[error("decryption failed: wrong key or corrupted data")]
    DecryptionFailed,

    #[error("requested key length {requested} is outside 1..={max}")]
    InvalidKeyLength { requested: usize, max: usize },

    #[error("the operating system random source failed")]
    RandomSourceFailed,
}

pub fn base64_encode(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

pub fn base64_decode(text: &str) -> Result<Vec<u8>, CryptoError> {
    let trimmed = text.trim();
    BASE64
        .decode(trimmed)
        .map_err(|_| CryptoError::InvalidBase64)
}

pub fn md5_hex(bytes: &[u8]) -> String {
    hex(&md5_digest(bytes))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&sha256_digest(bytes))
}

fn md5_digest(bytes: &[u8]) -> [u8; 16] {
    let mut hasher = md5::Md5::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

fn sha256_digest(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

pub fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    hex(&hmac_sha256(key, message))
}

pub fn hmac_sha256_base64(key: &[u8], message: &[u8]) -> String {
    base64_encode(&hmac_sha256(key, message))
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    // HMAC accepts keys of any length, so this cannot fail.
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(message);
    mac.finalize().into_bytes().into()
}

/// Raw HMAC-SHA1 bytes, which is what TOTP needs.
pub fn hmac_sha1(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("HMAC-SHA1 accepts any key length");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

/// AES-256-GCM encrypt. Returns base64 of `nonce || ciphertext`.
pub fn encrypt(plaintext: &[u8], passphrase: &str) -> Result<String, CryptoError> {
    let key = sha256_digest(passphrase.as_bytes());
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| CryptoError::DecryptionFailed)?;

    let nonce_bytes = random_bytes(NONCE_LEN)?;
    let nonce: Nonce<_> =
        Nonce::try_from(nonce_bytes.as_slice()).map_err(|_| CryptoError::DecryptionFailed)?;

    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| CryptoError::DecryptionFailed)?;

    let mut sealed = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    sealed.extend_from_slice(&nonce_bytes);
    sealed.extend_from_slice(&ciphertext);

    Ok(base64_encode(&sealed))
}

/// Inverse of [`encrypt`]. Accepts the base64 blob it produced.
pub fn decrypt(sealed_base64: &str, passphrase: &str) -> Result<Vec<u8>, CryptoError> {
    let sealed = base64_decode(sealed_base64)?;
    if sealed.len() <= NONCE_LEN {
        return Err(CryptoError::TruncatedCiphertext);
    }

    let (nonce_bytes, ciphertext) = sealed.split_at(NONCE_LEN);
    let key = sha256_digest(passphrase.as_bytes());
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| CryptoError::DecryptionFailed)?;
    let nonce: Nonce<_> =
        Nonce::try_from(nonce_bytes).map_err(|_| CryptoError::TruncatedCiphertext)?;

    cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|_| CryptoError::DecryptionFailed)
}

/// Random bytes from the OS. Returns a typed error rather than panicking if
/// the platform source is unavailable.
pub fn random_bytes(length: usize) -> Result<Vec<u8>, CryptoError> {
    let mut buffer = vec![0u8; length];
    getrandom::fill(&mut buffer).map_err(|_| CryptoError::RandomSourceFailed)?;
    Ok(buffer)
}

/// A fresh key for `utils.generateKey`, as base64 and hex.
pub fn generate_key(length: usize) -> Result<(String, String), CryptoError> {
    if length == 0 || length > MAX_GENERATED_KEY_LEN {
        return Err(CryptoError::InvalidKeyLength {
            requested: length,
            max: MAX_GENERATED_KEY_LEN,
        });
    }

    let bytes = random_bytes(length)?;
    Ok((base64_encode(&bytes), hex(&bytes)))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The six-digit TOTP code from the guide's example, computed independently of
/// JavaScript so the extension-side result can be compared against it.
pub fn totp_code(key: &[u8], counter: u64, digits: u32) -> String {
    let digest = hmac_sha1(key, &counter.to_be_bytes());
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let binary = ((u32::from(digest[offset]) & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);

    let modulus = 10u32.pow(digits);
    format!("{:0width$}", binary % modulus, width = digits as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Published vectors. Sources: RFC 1321 (MD5), FIPS 180-4 (SHA-256),
    // RFC 4231 (HMAC-SHA256), RFC 2202 (HMAC-SHA1), RFC 4648 (base64).

    #[test]
    fn md5_matches_published_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn sha256_matches_published_vectors() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hmac_sha256_matches_rfc4231_case_two() {
        // key = "Jefe", data = "what do ya want for nothing?"
        let expected = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
        assert_eq!(hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"), expected);
        // Same 32 bytes, base64 encoded. Derived by encoding the verified hex
        // digest above rather than transcribed from a table.
        assert_eq!(
            hmac_sha256_base64(b"Jefe", b"what do ya want for nothing?"),
            "W9zBRr9gdU5qBCQmCJV1x1oAPwidJzmDnexYuWTsOEM="
        );
    }

    #[test]
    fn hmac_sha1_matches_rfc2202_case_two() {
        // key = "Jefe", data = "what do ya want for nothing?"
        let digest = hmac_sha1(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&digest),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
    }

    #[test]
    fn base64_round_trips_non_ascii_text() {
        let text = "λύρα — 音楽 — 🎵";
        let encoded = base64_encode(text.as_bytes());
        let decoded = base64_decode(&encoded).expect("decodes");
        assert_eq!(String::from_utf8(decoded).expect("utf8"), text);
    }

    #[test]
    fn base64_rejects_garbage() {
        assert_eq!(base64_decode("not base64!!"), Err(CryptoError::InvalidBase64));
    }

    #[test]
    fn encrypt_decrypt_round_trips() {
        let sealed = encrypt(b"secret payload", "correct horse").expect("encrypt");
        let opened = decrypt(&sealed, "correct horse").expect("decrypt");
        assert_eq!(opened, b"secret payload");
    }

    #[test]
    fn encrypt_uses_a_fresh_nonce_each_time() {
        let first = encrypt(b"same text", "key").expect("encrypt");
        let second = encrypt(b"same text", "key").expect("encrypt");
        assert_ne!(first, second, "identical ciphertext leaks equality");
    }

    #[test]
    fn decrypt_with_the_wrong_key_fails_cleanly() {
        let sealed = encrypt(b"secret", "right").expect("encrypt");
        assert_eq!(
            decrypt(&sealed, "wrong"),
            Err(CryptoError::DecryptionFailed)
        );
    }

    #[test]
    fn decrypt_rejects_truncated_ciphertext() {
        let short = base64_encode(&[0u8; 4]);
        assert_eq!(decrypt(&short, "key"), Err(CryptoError::TruncatedCiphertext));
    }

    #[test]
    fn generate_key_returns_base64_and_hex_of_the_length_asked_for() {
        let (encoded, hexed) = generate_key(32).expect("key");

        assert_eq!(base64_decode(&encoded).expect("base64").len(), 32);
        assert_eq!(hexed.len(), 64);
        assert!(hexed.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn generate_key_refuses_degenerate_lengths() {
        assert!(matches!(
            generate_key(0),
            Err(CryptoError::InvalidKeyLength { .. })
        ));
        assert!(matches!(
            generate_key(MAX_GENERATED_KEY_LEN + 1),
            Err(CryptoError::InvalidKeyLength { .. })
        ));
    }

    #[test]
    fn totp_matches_rfc6238_sha1_vector() {
        // RFC 6238 Appendix B, SHA-1, T = 59 s => counter 1, secret "12345678901234567890".
        let code = totp_code(b"12345678901234567890", 1, 8);
        assert_eq!(code, "94287082");
    }
}
