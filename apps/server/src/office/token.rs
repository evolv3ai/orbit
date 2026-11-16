//! Office tokens let the Document Server, which has no Orbit session, download one file. A token
//! names one version of one file: the HMAC covers the file (kind and id), which the download URL
//! names, and the token carries the blob. So a token serves no other file, and no newer or older
//! bytes of its own file. It is stateless, like a socket ticket: base64url (no padding) of
//! `blob id (36 ASCII bytes) || expiry (i64 Unix ms, big-endian) || HMAC-SHA256`, keyed by a
//! subkey of the app key.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use orbit_platform::{Id, TimestampMillis};
use sha2::Sha256;

use super::versions::OfficeFileKind;

/// How long a token stays valid after it is issued. The Document Server downloads a file once,
/// when the first person opens that version of it.
pub const TOKEN_LIFETIME_MS: i64 = 5 * 60_000;

const ID_LEN: usize = 36;
const PAYLOAD_LEN: usize = ID_LEN + 8;
const TOKEN_LEN: usize = PAYLOAD_LEN + 32;

/// Issues and verifies office tokens.
#[derive(Clone)]
pub struct OfficeTokens {
    key: [u8; 32],
}

impl OfficeTokens {
    /// Derives the token key from the server's app key (`ORBIT_APP_KEY`).
    #[must_use]
    pub fn new(app_key: &[u8; 32]) -> Self {
        let mut key = [0_u8; 32];
        Hkdf::<Sha256>::new(None, app_key)
            .expand(b"orbit office token v1", &mut key)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        Self { key }
    }

    /// A token to download the bytes `blob` of the file, that expires at `expires_at`.
    #[must_use]
    pub fn issue(
        &self,
        kind: OfficeFileKind,
        file: Id,
        blob: Id,
        expires_at: TimestampMillis,
    ) -> String {
        let mut token = Vec::with_capacity(TOKEN_LEN);
        token.extend_from_slice(blob.to_string().as_bytes());
        token.extend_from_slice(&expires_at.as_millis().to_be_bytes());
        let signature = self.mac(&token, kind, file).finalize().into_bytes();
        token.extend_from_slice(&signature);
        URL_SAFE_NO_PAD.encode(token)
    }

    /// The blob of a token this server signed for the file that has not expired at `now`.
    #[must_use]
    pub fn verify(
        &self,
        token: &str,
        kind: OfficeFileKind,
        file: Id,
        now: TimestampMillis,
    ) -> Option<Id> {
        let bytes = URL_SAFE_NO_PAD.decode(token).ok()?;
        if bytes.len() != TOKEN_LEN {
            return None;
        }
        let (payload, signature) = bytes.split_at(PAYLOAD_LEN);
        self.mac(payload, kind, file).verify_slice(signature).ok()?;
        let (blob, expires_at) = payload.split_at(ID_LEN);
        let expires_at = i64::from_be_bytes(expires_at.try_into().ok()?);
        if now.as_millis() >= expires_at {
            return None;
        }
        std::str::from_utf8(blob).ok()?.parse().ok()
    }

    fn mac(&self, payload: &[u8], kind: OfficeFileKind, file: Id) -> Hmac<Sha256> {
        let mut mac =
            <Hmac<Sha256> as Mac>::new_from_slice(&self.key).expect("HMAC accepts any key length");
        mac.update(payload);
        mac.update(kind.as_str().as_bytes());
        mac.update(b":");
        mac.update(file.to_string().as_bytes());
        mac
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use orbit_platform::{Id, TimestampMillis};

    use super::OfficeTokens;
    use crate::office::versions::OfficeFileKind;

    const NOW: TimestampMillis = TimestampMillis::from_millis(1_700_000_000_000);
    const LATER: TimestampMillis = TimestampMillis::from_millis(1_700_000_300_000);

    #[test]
    fn a_token_names_its_blob_until_it_expires() {
        let tokens = OfficeTokens::new(&[1; 32]);
        let (file, blob) = (Id::new_v7(), Id::new_v7());
        let token = tokens.issue(OfficeFileKind::Attachment, file, blob, LATER);
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        );
        assert_eq!(
            tokens.verify(&token, OfficeFileKind::Attachment, file, NOW),
            Some(blob)
        );
        assert_eq!(
            tokens.verify(&token, OfficeFileKind::Attachment, file, LATER),
            None
        );
    }

    #[test]
    fn a_token_serves_only_its_file() {
        let tokens = OfficeTokens::new(&[1; 32]);
        let file = Id::new_v7();
        let token = tokens.issue(OfficeFileKind::Attachment, file, Id::new_v7(), LATER);
        assert_eq!(
            tokens.verify(&token, OfficeFileKind::PageFile, file, NOW),
            None
        );
        assert_eq!(
            tokens.verify(&token, OfficeFileKind::Attachment, Id::new_v7(), NOW),
            None
        );
    }

    #[test]
    fn altered_or_foreign_tokens_are_refused() {
        let tokens = OfficeTokens::new(&[1; 32]);
        let file = Id::new_v7();
        let token = tokens.issue(OfficeFileKind::PageFile, file, Id::new_v7(), LATER);
        assert_eq!(
            OfficeTokens::new(&[2; 32]).verify(&token, OfficeFileKind::PageFile, file, NOW),
            None
        );

        let mut bytes = URL_SAFE_NO_PAD.decode(&token).unwrap();
        // Another blob with the old signature.
        bytes[0] ^= 1;
        assert_eq!(
            tokens.verify(
                &URL_SAFE_NO_PAD.encode(&bytes),
                OfficeFileKind::PageFile,
                file,
                NOW
            ),
            None
        );
        assert_eq!(
            tokens.verify(&format!("{token}A"), OfficeFileKind::PageFile, file, NOW),
            None
        );
        assert_eq!(tokens.verify("", OfficeFileKind::PageFile, file, NOW), None);
    }
}
