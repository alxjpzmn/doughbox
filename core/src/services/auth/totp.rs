use rand_core::{OsRng, RngCore};
use totp_rs::{Algorithm, TOTP};

use crate::{database::models::auth::UserId, services::shared::secrets::encode_hex};

use super::{password::canonical_username, AuthError};

const STEP_SECONDS: u64 = 30;
const SECRET_BYTES: usize = 20;
const RECOVERY_CONTEXT: &str = "doughbox-auth-recovery-v1";

pub struct TotpEnrollment {
    pub secret_base32: String,
    pub otpauth_uri: String,
    pub qr_png_base64: String,
}

pub fn new_secret() -> Result<Vec<u8>, AuthError> {
    let mut secret = vec![0_u8; SECRET_BYTES];
    OsRng
        .try_fill_bytes(&mut secret)
        .map_err(|_| AuthError::Unavailable)?;
    Ok(secret)
}

pub fn enrollment(secret: Vec<u8>, username: &str) -> Result<TotpEnrollment, AuthError> {
    let username = canonical_username(username)?;
    let totp = TOTP::new(
        Algorithm::SHA1,
        6,
        0,
        STEP_SECONDS,
        secret,
        Some("Doughbox".to_owned()),
        username,
    )
    .map_err(|_| AuthError::Unavailable)?;
    Ok(TotpEnrollment {
        secret_base32: totp.get_secret_base32(),
        otpauth_uri: totp.get_url(),
        qr_png_base64: totp.get_qr_base64().map_err(|_| AuthError::Unavailable)?,
    })
}

// The returned step must be persisted while holding the account row lock in the
// same transaction that consumes the factor. This pure helper cannot serialize
// two concurrent requests on its own.
pub fn verify_totp_step(
    secret: &[u8],
    code: &str,
    unix_seconds: u64,
    last_step: Option<i64>,
) -> Option<i64> {
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let totp = TOTP::new(
        Algorithm::SHA1,
        6,
        0,
        STEP_SECONDS,
        secret.to_vec(),
        None,
        String::new(),
    )
    .ok()?;
    let current = unix_seconds / STEP_SECONDS;
    let mut highest = None;
    for candidate in [
        current.checked_sub(1),
        Some(current),
        current.checked_add(1),
    ]
    .into_iter()
    .flatten()
    {
        let Some(timestamp) = candidate.checked_mul(STEP_SECONDS) else {
            continue;
        };
        let Ok(step) = i64::try_from(candidate) else {
            continue;
        };
        // Evaluate every candidate, even if a previous one matched. On a rare
        // truncated-code collision, consuming the highest step prevents replay.
        let matches = totp.check(code, timestamp);
        if matches && last_step.is_none_or(|last| step > last) {
            highest = Some(highest.map_or(step, |previous: i64| previous.max(step)));
        }
    }
    highest
}

pub fn generate_recovery_codes() -> Result<Vec<String>, AuthError> {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut codes = Vec::with_capacity(10);
    for _ in 0..10 {
        let mut bytes = [0_u8; 16];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| AuthError::Unavailable)?;
        let mut code = String::with_capacity(35);
        for (index, byte) in bytes.into_iter().enumerate() {
            if index != 0 && index % 4 == 0 {
                code.push('-');
            }
            code.push(DIGITS[(byte >> 4) as usize] as char);
            code.push(DIGITS[(byte & 0x0f) as usize] as char);
        }
        codes.push(code);
    }
    Ok(codes)
}

pub fn recovery_hash(user_id: &UserId, code: &str) -> Option<String> {
    let mut normalized = [0_u8; 32];
    let mut length = 0;
    for byte in code.trim().bytes() {
        if byte == b'-' {
            continue;
        }
        if !byte.is_ascii_hexdigit() || length == normalized.len() {
            return None;
        }
        normalized[length] = byte.to_ascii_uppercase();
        length += 1;
    }
    if length != normalized.len() {
        return None;
    }
    let mut hasher = blake3::Hasher::new_derive_key(RECOVERY_CONTEXT);
    hasher.update(user_id.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(&normalized);
    Some(encode_hex(hasher.finalize().as_bytes().iter().copied()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc6238_sha1_vectors_use_six_digits_including_leading_zeroes() {
        for (timestamp, code) in [
            (59, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
            (20_000_000_000, "353130"),
        ] {
            let step = (timestamp / STEP_SECONDS) as i64;
            assert_eq!(
                verify_totp_step(RFC_SECRET, code, timestamp, None),
                Some(step)
            );
            assert_eq!(
                verify_totp_step(RFC_SECRET, code, timestamp, Some(step)),
                None
            );
            assert_eq!(
                verify_totp_step(RFC_SECRET, code, timestamp, Some(step + 1)),
                None
            );
            assert_eq!(
                verify_totp_step(RFC_SECRET, code, timestamp, Some(step - 1)),
                Some(step)
            );
        }
    }

    #[test]
    fn exact_boundaries_accept_one_step_but_not_two_steps_of_skew() {
        // RFC4226 counter 3 is 969429, valid at timestamps [60, 149].
        for timestamp in [60, 89, 90, 119, 120, 149] {
            assert_eq!(
                verify_totp_step(RFC_SECRET, "969429", timestamp, None),
                Some(3)
            );
        }
        for timestamp in [59, 150] {
            assert_eq!(
                verify_totp_step(RFC_SECRET, "969429", timestamp, None),
                None
            );
        }
        assert_eq!(verify_totp_step(RFC_SECRET, "969429", 120, Some(3)), None);
    }

    #[test]
    fn epoch_has_no_negative_step_and_invalid_input_is_not_normalized() {
        assert_eq!(verify_totp_step(RFC_SECRET, "755224", 0, None), Some(0));
        assert_eq!(verify_totp_step(RFC_SECRET, "287082", 0, None), Some(1));
        assert_eq!(verify_totp_step(RFC_SECRET, "359152", 0, None), None);
        assert_eq!(verify_totp_step(RFC_SECRET, "755224", 29, Some(0)), None);
        for code in [
            "",
            "75522",
            "7552240",
            " 755224",
            "755224 ",
            "７５５２２４",
            "abcdef",
        ] {
            assert_eq!(verify_totp_step(RFC_SECRET, code, 0, None), None);
        }
        assert_eq!(verify_totp_step(b"short", "755224", 0, None), None);
        // No candidate timestamp multiplication may overflow.
        let totp = TOTP::new(
            Algorithm::SHA1,
            6,
            0,
            STEP_SECONDS,
            RFC_SECRET.to_vec(),
            None,
            String::new(),
        )
        .unwrap();
        assert_eq!(
            verify_totp_step(RFC_SECRET, &totp.generate(u64::MAX), u64::MAX, None),
            Some((u64::MAX / STEP_SECONDS) as i64),
        );
    }

    #[test]
    fn enrollment_uses_canonical_account_and_locally_generated_png() {
        use base64::{engine::general_purpose::STANDARD, Engine};

        let result = enrollment(RFC_SECRET.to_vec(), " Alice ").unwrap();
        assert_eq!(result.secret_base32, "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        let totp = TOTP::from_url(&result.otpauth_uri).unwrap();
        assert_eq!(totp.account_name, "alice");
        assert_eq!(totp.issuer.as_deref(), Some("Doughbox"));
        assert_eq!(totp.algorithm, Algorithm::SHA1);
        assert_eq!(totp.digits, 6);
        assert_eq!(totp.step, 30);
        let png = STANDARD.decode(&result.qr_png_base64).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(enrollment(RFC_SECRET.to_vec(), "[::1]:8080").is_err());
    }

    #[test]
    fn recovery_codes_normalize_only_case_hyphens_and_outer_whitespace() {
        let owner = UserId::from_database("00000000-0000-0000-0000-000000000001".to_owned());
        let other = UserId::from_database("00000000-0000-0000-0000-000000000002".to_owned());
        let plain = "0123456789ABCDEF0123456789ABCDEF";
        let hash = recovery_hash(&owner, plain).unwrap();
        assert_eq!(
            recovery_hash(&owner, " \t01234567-89abcdef-01234567-89abcdef\n"),
            Some(hash.clone())
        );
        assert_ne!(recovery_hash(&other, plain), Some(hash.clone()));
        let material = format!("{}\0{}", owner.as_str(), plain);
        let expected = blake3::derive_key(RECOVERY_CONTEXT, material.as_bytes());
        assert_eq!(hash, encode_hex(expected.into_iter()));
        for invalid in [
            "",
            "01234567 89ABCDEF0123456789ABCDEF",
            "0123456789ABCDEG0123456789ABCDEF",
            "0123456789ABCDEF0123456789ABCDE",
            "0123456789ABCDEF0123456789ABCDEF0",
        ] {
            assert!(recovery_hash(&owner, invalid).is_none());
        }
    }

    #[test]
    fn generated_secrets_and_codes_have_required_entropy_shapes() {
        let owner = UserId::from_database("00000000-0000-0000-0000-000000000001".to_owned());
        assert_eq!(new_secret().unwrap().len(), 20);
        let codes = generate_recovery_codes().unwrap();
        assert_eq!(codes.len(), 10);
        for code in codes {
            assert_eq!(code.len(), 35);
            assert_eq!(
                code.split('-').map(str::len).collect::<Vec<_>>(),
                vec![8, 8, 8, 8]
            );
            assert!(code.bytes().all(|byte| byte == b'-'
                || byte.is_ascii_digit()
                || (b'A'..=b'F').contains(&byte)));
            assert!(recovery_hash(&owner, &code).is_some());
        }
    }
}
