use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const WEBSITE_NOTICE_URL: &str = "https://updates.psf-guard.com/notice.json";
pub const GITHUB_NOTICE_URL: &str =
    "https://github.com/theatrus/psf-guard/releases/latest/download/notice.json";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ReleaseNotice {
    pub schema_version: u32,
    pub version: String,
    pub release_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub urgency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_supported_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateNoticeStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<ReleaseNotice>,
    pub checking: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at_unix_seconds: Option<u64>,
}

#[derive(Default)]
struct CacheState {
    notice: Option<ReleaseNotice>,
    last_checked: Option<Instant>,
    checked_at_unix_seconds: Option<u64>,
    checking: bool,
}

#[derive(Clone)]
pub struct UpdateNoticeManager {
    inner: Arc<Mutex<CacheState>>,
    client: reqwest::Client,
    cache_ttl: Duration,
    urls: Arc<[String]>,
}

impl Default for UpdateNoticeManager {
    fn default() -> Self {
        Self::new(
            CACHE_TTL,
            vec![WEBSITE_NOTICE_URL.to_owned(), GITHUB_NOTICE_URL.to_owned()],
        )
    }
}

impl UpdateNoticeManager {
    fn new(cache_ttl: Duration, urls: Vec<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(12))
            .user_agent(concat!("psf-guard/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("update notice HTTP client should build");
        Self {
            inner: Arc::new(Mutex::new(CacheState::default())),
            client,
            cache_ttl,
            urls: urls.into(),
        }
    }

    pub fn snapshot(&self) -> UpdateNoticeStatus {
        let state = self.inner.lock().unwrap();
        UpdateNoticeStatus {
            notice: state.notice.clone(),
            checking: state.checking,
            checked_at_unix_seconds: state.checked_at_unix_seconds,
        }
    }

    /// Start a refresh only when the process cache is stale. HTTP handlers use
    /// this as a safety net; repeated page reloads only read the fresh cache.
    pub fn refresh_in_background_if_stale(&self) {
        if !self.mark_refresh_started_if_stale() {
            return;
        }
        let manager = self.clone();
        tokio::spawn(async move {
            manager.run_marked_refresh().await;
        });
    }

    /// Refresh once at startup, then every 24 hours while the server runs.
    pub fn start_refresh_loop(&self) {
        self.refresh_in_background_if_stale();
        let manager = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(CACHE_TTL).await;
                manager.refresh_in_background_if_stale();
            }
        });
    }

    fn mark_refresh_started_if_stale(&self) -> bool {
        let mut state = self.inner.lock().unwrap();
        let fresh = state
            .last_checked
            .is_some_and(|checked| checked.elapsed() < self.cache_ttl);
        if state.checking || fresh {
            return false;
        }
        state.checking = true;
        true
    }

    async fn run_marked_refresh(&self) {
        let fetched = self.fetch_notice().await;
        let checked_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut state = self.inner.lock().unwrap();
        if let Some(notice) = fetched {
            tracing::info!(version = %notice.version, "Release notice cache refreshed");
            state.notice = Some(notice);
        } else {
            tracing::warn!("Release notice feeds returned no valid notice; keeping cached data");
        }
        state.last_checked = Some(Instant::now());
        state.checked_at_unix_seconds = Some(checked_at);
        state.checking = false;
    }

    async fn fetch_notice(&self) -> Option<ReleaseNotice> {
        let mut selected: Option<ReleaseNotice> = None;
        for url in self.urls.iter() {
            let candidate = match self.client.get(url).send().await {
                Ok(response) if response.status().is_success() => response
                    .json::<ReleaseNotice>()
                    .await
                    .ok()
                    .and_then(validate_notice),
                _ => None,
            };
            if candidate.as_ref().is_some_and(|candidate| {
                selected
                    .as_ref()
                    .is_none_or(|current| notice_version(candidate) > notice_version(current))
            }) {
                selected = candidate;
            }
        }
        selected
    }
}

fn notice_version(notice: &ReleaseNotice) -> semver::Version {
    semver::Version::parse(&notice.version).expect("validated release notice version")
}

fn validate_notice(mut notice: ReleaseNotice) -> Option<ReleaseNotice> {
    if notice.schema_version != 1 {
        return None;
    }
    notice.version = notice.version.trim().trim_start_matches('v').to_owned();
    semver::Version::parse(&notice.version).ok()?;

    let release_url = reqwest::Url::parse(&notice.release_url).ok()?;
    let safe_host = matches!(
        release_url.host_str(),
        Some("updates.psf-guard.com") | Some("github.com")
    );
    let safe_github_path = release_url.host_str() != Some("github.com")
        || release_url
            .path()
            .starts_with("/theatrus/psf-guard/releases/");
    if release_url.scheme() != "https" || !safe_host || !safe_github_path {
        return None;
    }

    notice.summary = notice
        .summary
        .map(|summary| summary.trim().chars().take(240).collect::<String>())
        .filter(|summary| !summary.is_empty());
    notice.urgency = match notice.urgency.as_str() {
        "recommended" | "required" => notice.urgency,
        _ => "normal".to_owned(),
    };
    notice.minimum_supported_version = notice
        .minimum_supported_version
        .filter(|version| semver::Version::parse(version).is_ok());
    notice.published_at = notice
        .published_at
        .filter(|published| chrono::DateTime::parse_from_rfc3339(published).is_ok());
    Some(notice)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(version: &str) -> ReleaseNotice {
        ReleaseNotice {
            schema_version: 1,
            version: version.to_owned(),
            release_url: format!("https://github.com/theatrus/psf-guard/releases/tag/v{version}"),
            summary: Some(" Release notes. ".to_owned()),
            urgency: "recommended".to_owned(),
            minimum_supported_version: Some("0.5.0".to_owned()),
            published_at: Some("2026-07-26T18:00:00Z".to_owned()),
        }
    }

    #[test]
    fn validates_and_sanitizes_public_notices() {
        let valid = validate_notice(notice("v0.6.0")).unwrap();
        assert_eq!(valid.version, "0.6.0");
        assert_eq!(valid.summary.as_deref(), Some("Release notes."));

        let mut unsafe_notice = notice("0.6.0");
        unsafe_notice.release_url = "https://example.com/download".to_owned();
        assert!(validate_notice(unsafe_notice).is_none());
    }

    #[tokio::test]
    async fn fresh_cache_suppresses_reload_refreshes() {
        let manager = UpdateNoticeManager::new(Duration::from_secs(60), vec![]);
        assert!(manager.mark_refresh_started_if_stale());
        manager.run_marked_refresh().await;
        assert!(!manager.mark_refresh_started_if_stale());
        assert!(manager.snapshot().checked_at_unix_seconds.is_some());
    }
}
