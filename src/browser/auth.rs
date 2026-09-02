use std::{error::Error, fmt};

use subtle::ConstantTimeEq;

use crate::utils::configuration::Configuration;

const TOKEN_BYTES: usize = 32;

#[derive(Clone, Eq, PartialEq)]
pub struct PairingToken([u8; TOKEN_BYTES]);

impl fmt::Debug for PairingToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairingToken([redacted])")
    }
}

impl PairingToken {
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0_u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes)?;
        Ok(Self(bytes))
    }

    #[must_use]
    pub const fn from_bytes(bytes: [u8; TOKEN_BYTES]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(TOKEN_BYTES * 2);
        for byte in self.0 {
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        encoded
    }

    #[must_use]
    pub fn matches_hex(&self, candidate: &str) -> bool {
        decode_token(candidate)
            .map(|other| bool::from(self.0.ct_eq(&other)))
            .unwrap_or(false)
    }

    fn from_hex(encoded: &str) -> Option<Self> {
        decode_token(encoded).map(Self)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum AuthError {
    Random(getrandom::Error),
    InvalidStoredToken,
}

impl fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Random(error) => write!(formatter, "could not obtain OS randomness: {error}"),
            Self::InvalidStoredToken => {
                formatter.write_str("stored browser pairing token is invalid")
            }
        }
    }
}

impl Error for AuthError {}

impl From<getrandom::Error> for AuthError {
    fn from(error: getrandom::Error) -> Self {
        Self::Random(error)
    }
}

pub fn ensure_pairing_token(config: &mut Configuration) -> Result<PairingToken, AuthError> {
    if let Some(encoded) = config.browser_pairing_token.as_deref() {
        return PairingToken::from_hex(encoded).ok_or(AuthError::InvalidStoredToken);
    }

    let token = PairingToken::generate()?;
    config.browser_pairing_token = Some(token.to_hex());
    Ok(token)
}

fn decode_token(encoded: &str) -> Option<[u8; TOKEN_BYTES]> {
    let encoded = encoded.as_bytes();
    if encoded.len() != TOKEN_BYTES * 2 {
        return None;
    }

    let mut decoded = [0_u8; TOKEN_BYTES];
    for (index, pair) in encoded.chunks_exact(2).enumerate() {
        decoded[index] = decode_nibble(pair[0])? << 4 | decode_nibble(pair[1])?;
    }
    Some(decoded)
}

const fn decode_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthError, PairingToken, ensure_pairing_token};
    use crate::utils::configuration::Configuration;

    #[test]
    fn generated_token_round_trips_and_rejects_wrong_value() {
        let token = PairingToken::generate().expect("OS randomness");

        assert_eq!(token.to_hex().len(), 64);
        assert!(token.matches_hex(&token.to_hex()));
        assert!(!token.matches_hex(&"00".repeat(32)));
        assert!(!token.matches_hex("not-hex"));
    }

    #[test]
    fn ensure_pairing_token_generates_once_and_reuses_saved_value() {
        let mut config: Configuration = toml::from_str("").expect("in-memory configuration");

        let generated = ensure_pairing_token(&mut config).expect("generated token");
        assert_eq!(config.browser_pairing_token, Some(generated.to_hex()));
        assert_eq!(
            ensure_pairing_token(&mut config).expect("saved token"),
            generated
        );
    }

    #[test]
    fn ensure_pairing_token_rejects_malformed_saved_value() {
        let mut config: Configuration = toml::from_str("").expect("in-memory configuration");
        config.browser_pairing_token = Some("malformed".to_string());

        assert_eq!(
            ensure_pairing_token(&mut config),
            Err(AuthError::InvalidStoredToken)
        );
    }
}
