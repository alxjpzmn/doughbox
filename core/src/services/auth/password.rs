use std::sync::Arc;

use argon2::{
    password_hash::{Error as PasswordHashError, PasswordHash, SaltString},
    Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version,
};
use rand_core::{OsRng, RngCore};
use tokio::sync::Semaphore;

use super::AuthError;

const MEMORY_KIB: u32 = 65_536;
const ITERATIONS: u32 = 3;
const PARALLELISM: u32 = 1;
const SALT_BYTES: usize = 16;
const OUTPUT_BYTES: usize = 32;
// A fixed, valid PHC envelope with the same cost as real accounts. Its output is
// deliberately unrelated to any password; dummy verification can never log in.
const DUMMY_PHC: &str = "$argon2id$v=19$m=65536,t=3,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

pub fn canonical_username(username: &str) -> Result<String, AuthError> {
    let canonical = username.trim().to_ascii_lowercase();
    let bytes = canonical.as_bytes();
    if !(3..=32).contains(&bytes.len())
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    {
        return Err(AuthError::InvalidInput(
            "Username must be 3–32 ASCII letters, digits, dots, underscores or hyphens, starting with a letter or digit.",
        ));
    }
    Ok(canonical)
}

pub fn validate_password(password: &str) -> Result<(), AuthError> {
    if password.len() > 512 || !(12..=128).contains(&password.chars().count()) {
        return Err(AuthError::InvalidInput(
            "Password must contain 12–128 characters and at most 512 UTF-8 bytes.",
        ));
    }
    Ok(())
}

#[derive(Clone)]
pub struct PasswordService {
    slots: Arc<Semaphore>,
}

pub struct PasswordVerification {
    pub valid: bool,
    pub replacement_hash: Option<String>,
}

impl PasswordService {
    // Create once for the application; clones share the same non-waiting bound.
    pub fn new() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(2)),
        }
    }

    pub async fn hash(&self, password: String) -> Result<String, AuthError> {
        validate_password(&password)?;
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| AuthError::Unavailable)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            hash_password(&password)
        })
        .await
        .map_err(|_| AuthError::Unavailable)?
    }

    pub async fn verify(
        &self,
        password: String,
        stored_phc: Option<String>,
    ) -> Result<PasswordVerification, AuthError> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| AuthError::Unavailable)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let parsed = PasswordHash::new(stored_phc.as_deref().unwrap_or(DUMMY_PHC))
                .map_err(|_| AuthError::Unavailable)?;
            // PasswordVerifier reads algorithm, version and costs from the PHC,
            // rather than imposing today's hashing parameters on old hashes.
            let matches = match Argon2::default().verify_password(password.as_bytes(), &parsed) {
                Ok(()) => true,
                Err(PasswordHashError::Password) => false,
                Err(_) => return Err(AuthError::Unavailable),
            };
            let valid = stored_phc.is_some() && matches;
            let replacement_hash = if valid && needs_rehash(&parsed)? {
                Some(hash_password(&password)?)
            } else {
                None
            };
            Ok(PasswordVerification {
                valid,
                replacement_hash,
            })
        })
        .await
        .map_err(|_| AuthError::Unavailable)?
    }
}

fn parameters() -> Result<Params, AuthError> {
    Params::new(MEMORY_KIB, ITERATIONS, PARALLELISM, Some(OUTPUT_BYTES))
        .map_err(|_| AuthError::Unavailable)
}

fn hash_password(password: &str) -> Result<String, AuthError> {
    let mut salt_bytes = [0_u8; SALT_BYTES];
    OsRng
        .try_fill_bytes(&mut salt_bytes)
        .map_err(|_| AuthError::Unavailable)?;
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|_| AuthError::Unavailable)?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, parameters()?)
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| AuthError::Unavailable)
}

fn needs_rehash(hash: &PasswordHash<'_>) -> Result<bool, AuthError> {
    let params = Params::try_from(hash).map_err(|_| AuthError::Unavailable)?;
    let mut salt_bytes = [0_u8; 64];
    let salt_len = hash
        .salt
        .ok_or(AuthError::Unavailable)?
        .decode_b64(&mut salt_bytes)
        .map_err(|_| AuthError::Unavailable)?
        .len();
    Ok(hash.algorithm.as_str() != "argon2id"
        || hash.version != Some(19)
        || params != parameters()?
        || salt_len != SALT_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_canonicalize_without_unicode_folding() {
        assert_eq!(
            canonical_username("  AlIce.Name-1_  ").unwrap(),
            "alice.name-1_"
        );
        assert_eq!(
            canonical_username("ALICE").unwrap(),
            canonical_username("alice").unwrap()
        );
        assert!(canonical_username(&"a".repeat(32)).is_ok());
        for invalid in [
            "",
            "ab",
            "_alice",
            ".alice",
            "alice bob",
            "álîce",
            "Ａlice",
            "a/b",
        ] {
            assert!(canonical_username(invalid).is_err());
        }
        assert!(canonical_username(&"a".repeat(33)).is_err());
    }

    #[test]
    fn password_policy_counts_scalars_without_transforming_passwords() {
        assert!(validate_password(&"a".repeat(11)).is_err());
        assert!(validate_password(&"a".repeat(12)).is_ok());
        assert!(validate_password(&"a".repeat(128)).is_ok());
        assert!(validate_password(&"a".repeat(129)).is_err());
        assert!(validate_password(&"🦀".repeat(128)).is_ok());
        assert!(validate_password(&"🦀".repeat(129)).is_err());
        assert!(validate_password("            ").is_ok());
        assert!(validate_password("e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}").is_ok());
    }

    #[tokio::test]
    async fn phc_round_trip_preserves_password_and_current_parameters() {
        let service = PasswordService::new();
        let password = "  PäSsword 🦀  ";
        let hash = service.hash(password.to_owned()).await.unwrap();
        let parsed = PasswordHash::new(&hash).unwrap();
        assert!(!needs_rehash(&parsed).unwrap());
        let result = service
            .verify(password.to_owned(), Some(hash.clone()))
            .await
            .unwrap();
        assert!(result.valid);
        assert!(result.replacement_hash.is_none());
        for wrong in [password.trim(), "  pässword 🦀  ", "  Pa\u{308}Ssword 🦀  "] {
            let result = service
                .verify(wrong.to_owned(), Some(hash.clone()))
                .await
                .unwrap();
            assert!(!result.valid);
            assert!(result.replacement_hash.is_none());
        }
        let dummy = service.verify(password.to_owned(), None).await.unwrap();
        assert!(!dummy.valid);
        assert!(dummy.replacement_hash.is_none());
    }

    #[tokio::test]
    async fn successful_old_phc_verification_upgrades_parameters() {
        let password = "old password is unchanged";
        let salt = SaltString::encode_b64(b"fixed-test-salt!").unwrap();
        let old = Argon2::new(
            Algorithm::Argon2i,
            Version::V0x10,
            Params::new(32, 1, 1, Some(16)).unwrap(),
        )
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string();
        let service = PasswordService::new();
        let wrong = service
            .verify("wrong".to_owned(), Some(old.clone()))
            .await
            .unwrap();
        assert!(!wrong.valid);
        assert!(wrong.replacement_hash.is_none());
        let result = service
            .verify(password.to_owned(), Some(old))
            .await
            .unwrap();
        assert!(result.valid);
        let replacement = result.replacement_hash.unwrap();
        assert!(!needs_rehash(&PasswordHash::new(&replacement).unwrap()).unwrap());
        assert!(
            service
                .verify(password.to_owned(), Some(replacement))
                .await
                .unwrap()
                .valid
        );
    }

    #[tokio::test]
    async fn saturated_hash_budget_rejects_instead_of_queueing() {
        let service = PasswordService::new();
        let clone = service.clone();
        let _permits = service.slots.clone().try_acquire_many_owned(2).unwrap();
        assert!(matches!(
            clone.hash("valid password".to_owned()).await,
            Err(AuthError::Unavailable)
        ));
        assert!(matches!(
            clone.verify("valid password".to_owned(), None).await,
            Err(AuthError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn corrupt_stored_phc_is_unavailable_not_a_credential_bypass() {
        let service = PasswordService::new();
        assert!(matches!(
            service
                .verify("valid password".to_owned(), Some("corrupt".to_owned()))
                .await,
            Err(AuthError::Unavailable)
        ));
    }
}
