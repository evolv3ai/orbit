//! The JSON Web Tokens that Orbit and the Document Server sign for each other with the shared
//! secret (`JWT_SECRET` of the Document Server). Only HS256, the Document Server's default.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use orbit_platform::TimestampMillis;
use serde_json::Value;
use sha2::Sha256;

const HEADER: &str = r#"{"alg":"HS256","typ":"JWT"}"#;

/// `payload` as a token signed with `secret`.
#[must_use]
pub fn sign(payload: &Value, secret: &str) -> String {
    let mut token = URL_SAFE_NO_PAD.encode(HEADER);
    token.push('.');
    token.push_str(&URL_SAFE_NO_PAD.encode(payload.to_string()));
    let signature = mac(secret, &token).finalize().into_bytes();
    token.push('.');
    token.push_str(&URL_SAFE_NO_PAD.encode(signature));
    token
}

/// The payload of a token that `secret` signed with HS256 and that has not expired at `now`
/// (`exp`, when the payload has one). `None` for anything else.
#[must_use]
pub fn verify(token: &str, secret: &str, now: TimestampMillis) -> Option<Value> {
    let (signed, signature) = token.rsplit_once('.')?;
    let (header, payload) = signed.split_once('.')?;
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).ok()?).ok()?;
    // The algorithm is fixed: a token cannot choose a weaker one, or none.
    if header.get("alg").and_then(Value::as_str) != Some("HS256") {
        return None;
    }
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    mac(secret, signed).verify_slice(&signature).ok()?;
    let payload: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    if !payload.is_object() {
        return None;
    }
    if let Some(expires) = payload.get("exp") {
        let expires = expires.as_f64()?;
        #[allow(clippy::cast_precision_loss)]
        let now_seconds = now.as_millis() as f64 / 1000.0;
        if now_seconds >= expires {
            return None;
        }
    }
    Some(payload)
}

fn mac(secret: &str, signed: &str) -> Hmac<Sha256> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(signed.as_bytes());
    mac
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use orbit_platform::TimestampMillis;
    use serde_json::json;

    use super::{sign, verify};

    const NOW: TimestampMillis = TimestampMillis::from_millis(1_700_000_000_000);

    #[test]
    fn signs_the_token_that_other_libraries_sign() {
        // The expected token was computed with Node's `crypto` (HMAC-SHA256, base64url).
        let token = sign(&json!({"key": "abc"}), "secret");
        assert_eq!(
            token,
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJrZXkiOiJhYmMifQ.-QoCLltEcdTw0d-JvAP6crF_CKp_Mj7ufxrMwNcPihI"
        );
        assert_eq!(verify(&token, "secret", NOW), Some(json!({"key": "abc"})));
    }

    #[test]
    fn refuses_another_secret_an_altered_payload_and_another_algorithm() {
        let token = sign(&json!({"status": 2}), "secret");
        assert_eq!(verify(&token, "other", NOW), None);

        let (signed, signature) = token.rsplit_once('.').unwrap();
        let (header, _) = signed.split_once('.').unwrap();
        let payload = URL_SAFE_NO_PAD.encode(r#"{"status":6}"#);
        assert_eq!(
            verify(&format!("{header}.{payload}.{signature}"), "secret", NOW),
            None
        );

        let none = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let (_, payload) = signed.split_once('.').unwrap();
        assert_eq!(verify(&format!("{none}.{payload}."), "secret", NOW), None);
        assert_eq!(verify("", "secret", NOW), None);
        assert_eq!(verify("a.b", "secret", NOW), None);
    }

    #[test]
    fn refuses_an_expired_token() {
        let seconds = NOW.as_millis() / 1000;
        let live = sign(&json!({"exp": seconds + 60}), "secret");
        assert!(verify(&live, "secret", NOW).is_some());
        let expired = sign(&json!({"exp": seconds}), "secret");
        assert_eq!(verify(&expired, "secret", NOW), None);
    }
}
