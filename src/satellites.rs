//! Provenance-bearing satellite-track predictions for solved single exposures.
//!
//! Orbital predictions and constrained pixel-path alignments are persisted as
//! separate evidence. They only become grading evidence after the caller has
//! explicitly populated the orbital-element cache (or configured a local
//! OMM/TLE file).

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use seiza_satellites::{
    CacheState, CelesTrakSource, ExposureProvenance, ObserverLocation, SatelliteCatalog,
    SingleExposure, TrackOptions, UtcTimestamp,
};
use serde::{Deserialize, Serialize};

use crate::astrometry::{
    wcs_from_response, AstrometryAnalysis, AstrometrySourceFingerprint, WcsResponse,
};
use crate::astrometry_headers::FitsAstrometryHeaders;
use crate::trail_alignment::{PixelTrailAligner, PixelTrailAlignment, PIXEL_ALIGNMENT_VERSION};
use crate::FitsImage;

pub const SEIZA_SATELLITES_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SatelliteCatalogState {
    Configured,
    Fresh,
    Downloaded,
    StaleFallback,
    Cached,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SatelliteCatalogProvenance {
    pub source: String,
    pub state: SatelliteCatalogState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_unix_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieved_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Clone)]
pub struct SatelliteCatalogSnapshot {
    catalog: Arc<SatelliteCatalog>,
    pub provenance: SatelliteCatalogProvenance,
}

/// Shared orbital-element source. The network is touched only by
/// [`refresh_active`](Self::refresh_active), which is called by the explicit
/// on-demand endpoint. Sequence grading uses [`cached_only`](Self::cached_only).
pub struct SatelliteContext {
    source: CelesTrakSource,
    configured_elements: Option<PathBuf>,
    loaded: RwLock<Option<SatelliteCatalogSnapshot>>,
    refresh_mutex: tokio::sync::Mutex<()>,
}

impl SatelliteContext {
    pub fn new(cache_dir: PathBuf, configured_elements: Option<PathBuf>) -> Result<Self, String> {
        let source = CelesTrakSource::new(cache_dir).map_err(|error| error.to_string())?;
        Ok(Self {
            source,
            configured_elements,
            loaded: RwLock::new(None),
            refresh_mutex: tokio::sync::Mutex::new(()),
        })
    }

    pub fn cache_dir(&self) -> &Path {
        self.source.cache_dir()
    }

    pub fn has_cached_elements(&self) -> bool {
        self.configured_elements
            .as_deref()
            .is_some_and(Path::is_file)
            || self.loaded.read().unwrap().is_some()
            || std::fs::read_dir(self.source.cache_dir()).is_ok_and(|entries| {
                entries.filter_map(Result::ok).any(|entry| {
                    entry.path().extension().and_then(|value| value.to_str()) == Some("json")
                })
            })
    }

    /// Load a configured historical file, or refresh the shared CelesTrak
    /// active-satellite snapshot. This is the only network-capable path.
    pub async fn refresh_active(&self) -> Result<SatelliteCatalogSnapshot, String> {
        let _guard = self.refresh_mutex.lock().await;
        if let Some(path) = self.configured_elements.as_deref() {
            let snapshot = load_local_catalog(path, SatelliteCatalogState::Configured, None)?;
            *self.loaded.write().unwrap() = Some(snapshot.clone());
            return Ok(snapshot);
        }

        let load = self
            .source
            .load_active()
            .await
            .map_err(|error| error.to_string())?;
        let state = match load.state {
            CacheState::Fresh => SatelliteCatalogState::Fresh,
            CacheState::Downloaded => SatelliteCatalogState::Downloaded,
            CacheState::StaleFallback => SatelliteCatalogState::StaleFallback,
        };
        let (size_bytes, modified_unix_seconds) = file_identity(&load.cache_path);
        let snapshot = SatelliteCatalogSnapshot {
            provenance: SatelliteCatalogProvenance {
                source: load.catalog.source().to_string(),
                state,
                cache_path: Some(load.cache_path.display().to_string()),
                size_bytes,
                modified_unix_seconds,
                retrieved_at: load.catalog.retrieved_at().map(UtcTimestamp::to_rfc3339),
                warning: load.warning,
            },
            catalog: Arc::new(load.catalog),
        };
        *self.loaded.write().unwrap() = Some(snapshot.clone());
        Ok(snapshot)
    }

    /// Return orbital elements without downloading. A configured file wins;
    /// otherwise reuse memory or the newest valid JSON snapshot in the shared
    /// CelesTrak cache directory.
    pub fn cached_only(&self) -> Result<Option<SatelliteCatalogSnapshot>, String> {
        if let Some(path) = self.configured_elements.as_deref() {
            let snapshot = load_local_catalog(path, SatelliteCatalogState::Configured, None)?;
            *self.loaded.write().unwrap() = Some(snapshot.clone());
            return Ok(Some(snapshot));
        }
        if let Some(snapshot) = self.loaded.read().unwrap().clone() {
            return Ok(Some(snapshot));
        }

        let mut candidates = std::fs::read_dir(self.source.cache_dir())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry.path().extension().and_then(|value| value.to_str()) == Some("json")
                    })
                    .filter_map(|entry| {
                        let modified = entry.metadata().ok()?.modified().ok()?;
                        Some((modified, entry.path()))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
        for (modified, path) in candidates {
            if let Ok(snapshot) =
                load_local_catalog(&path, SatelliteCatalogState::Cached, Some(modified))
            {
                *self.loaded.write().unwrap() = Some(snapshot.clone());
                return Ok(Some(snapshot));
            }
        }
        Ok(None)
    }
}

fn load_local_catalog(
    path: &Path,
    state: SatelliteCatalogState,
    known_modified: Option<SystemTime>,
) -> Result<SatelliteCatalogSnapshot, String> {
    let modified = known_modified.or_else(|| std::fs::metadata(path).ok()?.modified().ok());
    let retrieved_at = modified
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| UtcTimestamp::from_unix_seconds(duration.as_secs_f64()).ok());
    let mut catalog = SatelliteCatalog::open(path).map_err(|error| error.to_string())?;
    if let Some(retrieved_at) = retrieved_at {
        catalog = catalog.with_retrieved_at(retrieved_at);
    }
    let (size_bytes, modified_unix_seconds) = file_identity(path);
    Ok(SatelliteCatalogSnapshot {
        provenance: SatelliteCatalogProvenance {
            source: path.display().to_string(),
            state,
            cache_path: Some(path.display().to_string()),
            size_bytes,
            modified_unix_seconds,
            retrieved_at: retrieved_at.map(UtcTimestamp::to_rfc3339),
            warning: None,
        },
        catalog: Arc::new(catalog),
    })
}

fn file_identity(path: &Path) -> (Option<u64>, Option<u64>) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return (None, None);
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    (Some(metadata.len()), modified)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SatelliteExposureContext {
    pub start_utc: String,
    pub end_utc: String,
    pub duration_seconds: f64,
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub altitude_m: f64,
    pub provenance: String,
    pub header_keywords: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrightTrailRiskLevel {
    Low,
    Possible,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SatelliteTrackPrediction {
    pub name: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub norad_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cospar_id: Option<String>,
    pub association: String,
    pub element_epoch_utc: String,
    pub element_age_seconds: f64,
    pub sample_interval_seconds: f64,
    pub clipped_segments: Vec<[[f64; 2]; 2]>,
    pub clipped_length_px: f64,
    pub maximum_elevation_deg: f64,
    pub minimum_range_km: f64,
    pub maximum_sunlight_fraction: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_apparent_rate_arcsec_per_second: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_pixel_rate_px_per_second: Option<f64>,
    /// Heuristic 0..1 chance of a visible trail. This is deliberately not an
    /// apparent magnitude and does not claim a pixel detection.
    pub bright_trail_risk: f64,
    pub risk_level: BrightTrailRiskLevel,
    /// Pixel evidence fitted inside a bounded corridor around this orbital
    /// prediction. The predicted segments above remain unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_alignment: Option<PixelTrailAlignment>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SatelliteRiskSummary {
    pub track_count: usize,
    pub potentially_bright_count: usize,
    pub high_risk_count: usize,
    pub maximum_bright_trail_risk: f64,
    #[serde(default)]
    pub pixel_alignment_attempted: bool,
    #[serde(default)]
    pub pixel_aligned_count: usize,
    #[serde(default)]
    pub pixel_aligned_high_risk_count: usize,
    pub reject_recommended: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SatelliteAnalysis {
    pub image_id: i32,
    pub association: String,
    pub seiza_version: String,
    pub seiza_satellites_version: String,
    #[serde(default)]
    pub pixel_alignment_version: u32,
    pub source_fingerprint: AstrometrySourceFingerprint,
    /// Exact WCS used for projection; also invalidates predictions after a
    /// re-solve even when the FITS file itself did not change.
    pub astrometry_wcs: WcsResponse,
    pub image_width: u32,
    pub image_height: u32,
    pub exposure: SatelliteExposureContext,
    pub catalog: SatelliteCatalogProvenance,
    pub elements_considered: usize,
    pub propagation_failures: usize,
    pub stale_elements: usize,
    pub tracks: Vec<SatelliteTrackPrediction>,
    pub risk: SatelliteRiskSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_alignment_error: Option<String>,
    pub computed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatelliteAnalysisStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis: Option<SatelliteAnalysis>,
    pub orbital_elements_cached: bool,
}

pub fn predict_tracks(
    image_id: i32,
    path: &Path,
    astrometry: &AstrometryAnalysis,
    snapshot: &SatelliteCatalogSnapshot,
) -> Result<SatelliteAnalysis, String> {
    let solution = astrometry
        .solution
        .as_ref()
        .ok_or_else(|| "a solved pixel WCS is required for satellite tracks".to_string())?;
    let headers = FitsAstrometryHeaders::from_path(path)
        .map_err(|error| format!("failed to read FITS exposure headers: {error}"))?;
    let (exposure, exposure_context) = single_exposure(&headers)?;
    let wcs = wcs_from_response(&solution.wcs);
    let result = snapshot
        .catalog
        .tracks_in_footprint(
            &wcs,
            (solution.image_width, solution.image_height),
            &exposure,
            &TrackOptions::default(),
        )
        .map_err(|error| error.to_string())?;

    let mut tracks = result
        .tracks
        .into_iter()
        .map(|track| {
            let clipped_length_px = track.clipped_length_px();
            let maximum_elevation_deg = track.maximum_elevation_deg();
            let maximum_sunlight_fraction = track.maximum_sunlight_fraction();
            let minimum_range_km = track
                .samples
                .iter()
                .map(|sample| sample.range_km)
                .fold(f64::INFINITY, f64::min);
            let bright_trail_risk = bright_trail_risk(
                maximum_sunlight_fraction,
                minimum_range_km,
                maximum_elevation_deg,
                clipped_length_px,
            );
            let risk_level = if bright_trail_risk >= 0.55
                && maximum_sunlight_fraction >= 0.5
                && minimum_range_km <= 4_000.0
                && clipped_length_px >= 10.0
            {
                BrightTrailRiskLevel::High
            } else if maximum_sunlight_fraction >= 0.2
                && minimum_range_km <= 10_000.0
                && clipped_length_px >= 2.0
            {
                BrightTrailRiskLevel::Possible
            } else {
                BrightTrailRiskLevel::Low
            };
            SatelliteTrackPrediction {
                name: track.identity.name.clone(),
                label: track.identity.display_label(),
                norad_id: track.identity.norad_id,
                cospar_id: track.identity.cospar_id.clone(),
                association: "predicted".to_string(),
                element_epoch_utc: track.element_epoch_utc.to_rfc3339(),
                element_age_seconds: track.element_age_seconds,
                sample_interval_seconds: track.sample_interval_seconds,
                clipped_segments: track
                    .clipped_segments
                    .iter()
                    .map(|segment| {
                        [
                            [segment.start.x, segment.start.y],
                            [segment.end.x, segment.end.y],
                        ]
                    })
                    .collect(),
                clipped_length_px,
                maximum_elevation_deg,
                minimum_range_km,
                maximum_sunlight_fraction,
                maximum_apparent_rate_arcsec_per_second: track
                    .maximum_apparent_rate_arcsec_per_second(),
                maximum_pixel_rate_px_per_second: track.maximum_pixel_rate_px_per_second(),
                bright_trail_risk,
                risk_level,
                pixel_alignment: None,
            }
        })
        .collect::<Vec<_>>();
    let (pixel_alignment_attempted, pixel_alignment_error) = match FitsImage::from_file(path) {
        Ok(image) => {
            let aligner = PixelTrailAligner::new(&image);
            for track in &mut tracks {
                track.pixel_alignment = Some(aligner.align_track(&track.clipped_segments));
            }
            (true, None)
        }
        Err(error) => (
            false,
            Some(format!(
                "failed to load FITS pixels for trail alignment: {error}"
            )),
        ),
    };
    let high_risk_count = tracks
        .iter()
        .filter(|track| track.risk_level == BrightTrailRiskLevel::High)
        .count();
    let potentially_bright_count = tracks
        .iter()
        .filter(|track| track.risk_level != BrightTrailRiskLevel::Low)
        .count();
    let maximum_bright_trail_risk = tracks
        .iter()
        .map(|track| track.bright_trail_risk)
        .fold(0.0, f64::max);
    let pixel_aligned_count = tracks
        .iter()
        .filter(|track| {
            track
                .pixel_alignment
                .as_ref()
                .is_some_and(PixelTrailAlignment::detected)
        })
        .count();
    let pixel_aligned_high_risk_count = tracks
        .iter()
        .filter(|track| {
            track.risk_level == BrightTrailRiskLevel::High
                && track
                    .pixel_alignment
                    .as_ref()
                    .is_some_and(PixelTrailAlignment::detected)
        })
        .count();
    let association = if pixel_aligned_count > 0 {
        "predicted_with_pixel_alignment"
    } else if pixel_alignment_attempted {
        "predicted_pixel_checked"
    } else {
        "predicted_not_pixel_detected"
    };

    Ok(SatelliteAnalysis {
        image_id,
        association: association.to_string(),
        seiza_version: crate::astrometry::SEIZA_VERSION.to_string(),
        seiza_satellites_version: SEIZA_SATELLITES_VERSION.to_string(),
        pixel_alignment_version: PIXEL_ALIGNMENT_VERSION,
        source_fingerprint: astrometry.source_fingerprint.clone(),
        astrometry_wcs: solution.wcs.clone(),
        image_width: solution.image_width,
        image_height: solution.image_height,
        exposure: exposure_context,
        catalog: snapshot.provenance.clone(),
        elements_considered: result.elements_considered,
        propagation_failures: result.propagation_failures,
        stale_elements: result.stale_elements,
        risk: SatelliteRiskSummary {
            track_count: tracks.len(),
            potentially_bright_count,
            high_risk_count,
            maximum_bright_trail_risk,
            pixel_alignment_attempted,
            pixel_aligned_count,
            pixel_aligned_high_risk_count,
            reject_recommended: pixel_aligned_high_risk_count > 0,
        },
        tracks,
        pixel_alignment_error,
        computed_at: unix_now(),
    })
}

/// Validate the timing/site inputs before a caller performs an expensive
/// plate solve or an explicit orbital-element refresh.
pub fn validate_exposure(path: &Path) -> Result<(), String> {
    let headers = FitsAstrometryHeaders::from_path(path)
        .map_err(|error| format!("failed to read FITS exposure headers: {error}"))?;
    single_exposure(&headers).map(|_| ())
}

fn single_exposure(
    headers: &FitsAstrometryHeaders,
) -> Result<(SingleExposure, SatelliteExposureContext), String> {
    let observer = headers.observer.as_ref().ok_or_else(|| {
        "satellite prediction needs FITS site coordinates (SITELAT/SITELONG or OBSGEO-B/OBSGEO-L)"
            .to_string()
    })?;
    let observer_location = ObserverLocation::geodetic(
        observer.value.latitude_deg,
        observer.value.longitude_deg,
        observer.value.altitude_m,
    )
    .map_err(|error| error.to_string())?;

    let (exposure, provenance, mut keywords) = if let (Some(start), Some(end)) = (
        headers.capture_time.as_ref(),
        headers.exposure_end_time.as_ref(),
    ) {
        let start_utc = UtcTimestamp::parse(&start.value).map_err(|error| error.to_string())?;
        let end_utc = UtcTimestamp::parse(&end.value).map_err(|error| error.to_string())?;
        let exposure = SingleExposure::new(
            start_utc,
            end_utc,
            observer_location,
            ExposureProvenance::FitsBounds,
        )
        .map_err(|error| error.to_string())?;
        let mut keywords = start.sources.clone();
        keywords.extend(end.sources.clone());
        (exposure, "fits_bounds", keywords)
    } else if let (Some(midpoint), Some(duration)) = (
        headers.exposure_mid_time.as_ref(),
        headers.exposure_seconds.as_ref(),
    ) {
        let midpoint_utc =
            UtcTimestamp::parse(&midpoint.value).map_err(|error| error.to_string())?;
        let half_duration = duration.value / 2.0;
        let exposure = SingleExposure::new(
            midpoint_utc
                .add_seconds(-half_duration)
                .map_err(|error| error.to_string())?,
            midpoint_utc
                .add_seconds(half_duration)
                .map_err(|error| error.to_string())?,
            observer_location,
            ExposureProvenance::FitsDateObsAndExposure,
        )
        .map_err(|error| error.to_string())?;
        let mut keywords = midpoint.sources.clone();
        keywords.extend(duration.sources.clone());
        (exposure, "fits_date_avg_and_exposure", keywords)
    } else {
        let start = headers.capture_time.as_ref().ok_or_else(|| {
            "satellite prediction needs FITS exposure timing (DATE-BEG/DATE-OBS or DATE-AVG)"
                .to_string()
        })?;
        let start_utc = UtcTimestamp::parse(&start.value).map_err(|error| error.to_string())?;
        let duration = headers.exposure_seconds.as_ref().ok_or_else(|| {
            "satellite prediction needs FITS EXPTIME/EXPOSURE when DATE-END is absent".to_string()
        })?;
        let exposure = SingleExposure::from_start_and_duration(
            start_utc,
            duration.value,
            observer_location,
            ExposureProvenance::FitsDateObsAndExposure,
        )
        .map_err(|error| error.to_string())?;
        let mut keywords = start.sources.clone();
        keywords.extend(duration.sources.clone());
        (exposure, "fits_date_obs_and_exposure", keywords)
    };
    keywords.extend(observer.sources.clone());
    keywords.sort();
    keywords.dedup();
    let context = SatelliteExposureContext {
        start_utc: exposure.start_utc.to_rfc3339(),
        end_utc: exposure.end_utc.to_rfc3339(),
        duration_seconds: exposure.duration_seconds(),
        latitude_deg: observer.value.latitude_deg,
        longitude_deg: observer.value.longitude_deg,
        altitude_m: observer.value.altitude_m,
        provenance: provenance.to_string(),
        header_keywords: keywords,
    };
    Ok((exposure, context))
}

fn bright_trail_risk(
    sunlight_fraction: f64,
    range_km: f64,
    elevation_deg: f64,
    clipped_length_px: f64,
) -> f64 {
    if !sunlight_fraction.is_finite()
        || !range_km.is_finite()
        || !elevation_deg.is_finite()
        || !clipped_length_px.is_finite()
        || sunlight_fraction <= 0.0
    {
        return 0.0;
    }
    let range_factor = (1.0 - ((range_km - 500.0) / 9_500.0)).clamp(0.0, 1.0);
    let elevation_factor = (elevation_deg / 60.0).clamp(0.0, 1.0);
    let path_factor = (clipped_length_px / 100.0).clamp(0.0, 1.0);
    (sunlight_fraction.clamp(0.0, 1.0)
        * (0.60 * range_factor + 0.20 * elevation_factor + 0.20 * path_factor))
        .clamp(0.0, 1.0)
}

fn cache_path(cache_dir: &Path, image_id: i32) -> PathBuf {
    cache_dir
        .join("satellites")
        .join(format!("{image_id}.json"))
}

pub fn persist_analysis(cache_dir: &Path, analysis: &SatelliteAnalysis) -> Result<(), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static PERSIST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let path = cache_path(cache_dir, analysis.image_id);
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid satellite cache path {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    let temporary = path.with_extension(format!(
        "json.tmp.{}.{}",
        std::process::id(),
        PERSIST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = serde_json::to_vec(analysis).map_err(|error| error.to_string())?;
    let result = std::fs::write(&temporary, bytes)
        .and_then(|_| std::fs::rename(&temporary, &path))
        .map_err(|error| format!("failed to persist {}: {error}", path.display()));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub fn persisted_analysis(
    cache_dir: &Path,
    image_id: i32,
    astrometry: &AstrometryAnalysis,
) -> Option<SatelliteAnalysis> {
    let bytes = std::fs::read(cache_path(cache_dir, image_id)).ok()?;
    let cached: SatelliteAnalysis = serde_json::from_slice(&bytes).ok()?;
    let solution = astrometry.solution.as_ref()?;
    (cached.image_id == image_id
        && cached.source_fingerprint == astrometry.source_fingerprint
        && cached.astrometry_wcs == solution.wcs
        && cached.seiza_version == crate::astrometry::SEIZA_VERSION
        && cached.seiza_satellites_version == SEIZA_SATELLITES_VERSION
        && cached.pixel_alignment_version == PIXEL_ALIGNMENT_VERSION)
        .then_some(cached)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiza_fits::HeaderValue;

    #[test]
    fn risk_requires_sunlight_and_favors_close_long_tracks() {
        assert_eq!(bright_trail_risk(0.0, 500.0, 80.0, 1000.0), 0.0);
        let close = bright_trail_risk(1.0, 600.0, 70.0, 500.0);
        let distant = bright_trail_risk(1.0, 30_000.0, 70.0, 500.0);
        assert!(close > 0.9);
        assert!(distant < close);
    }

    #[test]
    fn date_avg_centers_the_exposure_when_no_explicit_end_exists() {
        let headers = FitsAstrometryHeaders::from_headers(&[
            (
                "DATE-OBS".to_string(),
                HeaderValue::String("2026-05-21T07:13:14.8423096".into()),
            ),
            (
                "DATE-AVG".to_string(),
                HeaderValue::String("2026-05-21T07:13:45.3551363".into()),
            ),
            ("EXPTIME".to_string(), HeaderValue::Float(60.0)),
            ("SITELAT".to_string(), HeaderValue::Float(35.0)),
            ("SITELONG".to_string(), HeaderValue::Float(-105.0)),
            ("SITEELEV".to_string(), HeaderValue::Float(100.0)),
        ]);

        let (_, context) = single_exposure(&headers).unwrap();
        assert_eq!(context.provenance, "fits_date_avg_and_exposure");
        assert_eq!(context.start_utc, "2026-05-21T07:13:15.355136Z");
        assert_eq!(context.end_utc, "2026-05-21T07:14:15.355136Z");
        assert_eq!(
            context.header_keywords,
            ["DATE-AVG", "EXPTIME", "SITEELEV", "SITELAT", "SITELONG"]
        );
    }
}
