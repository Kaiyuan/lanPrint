use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{
    PasswordHash as ArgonHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid access code")]
    InvalidPassword,
    #[error("access code is required")]
    PasswordRequired,
    #[error("permission denied")]
    Forbidden,
    #[error("token expired")]
    Expired,
    #[error("invalid token")]
    InvalidToken,
    #[error("internal authentication error: {0}")]
    Internal(String),
}

/// Wrapper around a PHC-formatted Argon2id hash.
#[derive(Clone, Debug)]
pub struct PasswordHash(String);

impl PasswordHash {
    pub fn create(password: &str) -> Result<Self, AuthError> {
        if password.trim().is_empty() {
            return Err(AuthError::PasswordRequired);
        }
        // Keep memory use modest (8 MiB) so the app remains friendly to low
        // end and Windows 7 machines, while still using Argon2id.
        let params = Params::new(8192, 2, 1, None)
            .map_err(|e| AuthError::Internal(format!("argon2 params: {e}")))?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let salt = SaltString::generate(&mut OsRng);
        let hash = argon
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| AuthError::Internal(format!("hash password: {e}")))?;
        Ok(Self(hash.to_string()))
    }

    pub fn from_string(value: &str) -> Result<Self, AuthError> {
        ArgonHash::new(value)
            .map(|_| Self(value.to_string()))
            .map_err(|_| AuthError::Internal("invalid password hash".into()))
    }

    pub fn verify(&self, password: &str) -> bool {
        let Ok(parsed) = ArgonHash::new(&self.0) else {
            return false;
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenClaims {
    /// Issuing device id. Tokens are only valid on the device that signed them.
    pub dev: String,
    /// `host` or `printer`.
    pub scope: String,
    /// Printer name for printer scoped tokens.
    #[serde(default)]
    pub printer: String,
    /// Display name of the client that requested a printer token.
    #[serde(default)]
    pub dname: String,
    /// Unix seconds.
    pub exp: i64,
    /// Random id to keep tokens unique.
    pub jti: String,
}

impl TokenClaims {
    pub fn host(device_id: &str, ttl_secs: i64) -> Self {
        Self {
            dev: device_id.to_string(),
            scope: "host".into(),
            printer: String::new(),
            dname: String::new(),
            exp: chrono::Utc::now().timestamp() + ttl_secs,
            jti: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub fn printer(device_id: &str, printer: &str, ttl_secs: i64) -> Self {
        Self {
            dev: device_id.to_string(),
            scope: "printer".into(),
            printer: printer.to_string(),
            dname: String::new(),
            exp: chrono::Utc::now().timestamp() + ttl_secs,
            jti: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub fn is_expired(&self) -> bool {
        self.exp <= chrono::Utc::now().timestamp()
    }
}

pub fn decode_secret(secret: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(secret)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(secret))
        .unwrap_or_else(|_| secret.as_bytes().to_vec())
}

/// Issue a compact `payload.signature` token (base64url, no padding).
pub fn issue_token(secret: &[u8], claims: &TokenClaims) -> String {
    let payload = serde_json::to_vec(claims).unwrap_or_default();
    let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key size");
    mac.update(payload_b64.as_bytes());
    let sig = mac.finalize().into_bytes();
    format!(
        "{payload_b64}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig)
    )
}

pub fn verify_token(secret: &[u8], token: &str) -> Result<TokenClaims, AuthError> {
    let (payload_b64, sig_b64) = token.split_once('.').ok_or(AuthError::InvalidToken)?;
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| AuthError::InvalidToken)?;
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key size");
    mac.update(payload_b64.as_bytes());
    mac.verify_slice(&sig)
        .map_err(|_| AuthError::InvalidToken)?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| AuthError::InvalidToken)?;
    let claims: TokenClaims =
        serde_json::from_slice(&payload).map_err(|_| AuthError::InvalidToken)?;
    if claims.is_expired() {
        return Err(AuthError::Expired);
    }
    Ok(claims)
}

/// Verify that a token grants access to a printer (or to the host itself).
pub fn authorize_printer(
    secret: &[u8],
    token: &str,
    device_id: &str,
    printer: &str,
) -> Result<TokenClaims, AuthError> {
    let claims = verify_token(secret, token)?;
    if claims.dev != device_id {
        return Err(AuthError::InvalidToken);
    }
    match claims.scope.as_str() {
        "host" => Ok(claims),
        "printer" if claims.printer == printer => Ok(claims),
        _ => Err(AuthError::Forbidden),
    }
}

pub fn authorize_host(
    secret: &[u8],
    token: &str,
    device_id: &str,
) -> Result<TokenClaims, AuthError> {
    let claims = verify_token(secret, token)?;
    if claims.dev != device_id || claims.scope != "host" {
        return Err(AuthError::Forbidden);
    }
    Ok(claims)
}

pub const HOST_TOKEN_TTL: i64 = 7 * 24 * 3600;
pub const PRINTER_TOKEN_TTL: i64 = 30 * 24 * 3600;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let hash = PasswordHash::create("hunter2").unwrap();
        assert!(hash.verify("hunter2"));
        assert!(!hash.verify("wrong"));
    }

    #[test]
    fn token_roundtrip() {
        let secret = b"0123456789abcdef0123456789abcdef";
        let claims = TokenClaims::host("dev-1", 60);
        let token = issue_token(secret, &claims);
        let decoded = verify_token(secret, &token).unwrap();
        assert_eq!(decoded.dev, "dev-1");
        assert_eq!(decoded.scope, "host");
    }

    #[test]
    fn wrong_device_rejected() {
        let secret = b"0123456789abcdef0123456789abcdef";
        let token = issue_token(secret, &TokenClaims::host("dev-1", 60));
        assert!(authorize_host(secret, &token, "dev-2").is_err());
    }
}
