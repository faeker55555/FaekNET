use base64::Engine;
use chacha20poly1305::aead::{Aead, Generate};
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};

const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

pub struct Cipher {
    cipher: ChaCha20Poly1305,
}

impl Cipher {
    pub fn from_psk_b64(psk_b64: &str) -> Result<Cipher, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(psk_b64.trim())
            .map_err(|e| format!("PSK is not valid base64: {e}"))?;
        if bytes.len() != 32 {
            return Err(format!(
                "PSK must decode to exactly 32 bytes, got {} bytes",
                bytes.len()
            ));
        }
        let key = Key::try_from(bytes.as_slice())
            .map_err(|_| "PSK has the wrong length for a ChaCha20Poly1305 key".to_string())?;
        Ok(Cipher {
            cipher: ChaCha20Poly1305::new(&key),
        })
    }

    pub fn generate_psk_b64() -> String {
        let key = Key::generate();
        base64::engine::general_purpose::STANDARD.encode(key)
    }

    /// Encrypts `plaintext`, returning `nonce || ciphertext || tag` ready to
    /// send on the wire.
    pub fn seal(&self, plaintext: &[u8]) -> Vec<u8> {
        let nonce = Nonce::generate();
        let mut out = Vec::with_capacity(NONCE_LEN + plaintext.len() + TAG_LEN);
        out.extend_from_slice(&nonce);
        // encrypt() cannot fail for this AEAD/key setup.
        let ct = self
            .cipher
            .encrypt(&nonce, plaintext)
            .expect("chacha20poly1305 encryption failed unexpectedly");
        out.extend_from_slice(&ct);
        out
    }

    /// Decrypts a wire packet of the form `nonce || ciphertext || tag`.
    /// Returns None if the packet is malformed or fails authentication
    /// (wrong PSK, corrupted, or tampered) -- callers should silently drop
    /// such packets since the public internet is full of noise/scans.
    pub fn open(&self, wire: &[u8]) -> Option<Vec<u8>> {
        if wire.len() < NONCE_LEN + TAG_LEN {
            return None;
        }
        let (nonce_bytes, ct) = wire.split_at(NONCE_LEN);
        let nonce = Nonce::try_from(nonce_bytes).ok()?;
        self.cipher.decrypt(&nonce, ct).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let psk = Cipher::generate_psk_b64();
        let c = Cipher::from_psk_b64(&psk).unwrap();
        let msg = b"hello lan mesh";
        let wire = c.seal(msg);
        let out = c.open(&wire).unwrap();
        assert_eq!(out, msg);
    }

    #[test]
    fn rejects_tampered() {
        let psk = Cipher::generate_psk_b64();
        let c = Cipher::from_psk_b64(&psk).unwrap();
        let mut wire = c.seal(b"hello");
        let last = wire.len() - 1;
        wire[last] ^= 0xFF;
        assert!(c.open(&wire).is_none());
    }

    #[test]
    fn rejects_wrong_key() {
        let c1 = Cipher::from_psk_b64(&Cipher::generate_psk_b64()).unwrap();
        let c2 = Cipher::from_psk_b64(&Cipher::generate_psk_b64()).unwrap();
        let wire = c1.seal(b"hello");
        assert!(c2.open(&wire).is_none());
    }
}
