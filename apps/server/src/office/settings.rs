//! The office Document Server's settings (spec section 4.1): the root user saves them in Admin → Office editing
//! (`office_settings`, one row), or `ORBIT_ONLYOFFICE_*` variables set them. Set variables replace the saved
//! values on every read and are not saved, like the other instance settings
//! (`repositories/environment_settings.rs`).
//!
//! The settings also decide the content security policy: while the integration is on, pages may load the
//! editor from the Document Server's origin ([`CspOfficeOrigin`]). [`reload`] keeps the two in step.

use std::sync::Arc;

use orbit_platform::{CspOfficeOrigin, Database, Id, InstanceConfig, Row, TimestampMillis};
use thiserror::Error;

use crate::audit::{self, AuditOutcome};
use crate::secret_box::{decrypt_secret, encrypt_secret};

/// The longest URL and the longest secret that a save takes.
pub const MAX_URL_BYTES: usize = 2048;
pub const MAX_SECRET_BYTES: usize = 512;

/// A Document Server that Orbit can use. `jwt_secret` is encrypted under the app key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfficeSettings {
    /// Off: the settings are kept, and Orbit behaves as without a Document Server.
    pub enabled: bool,
    /// The Document Server as browsers reach it. Without a trailing `/`.
    pub url: String,
    /// The Document Server as Orbit reaches it, when that is another address than `url`.
    pub internal_url: Option<String>,
    /// Orbit as the Document Server reaches it, when that is another address than Orbit's public origin.
    pub orbit_url: Option<String>,
    pub jwt_secret: Vec<u8>,
}

impl OfficeSettings {
    /// Where Orbit sends its own requests to the Document Server (the health check, saved files).
    #[must_use]
    pub fn server_url(&self) -> &str {
        self.internal_url.as_deref().unwrap_or(&self.url)
    }

    /// The shared secret in plain text, to sign and verify the requests of the Document Server. `None` when
    /// `app_key` is not the key that encrypted it.
    pub fn jwt_secret(&self, app_key: &[u8; 32]) -> Option<String> {
        decrypt_secret(app_key, &self.jwt_secret).ok()
    }

    /// `scheme://host[:port]` of the public URL, as the content security policy names it.
    #[must_use]
    pub fn csp_origin(&self) -> Option<String> {
        csp_origin(&self.url)
    }
}

/// The origin of `url` when the content security policy can name it.
fn csp_origin(url: &str) -> Option<String> {
    let origin = url::Url::parse(url).ok()?.origin().ascii_serialization();
    CspOfficeOrigin::default()
        .set(&origin)
        .is_ok()
        .then_some(origin)
}

/// Why a URL is not usable, as a sentence for the person who typed it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidUrl(pub &'static str);

/// A URL of the settings: `http(s)`, with a host, without credentials, query or fragment. Returned without a
/// trailing `/`.
pub fn clean_url(value: &str) -> Result<String, InvalidUrl> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() || value.len() > MAX_URL_BYTES {
        return Err(InvalidUrl(
            "enter an http(s) URL of at most 2048 characters.",
        ));
    }
    let valid = url::Url::parse(value).is_ok_and(|url| {
        matches!(url.scheme(), "https" | "http")
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    });
    if valid {
        Ok(value.to_owned())
    } else {
        Err(InvalidUrl(
            "enter an http(s) URL without a user name, a query or a fragment.",
        ))
    }
}

/// The public URL: [`clean_url`], and an origin that the content security policy can name (a host name or an
/// IPv4 address).
pub fn clean_public_url(value: &str) -> Result<String, InvalidUrl> {
    let url = clean_url(value)?;
    if csp_origin(&url).is_none() {
        return Err(InvalidUrl(
            "use a host name or an IPv4 address, so browsers can be allowed to load the editor from it.",
        ));
    }
    Ok(url)
}

/// The settings from `ORBIT_ONLYOFFICE_*`, with the secret encrypted under the app key like a saved one.
#[derive(Debug, Default)]
pub struct EnvironmentOffice(Option<OfficeSettings>);

impl EnvironmentOffice {
    pub fn from_config(config: &InstanceConfig, app_key: &[u8; 32]) -> Result<Self, String> {
        let Some(office) = &config.onlyoffice else {
            return Ok(Self(None));
        };
        if csp_origin(&office.url).is_none() {
            return Err(
                "ORBIT_ONLYOFFICE_URL must have a host name or an IPv4 address as its host"
                    .to_owned(),
            );
        }
        Ok(Self(Some(OfficeSettings {
            enabled: office.enabled,
            url: office.url.clone(),
            internal_url: office.internal_url.clone(),
            orbit_url: office.orbit_url.clone(),
            jwt_secret: encrypt_secret(app_key, office.jwt_secret.expose())
                .map_err(|()| "a secret could not be encrypted".to_owned())?,
        })))
    }

    /// From now on, every read of the office settings of `database` uses these settings.
    pub fn install(self, database: &Database) {
        database.extension(|| self);
    }

    /// The settings of `database`; none when [`EnvironmentOffice::install`] did not run.
    #[must_use]
    pub fn of(database: &Database) -> Arc<Self> {
        database.existing_extension::<Self>().unwrap_or_default()
    }

    #[must_use]
    pub fn settings(&self) -> Option<&OfficeSettings> {
        self.0.as_ref()
    }
}

/// What a save does with the stored secret.
pub enum SecretChange {
    Keep,
    /// Already encrypted.
    Set(Vec<u8>),
}

/// The values of a save, already checked ([`clean_public_url`], [`clean_url`]).
pub struct SavedOffice {
    pub enabled: bool,
    pub url: String,
    pub internal_url: Option<String>,
    pub orbit_url: Option<String>,
}

#[derive(Debug, Error)]
pub enum OfficeSettingsError {
    #[error("the first save needs the secret")]
    SecretRequired,
    #[error("office settings are unavailable")]
    Unavailable(#[from] sqlx::Error),
}

#[derive(Clone)]
pub struct OfficeSettingsRepository {
    database: Database,
}

impl OfficeSettingsRepository {
    #[must_use]
    pub fn new(database: Database) -> Self {
        Self { database }
    }

    /// The settings in use: from the environment, otherwise the saved ones. `None` until a URL and a secret
    /// are known.
    pub async fn get(&self) -> Result<Option<OfficeSettings>, sqlx::Error> {
        if let Some(settings) = EnvironmentOffice::of(&self.database).settings() {
            return Ok(Some(settings.clone()));
        }
        let row = sqlx::query(
            "SELECT enabled, url, internal_url, orbit_url, jwt_secret FROM office_settings WHERE id = 1",
        )
        .fetch_one(self.database.pool())
        .await?;
        Ok(
            match (
                row.get::<Option<String>, _>("url"),
                row.get::<Option<Vec<u8>>, _>("jwt_secret"),
            ) {
                (Some(url), Some(jwt_secret)) => Some(OfficeSettings {
                    enabled: row.get::<i64, _>("enabled") == 1,
                    url,
                    internal_url: row.get("internal_url"),
                    orbit_url: row.get("orbit_url"),
                    jwt_secret,
                }),
                _ => None,
            },
        )
    }

    pub async fn save(
        &self,
        actor_id: Id,
        office: SavedOffice,
        secret: SecretChange,
        request_id: &str,
        now: TimestampMillis,
    ) -> Result<(), OfficeSettingsError> {
        let mut transaction = self.database.immediate_transaction().await?;
        let jwt_secret = match secret {
            SecretChange::Set(sealed) => sealed,
            SecretChange::Keep => sqlx::query_scalar::<_, Option<Vec<u8>>>(
                "SELECT jwt_secret FROM office_settings WHERE id = 1",
            )
            .fetch_one(&mut *transaction)
            .await?
            .ok_or(OfficeSettingsError::SecretRequired)?,
        };
        sqlx::query(
            "UPDATE office_settings SET enabled = $1, url = $2, internal_url = $3, orbit_url = $4, \
             jwt_secret = $5, updated_at = $6 WHERE id = 1",
        )
        .bind(i64::from(office.enabled))
        .bind(&office.url)
        .bind(&office.internal_url)
        .bind(&office.orbit_url)
        .bind(jwt_secret)
        .bind(now.as_millis())
        .execute(&mut *transaction)
        .await?;
        audit::record_global(
            &mut transaction,
            Some(actor_id),
            "instance.office_updated",
            AuditOutcome::Success,
            "installation",
            None,
            request_id,
            serde_json::json!({"enabled": office.enabled, "url": office.url}),
            now,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Removes the saved Document Server and its secret.
    pub async fn clear(
        &self,
        actor_id: Id,
        request_id: &str,
        now: TimestampMillis,
    ) -> Result<(), sqlx::Error> {
        let mut transaction = self.database.immediate_transaction().await?;
        sqlx::query(
            "UPDATE office_settings SET enabled = 0, url = NULL, internal_url = NULL, orbit_url = NULL, \
             jwt_secret = NULL, updated_at = $1 WHERE id = 1",
        )
        .bind(now.as_millis())
        .execute(&mut *transaction)
        .await?;
        audit::record_global(
            &mut transaction,
            Some(actor_id),
            "instance.office_removed",
            AuditOutcome::Success,
            "installation",
            None,
            request_id,
            serde_json::json!({}),
            now,
        )
        .await?;
        transaction.commit().await
    }
}

/// The handle that the HTTP layer of `database`'s server reads for the content security policy.
#[must_use]
pub fn csp_handle(database: &Database) -> CspOfficeOrigin {
    (*database.extension(CspOfficeOrigin::default)).clone()
}

/// Makes the content security policy follow the settings in use: the Document Server's origin while the
/// integration is on, no extra source otherwise. Runs when the server starts and after every change, also one
/// made on another server of the database.
pub async fn reload(database: &Database) -> Result<(), sqlx::Error> {
    let settings = OfficeSettingsRepository::new(database.clone())
        .get()
        .await?;
    let handle = csp_handle(database);
    match settings
        .filter(|settings| settings.enabled)
        .and_then(|settings| settings.csp_origin())
    {
        Some(origin) => {
            // `csp_origin` returns only origins that `set` takes.
            let _ = handle.set(&origin);
        }
        None => handle.clear(),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_lose_the_trailing_slash_and_keep_a_path() {
        assert_eq!(
            clean_url(" https://office.example.com/ ").unwrap(),
            "https://office.example.com"
        );
        assert_eq!(
            clean_url("http://documentserver:8000/ds/").unwrap(),
            "http://documentserver:8000/ds"
        );
        for invalid in [
            "",
            "office.example.com",
            "ftp://office.example.com",
            "https://user:secret@office.example.com",
            "https://office.example.com/?a=1",
            "https://office.example.com/#top",
        ] {
            assert!(clean_url(invalid).is_err(), "{invalid}");
        }
        assert!(
            clean_url(&format!(
                "https://example.com/{}",
                "a".repeat(MAX_URL_BYTES)
            ))
            .is_err()
        );
    }

    #[test]
    fn the_policy_names_the_origin_of_the_public_url() {
        let settings = |url: &str| OfficeSettings {
            enabled: true,
            url: url.to_owned(),
            internal_url: None,
            orbit_url: None,
            jwt_secret: Vec::new(),
        };
        assert_eq!(
            settings("https://orbit.example.com/office")
                .csp_origin()
                .as_deref(),
            Some("https://orbit.example.com")
        );
        assert_eq!(
            settings("http://192.168.1.20:8000").csp_origin().as_deref(),
            Some("http://192.168.1.20:8000")
        );
        // The policy cannot name an IPv6 host, so such a URL is refused when it is saved.
        assert!(clean_public_url("http://[::1]:8000").is_err());
        assert!(clean_public_url("https://office.example.com").is_ok());
    }

    #[test]
    fn orbit_uses_the_internal_url_when_there_is_one() {
        let mut settings = OfficeSettings {
            enabled: true,
            url: "https://office.example.com".to_owned(),
            internal_url: None,
            orbit_url: None,
            jwt_secret: Vec::new(),
        };
        assert_eq!(settings.server_url(), "https://office.example.com");
        settings.internal_url = Some("http://documentserver".to_owned());
        assert_eq!(settings.server_url(), "http://documentserver");
    }
}
