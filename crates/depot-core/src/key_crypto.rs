use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum KeyEncryptionError {
    #[error("failed to decode key encryption key: {0}")]
    InvalidBase64(String),
    #[error("key encryption key must decode to 32 bytes, got {0}")]
    InvalidLength(usize),
    #[error("failed to generate encryption nonce: {0}")]
    NonceGeneration(String),
    #[error("failed to encrypt signing key")]
    Encrypt,
    #[error("failed to decrypt signing key")]
    Decrypt,
}

#[derive(Debug, Clone)]
pub struct KeyEncryptionKey([u8; 32]);

#[derive(Debug, Clone)]
pub struct EncryptedSigningKey {
    pub ciphertext: String,
    pub nonce: String,
}

impl KeyEncryptionKey {
    pub fn parse_base64(input: &str) -> Result<Self, KeyEncryptionError> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(input.trim())
            .map_err(|error| KeyEncryptionError::InvalidBase64(error.to_string()))?;

        if bytes.len() != 32 {
            return Err(KeyEncryptionError::InvalidLength(bytes.len()));
        }

        let mut key = [0_u8; 32];
        key.copy_from_slice(&bytes);

        Ok(Self(key))
    }

    pub fn encrypt(
        &self,
        plaintext: &str,
        aad: &[u8],
    ) -> Result<EncryptedSigningKey, KeyEncryptionError> {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.0));

        let mut nonce_bytes = [0_u8; 12];
        getrandom::fill(&mut nonce_bytes)
            .map_err(|error| KeyEncryptionError::NonceGeneration(error.to_string()))?;

        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                chacha20poly1305::aead::Payload {
                    msg: plaintext.as_bytes(),
                    aad,
                },
            )
            .map_err(|_| KeyEncryptionError::Encrypt)?;

        Ok(EncryptedSigningKey {
            ciphertext: base64::engine::general_purpose::STANDARD.encode(ciphertext),
            nonce: base64::engine::general_purpose::STANDARD.encode(nonce_bytes),
        })
    }

    pub fn decrypt(
        &self,
        encrypted: &EncryptedSigningKey,
        aad: &[u8],
    ) -> Result<String, KeyEncryptionError> {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.0));

        let nonce = base64::engine::general_purpose::STANDARD
            .decode(&encrypted.nonce)
            .map_err(|error| KeyEncryptionError::InvalidBase64(error.to_string()))?;
        if nonce.len() != 12 {
            return Err(KeyEncryptionError::Decrypt);
        }

        let ciphertext = base64::engine::general_purpose::STANDARD
            .decode(&encrypted.ciphertext)
            .map_err(|error| KeyEncryptionError::InvalidBase64(error.to_string()))?;

        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                chacha20poly1305::aead::Payload {
                    msg: &ciphertext,
                    aad,
                },
            )
            .map_err(|_| KeyEncryptionError::Decrypt)?;

        String::from_utf8(plaintext).map_err(|_| KeyEncryptionError::Decrypt)
    }
}
