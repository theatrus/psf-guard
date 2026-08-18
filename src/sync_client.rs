//! Client side of the remote sync protocol.
//!
//! `server::remote_sync` answers `/api/sync/v1`; this speaks it. With both,
//! one PSF Guard can sync with another over the network the same way a
//! N.I.N.A. plugin would — no shared filesystem, no copying a live SQLite
//! file out from under the process writing it.
//!
//! Every direction is preview-first, because the remote holds the preview and
//! will only apply an ID it issued. A caller therefore always sees the counts
//! before anything is written, and a plain dry run is just a preview it never
//! applies.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path, time::Duration};

use crate::server::remote_sync::{CatalogBundle, SyncOperation};

/// Long enough for a large merge bundle to cross a domestic uplink, short
/// enough that an unreachable peer fails while someone is still watching.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// The envelope every PSF Guard endpoint answers in.
#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    success: bool,
    data: Option<T>,
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteCatalog {
    pub id: String,
    pub name: String,
    pub readable: bool,
    pub writable: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteCapabilities {
    pub protocol_version: u32,
    pub product: String,
    pub product_version: String,
    pub capabilities: Vec<String>,
    pub catalogs: Vec<RemoteCatalog>,
}

impl RemoteCapabilities {
    /// The one catalog this key opens. The protocol scopes a key to exactly
    /// one, so anything else means the peer changed under us.
    pub fn catalog(&self) -> Result<&RemoteCatalog> {
        match self.catalogs.as_slice() {
            [catalog] => Ok(catalog),
            [] => bail!("the peer's key opens no catalog"),
            many => bail!(
                "the peer's key opens {} catalogs; this protocol scopes a key to one",
                many.len()
            ),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemotePreview {
    pub preview_id: String,
    pub expires_at: String,
    pub summary: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteApplied {
    pub state: String,
    pub summary: BTreeMap<String, i64>,
}

#[derive(Debug, Deserialize)]
struct RemoteExport {
    bundle: Option<CatalogBundle>,
    error: Option<String>,
}

/// One authenticated PSF Guard peer.
#[derive(Debug)]
pub struct SyncClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl SyncClient {
    pub fn new(base_url: &str, token: &str) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/').to_string();
        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            bail!("peer URL must start with http:// or https:// (got {base_url})");
        }
        if base_url.starts_with("http://")
            && !base_url.starts_with("http://127.0.0.1")
            && !base_url.starts_with("http://localhost")
        {
            // The key travels in a header on every request. Over plain HTTP it
            // travels in the clear, so say so rather than let it happen
            // quietly.
            tracing::warn!(
                "syncing with {base_url} over plain HTTP — the API key is sent \
                 unencrypted. Use https:// for anything off this machine."
            );
        }
        Ok(Self {
            base_url,
            token: token.trim().to_string(),
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .context("building the sync HTTP client")?,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn capabilities(&self) -> Result<RemoteCapabilities> {
        self.get("/api/sync/v1/capabilities")
            .await
            .with_context(|| format!("asking {} what it supports", self.base_url))
    }

    /// Ask the peer to build a bundle of its own catalog.
    pub async fn export(
        &self,
        catalog_id: &str,
        operation: SyncOperation,
        reviewed_only: bool,
    ) -> Result<CatalogBundle> {
        let export: RemoteExport = self
            .post(
                "/api/sync/v1/exports",
                &serde_json::json!({
                    "protocol_version": 1,
                    "catalog_id": catalog_id,
                    "operation": operation,
                    "reviewed_only": reviewed_only,
                }),
            )
            .await
            .with_context(|| format!("asking {} for a bundle", self.base_url))?;
        match export.bundle {
            Some(bundle) => Ok(bundle),
            None => bail!(
                "{} could not build a bundle: {}",
                self.base_url,
                export.error.unwrap_or_else(|| "no reason given".into())
            ),
        }
    }

    /// Send a bundle for the peer to review. Writes nothing there.
    pub async fn preview(
        &self,
        catalog_id: &str,
        operation: SyncOperation,
        bundle: &CatalogBundle,
    ) -> Result<RemotePreview> {
        self.post(
            "/api/sync/v1/previews",
            &serde_json::json!({
                "protocol_version": 1,
                "catalog_id": catalog_id,
                "operation": operation,
                "bundle": bundle,
            }),
        )
        .await
        .with_context(|| format!("sending a bundle to {} for review", self.base_url))
    }

    /// Commit a preview the peer is holding.
    pub async fn apply(&self, preview_id: &str) -> Result<RemoteApplied> {
        self.post_empty(&format!("/api/sync/v1/previews/{preview_id}/apply"))
            .await
            .with_context(|| format!("applying preview {preview_id} on {}", self.base_url))
    }

    /// Re-review a kept preview against the peer as it now stands. The way
    /// back from a stale-preview refusal without re-sending the bundle.
    pub async fn refresh(&self, preview_id: &str) -> Result<RemotePreview> {
        self.post_empty(&format!("/api/sync/v1/previews/{preview_id}/refresh"))
            .await
            .with_context(|| format!("refreshing preview {preview_id} on {}", self.base_url))
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .http
            .get(format!("{}{path}", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await?;
        Self::unwrap_envelope(response).await
    }

    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        let response = self
            .http
            .post(format!("{}{path}", self.base_url))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await?;
        Self::unwrap_envelope(response).await
    }

    async fn post_empty<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .http
            .post(format!("{}{path}", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await?;
        Self::unwrap_envelope(response).await
    }

    /// Turn one response into either the payload or an error worth reading.
    ///
    /// The peer's own message matters more than the status line here: it is
    /// the half of the exchange the operator cannot see.
    async fn unwrap_envelope<T: serde::de::DeserializeOwned>(
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let envelope: ApiEnvelope<T> = match serde_json::from_str(&body) {
            Ok(envelope) => envelope,
            Err(error) => {
                bail!(
                    "peer returned {status} that is not a PSF Guard response \
                     ({error}). Check the URL points at a PSF Guard server: {}",
                    body.chars().take(200).collect::<String>()
                )
            }
        };
        if let Some(data) = envelope.data.filter(|_| envelope.success) {
            return Ok(data);
        }
        let detail = envelope
            .error
            .unwrap_or_else(|| "no reason given".to_string());
        match status.as_u16() {
            401 | 403 => bail!("peer refused the key: {detail}"),
            409 => bail!("{detail}"),
            _ => bail!("peer returned {status}: {detail}"),
        }
    }
}

/// Build a bundle from a local scheduler database, to send to a peer.
pub fn local_bundle(
    database_path: &Path,
    catalog_id: &str,
    operation: SyncOperation,
    reviewed_only: bool,
    include_thumbnails: bool,
) -> Result<CatalogBundle> {
    crate::server::remote_sync::export_bundle(
        database_path,
        catalog_id,
        operation,
        reviewed_only,
        include_thumbnails,
    )
}

/// Write a bundle received from a peer into a throwaway SQLite source the
/// local sync engine can read, borrowing DDL from `template_path` for any
/// table the peer left out.
pub fn materialize(bundle: &CatalogBundle, into: &Path, template_path: &Path) -> Result<()> {
    crate::server::remote_sync::materialize_bundle(into, template_path, bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_url_must_name_a_scheme() {
        let error = SyncClient::new("telescope.local:3000", "a-token-long-enough-for-this")
            .unwrap_err()
            .to_string();
        assert!(error.contains("http://"), "{error}");
    }

    #[test]
    fn a_trailing_slash_does_not_double_up_in_paths() {
        let client =
            SyncClient::new("https://scope.example/", "a-token-long-enough-for-this").unwrap();
        assert_eq!(client.base_url(), "https://scope.example");
    }

    #[test]
    fn one_catalog_per_key_is_the_contract() {
        let capabilities = |count: usize| RemoteCapabilities {
            protocol_version: 1,
            product: "PSF Guard".into(),
            product_version: "0.5.0".into(),
            capabilities: vec![],
            catalogs: (0..count)
                .map(|index| RemoteCatalog {
                    id: format!("catalog-{index}"),
                    name: "Catalog".into(),
                    readable: true,
                    writable: true,
                })
                .collect(),
        };

        assert_eq!(capabilities(1).catalog().unwrap().id, "catalog-0");
        assert!(capabilities(0).catalog().is_err());
        assert!(capabilities(2).catalog().is_err());
    }
}
