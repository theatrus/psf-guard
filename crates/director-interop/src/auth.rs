//! Bounded auth replies. Secrets deliberately implement neither Debug nor Serialize.
use crate::{astrocollab::Source, json, Error};
use serde_json::Value;

pub struct Enrollment {
    pub agent_id: String,
    pub token: String,
}
pub struct Login {
    pub code: String,
    pub url: String,
    pub expires_in: u64,
}
pub enum Poll {
    Pending,
    Done(String),
    Claimed,
    Expired,
}
pub fn valid_token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 8192 && value.bytes().all(|b| (33..=126).contains(&b))
}
fn field(value: &Value, name: &str, max: usize) -> Result<String, Error> {
    value[name]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= max && !v.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or(Error::InvalidReply)
}
pub fn enrollment(bytes: &[u8]) -> Result<Enrollment, Error> {
    let value = json::decode(bytes)?;
    let agent_id = field(&value["agent"], "id", 12)?;
    Source::new("https://validation.invalid/", &agent_id, false)?;
    let token = field(&value, "token", 8192)?;
    if !valid_token(&token) {
        return Err(Error::InvalidReply);
    }
    Ok(Enrollment { agent_id, token })
}
pub fn login(bytes: &[u8]) -> Result<Login, Error> {
    let value = json::decode(bytes)?;
    let expires_in = value["expiresIn"]
        .as_f64()
        .filter(|n| n.is_finite() && (1.0..=3600.0).contains(n))
        .ok_or(Error::InvalidReply)?
        .floor() as u64;
    Ok(Login {
        code: field(&value, "code", 512)?,
        url: field(&value, "url", 4096)?,
        expires_in,
    })
}
pub fn poll(bytes: &[u8]) -> Result<Poll, Error> {
    let value = json::decode(bytes)?;
    match value["state"].as_str() {
        Some("pending") => Ok(Poll::Pending),
        Some("claimed") => Ok(Poll::Claimed),
        Some("expired") => Ok(Poll::Expired),
        Some("done") => {
            let token = field(&value, "token", 8192)?;
            if !valid_token(&token) {
                return Err(Error::InvalidReply);
            }
            Ok(Poll::Done(token))
        }
        _ => Err(Error::InvalidReply),
    }
}
pub fn signin_available(bytes: &[u8]) -> Result<bool, Error> {
    json::decode(bytes)?["discord"]
        .as_bool()
        .ok_or(Error::InvalidReply)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replies_reject_ambiguous_secrets_and_invalid_identities() {
        assert!(enrollment(br#"{"agent":{"id":"000000000001"},"token":"opaque-token"}"#).is_ok());
        assert!(enrollment(br#"{"agent":{"id":"../foreign"},"token":"opaque"}"#).is_err());
        assert!(enrollment(br#"{"agent":{"id":"000000000001"},"token":"a","token":"b"}"#).is_err());
        assert!(poll(br#"{"state":"done","token":"bad\r\nheader"}"#).is_err());
        assert!(matches!(poll(br#"{"state":"claimed"}"#), Ok(Poll::Claimed)));
        assert!(login(br#"{"code":"code","url":"https://example.com/","expiresIn":0}"#).is_err());
    }
    #[test]
    fn login_accepts_protocol_numeric_seconds_without_extending_expiry() {
        assert_eq!(
            login(br#"{"code":"code","url":"https://example.com/","expiresIn":600.0}"#)
                .unwrap()
                .expires_in,
            600
        );
        assert_eq!(
            login(br#"{"code":"code","url":"https://example.com/","expiresIn":1.9}"#)
                .unwrap()
                .expires_in,
            1
        );
    }
}
