use anyhow::{anyhow, bail, Result};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use rand_core::{OsRng, RngCore};

const NONCE_BYTES: usize = 24;
const TAG_BYTES: usize = 16;

pub fn parse_master_key(encoded: &str) -> Result<[u8; 32]> {
    let mut key = [0_u8; 32];
    if !decode_hex_into(encoded, &mut key) {
        bail!("Master key must contain exactly 64 hexadecimal characters");
    }
    Ok(key)
}

// Keep the existing unprefixed trading envelope: hex(nonce || ciphertext || tag).
// Callers supply their own AAD and any version prefix outside this envelope.
pub fn encrypt(plaintext: &[u8], key: &[u8; 32], aad: &[u8]) -> Result<String> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce = [0_u8; NONCE_BYTES];
    OsRng
        .try_fill_bytes(&mut nonce)
        .map_err(|_| anyhow!("Secure randomness is unavailable"))?;
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| anyhow!("Secret could not be encrypted"))?;
    Ok(encode_hex(nonce.iter().chain(ciphertext.iter()).copied()))
}

pub fn decrypt(envelope: &str, key: &[u8; 32], aad: &[u8]) -> Result<Vec<u8>> {
    if envelope.len() < (NONCE_BYTES + TAG_BYTES) * 2 || envelope.len() % 2 != 0 {
        bail!("Encrypted secret is invalid");
    }
    let mut encrypted = vec![0_u8; envelope.len() / 2];
    if !decode_hex_into(envelope, &mut encrypted) {
        bail!("Encrypted secret is invalid");
    }
    XChaCha20Poly1305::new(key.into())
        .decrypt(
            XNonce::from_slice(&encrypted[..NONCE_BYTES]),
            Payload {
                msg: &encrypted[NONCE_BYTES..],
                aad,
            },
        )
        .map_err(|_| anyhow!("Secret could not be decrypted"))
}

pub fn decode_hex_into(encoded: &str, output: &mut [u8]) -> bool {
    if encoded.len() != output.len() * 2 {
        return false;
    }
    fn digit(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    for (pair, slot) in encoded.as_bytes().chunks_exact(2).zip(output) {
        let (Some(high), Some(low)) = (digit(pair[0]), digit(pair[1])) else {
            return false;
        };
        *slot = (high << 4) | low;
    }
    true
}

pub fn encode_hex(bytes: impl Iterator<Item = u8>) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.size_hint().0 * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 15) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_envelopes_are_randomized_and_bound_to_key_and_aad() {
        let key = [7; 32];
        let plaintext = b"secret bytes\0\xff";
        let aad = b"doughbox-totp-v1:user:pending";
        let first = encrypt(plaintext, &key, aad).unwrap();
        let second = encrypt(plaintext, &key, aad).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.len(), (NONCE_BYTES + plaintext.len() + TAG_BYTES) * 2);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(decrypt(&first, &key, aad).unwrap(), plaintext);
        assert!(decrypt(&first, &[8; 32], aad).is_err());
        assert!(decrypt(&first, &key, b"doughbox-totp-v1:user:active").is_err());
        assert!(decrypt(&format!("v1:{first}"), &key, aad).is_err());

        let mut changed = first.into_bytes();
        changed[0] = if changed[0] == b'0' { b'1' } else { b'0' };
        assert!(decrypt(std::str::from_utf8(&changed).unwrap(), &key, aad).is_err());
    }

    #[test]
    fn empty_plaintext_is_supported() {
        let key = [7; 32];
        let envelope = encrypt(b"", &key, b"purpose").unwrap();
        assert!(decrypt(&envelope, &key, b"purpose").unwrap().is_empty());
    }

    #[test]
    fn invalid_envelopes_and_master_keys_are_rejected() {
        for encoded in ["", "0", "ff", "zz", &"0".repeat(79), &"g".repeat(80)] {
            assert!(decrypt(encoded, &[7; 32], b"purpose").is_err());
        }
        for encoded in [
            "",
            "key",
            &"0".repeat(63),
            &"0".repeat(65),
            &"g".repeat(64),
            &" ".repeat(64),
        ] {
            assert!(parse_master_key(encoded).is_err());
        }
        assert_eq!(parse_master_key(&"aB".repeat(32)).unwrap(), [0xab; 32]);
    }

    #[test]
    fn hex_helpers_preserve_bytes_and_reject_invalid_lengths_and_digits() {
        assert_eq!(encode_hex([0x00, 0x0f, 0xa0, 0xff].into_iter()), "000fa0ff");
        let mut decoded = [0; 4];
        assert!(decode_hex_into("000FA0fF", &mut decoded));
        assert_eq!(decoded, [0x00, 0x0f, 0xa0, 0xff]);
        for invalid in ["000fa0f", "000fa0fff", "000fa0fg", "000fa0é"] {
            assert!(!decode_hex_into(invalid, &mut decoded));
        }
    }
}
