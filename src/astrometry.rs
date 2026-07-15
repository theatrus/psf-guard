//! Process-global Seiza catalog configuration, lazy loading, and diagnostics.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

pub const SEIZA_VERSION: &str = "0.4.1";
pub const SEIZA_FITS_VERSION: &str = "0.1.4";

const OBJECTS_FILE: &str = "objects.bin";
const STARS_FILE: &str = "stars-gaia.bin";
const STAR_IDENTIFIERS_FILE: &str = "stars-lite-tycho2.ids.bin";
const BLIND_INDEX_FILE: &str = "blind-gaia16.idx";
const TRANSIENTS_FILE: &str = "transients.bin";
const MINOR_BODIES_FILE: &str = "minor-bodies.bin";

/// Process-global paths to Seiza's offline data files.
///
/// An explicit relative filename is resolved below `data_dir`. When a field is
/// absent but `data_dir` is present, the canonical Seiza bundle filename is
/// auto-discovered. With neither a directory nor an explicit path, that
/// capability is not configured.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AstrometryConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objects: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stars: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub star_identifiers: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blind_index: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transients: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minor_bodies: Option<String>,
}

impl AstrometryConfig {
    fn resolve(&self, explicit: &Option<String>, canonical: &str) -> Option<PathBuf> {
        match explicit {
            Some(path) => {
                let path = PathBuf::from(path);
                if path.is_absolute() {
                    Some(path)
                } else if let Some(data_dir) = &self.data_dir {
                    Some(PathBuf::from(data_dir).join(path))
                } else {
                    Some(path)
                }
            }
            None => self
                .data_dir
                .as_ref()
                .map(|directory| PathBuf::from(directory).join(canonical)),
        }
    }

    pub fn objects_path(&self) -> Option<PathBuf> {
        self.resolve(&self.objects, OBJECTS_FILE)
    }

    pub fn stars_path(&self) -> Option<PathBuf> {
        self.resolve(&self.stars, STARS_FILE)
    }

    pub fn star_identifiers_path(&self) -> Option<PathBuf> {
        self.resolve(&self.star_identifiers, STAR_IDENTIFIERS_FILE)
    }

    pub fn blind_index_path(&self) -> Option<PathBuf> {
        self.resolve(&self.blind_index, BLIND_INDEX_FILE)
    }

    pub fn transients_path(&self) -> Option<PathBuf> {
        self.resolve(&self.transients, TRANSIENTS_FILE)
    }

    pub fn minor_bodies_path(&self) -> Option<PathBuf> {
        self.resolve(&self.minor_bodies, MINOR_BODIES_FILE)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AstrometryResourceStatus {
    NotConfigured,
    Missing,
    Available,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstrometryResourceCapability {
    pub name: String,
    pub status: AstrometryResourceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AstrometryResourceCapability {
    fn available(&self) -> bool {
        self.status == AstrometryResourceStatus::Available
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstrometryResources {
    pub objects: AstrometryResourceCapability,
    pub stars: AstrometryResourceCapability,
    pub star_identifiers: AstrometryResourceCapability,
    pub blind_index: AstrometryResourceCapability,
    pub transients: AstrometryResourceCapability,
    pub minor_bodies: AstrometryResourceCapability,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstrometryFeatures {
    pub object_association: bool,
    pub object_name_search: bool,
    pub stellar_name_search: bool,
    pub hinted_solve: bool,
    pub blind_solve: bool,
    pub transient_annotations: bool,
    pub minor_body_annotations: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstrometryCapabilities {
    pub seiza_version: String,
    pub seiza_fits_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    pub resources: AstrometryResources,
    pub features: AstrometryFeatures,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstrometryResourceValidation {
    pub name: String,
    pub status: AstrometryResourceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub validated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstrometryValidationReport {
    pub all_configured_valid: bool,
    pub resources: Vec<AstrometryResourceValidation>,
}

/// File identity used to invalidate derived image analysis when a path is
/// replaced in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AstrometrySourceFingerprint {
    pub canonical_path: String,
    pub size_bytes: u64,
    pub modified_unix_seconds: u64,
}

/// Catalog identity used to distinguish WCS validity from refreshable
/// annotation validity. Managed installs populate bundle/hash fields; custom
/// directories can rely on the individual file metadata until explicitly
/// hashed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AstrometryCatalogSignature {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_version: Option<String>,
    pub files: Vec<AstrometryCatalogFileSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AstrometryCatalogFileSignature {
    pub name: String,
    pub path: String,
    pub format: String,
    pub size_bytes: u64,
    pub modified_unix_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AstrometryAnalysisStatus {
    Unavailable,
    CatalogOnly,
    Solved,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AstrometrySolveMode {
    EmbeddedWcs,
    Hinted,
    Blind,
}

/// A celestial coordinate plus the source that gave it its semantic role.
/// Keeping hint and expected coordinates separate prevents a derived center
/// from silently replacing the Target Scheduler target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AstrometryCoordinateSource {
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub header_keywords: Vec<String>,
}

/// TAN WCS response compatible with the seiza-server/Tenrankai overlay model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WcsResponse {
    pub crval: [f64; 2],
    pub crpix: [f64; 2],
    pub cd: [[f64; 2]; 2],
    pub ctype: [String; 2],
    pub cunit: [String; 2],
    pub radesys: String,
    pub equinox: f64,
}

/// Seiza v3 identity, hierarchy, and provenance carried through PSF Guard APIs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogObjectIdentity {
    pub stable_id: String,
    pub source: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub parent_ids: Vec<String>,
    #[serde(default)]
    pub alternate_ids: Vec<String>,
    #[serde(default)]
    pub alternate_sources: Vec<String>,
}

/// Object association from known coordinates without a plate solve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogHitResponse {
    #[serde(flatten)]
    pub identity: CatalogObjectIdentity,
    pub name: String,
    pub common_name: String,
    pub kind: String,
    pub mag: Option<f32>,
    pub major_arcmin: Option<f32>,
    pub minor_arcmin: Option<f32>,
    pub position_angle_deg: Option<f32>,
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub center_inside: bool,
    pub extent_only: bool,
    pub distance_from_center_deg: f64,
    /// Catalog-based heuristic only; not evidence of pixel visibility.
    pub predicted_prominence: f64,
}

/// Object projected into a solved image. Core names match
/// `@seiza/astro-overlay` so the frontend can render the shared component
/// without translating the geometry contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlayObjectResponse {
    #[serde(flatten)]
    pub identity: CatalogObjectIdentity,
    pub name: String,
    pub common_name: String,
    pub kind: String,
    pub mag: Option<f32>,
    pub x: f64,
    pub y: f64,
    pub semi_major_px: f64,
    pub semi_minor_px: f64,
    pub angle_deg: f64,
    pub ra_deg: f64,
    pub dec_deg: f64,
    /// Ranking value consumed by the shared overlay's density selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prominence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near_capture: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance_au: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction_pa_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction_angle_deg: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AstrometrySolutionResponse {
    pub center_ra_deg: f64,
    pub center_dec_deg: f64,
    pub pixel_scale_arcsec_per_pixel: f64,
    pub matched_stars: usize,
    pub rms_arcsec: f64,
    pub image_width: u32,
    pub image_height: u32,
    pub wcs: WcsResponse,
    /// ICRS vertices in image-boundary order.
    pub footprint: Vec<[f64; 2]>,
    pub objects: Vec<OverlayObjectResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_time: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PointingResult {
    pub expected_ra_deg: f64,
    pub expected_dec_deg: f64,
    pub east_offset_arcsec: f64,
    pub north_offset_arcsec: f64,
    pub separation_arcsec: f64,
    pub target_in_frame: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_edge_margin_px: Option<f64>,
}

/// Stable top-level per-image response. Phase 1 fills `catalog_hits`; later
/// phases add the solution and pointing fields without changing the envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AstrometryAnalysis {
    pub image_id: i32,
    pub status: AstrometryAnalysisStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<AstrometrySolveMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint_source: Option<AstrometryCoordinateSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_source: Option<AstrometryCoordinateSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<AstrometrySolutionResponse>,
    #[serde(default)]
    pub catalog_hits: Vec<CatalogHitResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointing: Option<PointingResult>,
    pub source_fingerprint: AstrometrySourceFingerprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_signature: Option<AstrometryCatalogSignature>,
    pub computed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

type ObjectCatalog = seiza::objects::ObjectCatalog;
type TileCatalog = seiza::catalog::TileCatalog;
type StarIdentifierCatalog = seiza::star_ids::StarIdentifierCatalog;
type BlindIndex = seiza::blind::BlindIndex;
type MinorBodyCatalog = seiza::minor_bodies::MinorBodyCatalog;

/// Shared, lazily opened Seiza resources. This belongs on `AppState`, not on a
/// database context, because every configured scheduler database uses the same
/// sky catalogs.
pub struct AstrometryContext {
    config: AstrometryConfig,
    validation_running: AtomicBool,
    objects: RwLock<Option<Arc<ObjectCatalog>>>,
    stars: RwLock<Option<Arc<TileCatalog>>>,
    star_identifiers: RwLock<Option<Arc<StarIdentifierCatalog>>>,
    blind_index: RwLock<Option<Arc<BlindIndex>>>,
    transients: RwLock<Option<Arc<ObjectCatalog>>>,
    minor_bodies: RwLock<Option<Arc<MinorBodyCatalog>>>,
}

impl Default for AstrometryContext {
    fn default() -> Self {
        Self::new(AstrometryConfig::default())
    }
}

impl AstrometryContext {
    pub fn new(config: AstrometryConfig) -> Self {
        Self {
            config,
            validation_running: AtomicBool::new(false),
            objects: RwLock::new(None),
            stars: RwLock::new(None),
            star_identifiers: RwLock::new(None),
            blind_index: RwLock::new(None),
            transients: RwLock::new(None),
            minor_bodies: RwLock::new(None),
        }
    }

    pub fn config(&self) -> &AstrometryConfig {
        &self.config
    }

    pub fn object_catalog(&self) -> Result<Arc<ObjectCatalog>, String> {
        load_cached(&self.objects, self.config.objects_path(), |path| {
            ObjectCatalog::open(path).map_err(|error| error.to_string())
        })
    }

    pub fn star_catalog(&self) -> Result<Arc<TileCatalog>, String> {
        load_cached(&self.stars, self.config.stars_path(), |path| {
            TileCatalog::open(path).map_err(|error| error.to_string())
        })
    }

    pub fn star_identifier_catalog(&self) -> Result<Arc<StarIdentifierCatalog>, String> {
        load_cached(
            &self.star_identifiers,
            self.config.star_identifiers_path(),
            |path| StarIdentifierCatalog::open(path).map_err(|error| error.to_string()),
        )
    }

    pub fn blind_index(&self) -> Result<Arc<BlindIndex>, String> {
        load_cached(&self.blind_index, self.config.blind_index_path(), |path| {
            BlindIndex::open(path).map_err(|error| error.to_string())
        })
    }

    pub fn transient_catalog(&self) -> Result<Arc<ObjectCatalog>, String> {
        load_cached(&self.transients, self.config.transients_path(), |path| {
            ObjectCatalog::open(path).map_err(|error| error.to_string())
        })
    }

    pub fn minor_body_catalog(&self) -> Result<Arc<MinorBodyCatalog>, String> {
        load_cached(
            &self.minor_bodies,
            self.config.minor_bodies_path(),
            |path| MinorBodyCatalog::open(path).map_err(|error| error.to_string()),
        )
    }

    /// Open configured files lazily and report which higher-level features are
    /// usable. This performs only each format's bounded normal open, never an
    /// exhaustive validation scan.
    pub fn capabilities(&self) -> AstrometryCapabilities {
        let objects = capability(
            "objects",
            self.config.objects_path(),
            self.object_catalog().map(|_| ()),
        );
        let stars = capability(
            "stars",
            self.config.stars_path(),
            self.star_catalog().map(|_| ()),
        );
        let star_identifiers = capability(
            "star_identifiers",
            self.config.star_identifiers_path(),
            self.star_identifier_catalog().map(|_| ()),
        );
        let blind_index = capability(
            "blind_index",
            self.config.blind_index_path(),
            self.blind_index().map(|_| ()),
        );
        let transients = capability(
            "transients",
            self.config.transients_path(),
            self.transient_catalog().map(|_| ()),
        );
        let minor_bodies = capability(
            "minor_bodies",
            self.config.minor_bodies_path(),
            self.minor_body_catalog().map(|_| ()),
        );

        let features = AstrometryFeatures {
            object_association: objects.available(),
            object_name_search: objects.available(),
            stellar_name_search: star_identifiers.available(),
            hinted_solve: stars.available(),
            blind_solve: stars.available() && blind_index.available(),
            transient_annotations: transients.available(),
            minor_body_annotations: minor_bodies.available(),
        };

        AstrometryCapabilities {
            seiza_version: SEIZA_VERSION.to_string(),
            seiza_fits_version: SEIZA_FITS_VERSION.to_string(),
            data_dir: self.config.data_dir.clone(),
            resources: AstrometryResources {
                objects,
                stars,
                star_identifiers,
                blind_index,
                transients,
                minor_bodies,
            },
            features,
        }
    }

    /// Deliberately touch and validate every configured resource. Callers must
    /// run this on a blocking worker because deep catalogs can take time to
    /// page through.
    pub fn try_validate_all(&self) -> Result<AstrometryValidationReport, String> {
        let _guard = self.begin_validation()?;
        Ok(self.validate_all())
    }

    fn begin_validation(&self) -> Result<AstrometryValidationGuard<'_>, String> {
        self.validation_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "astrometry catalog validation is already running".to_string())?;
        Ok(AstrometryValidationGuard {
            running: &self.validation_running,
        })
    }

    fn validate_all(&self) -> AstrometryValidationReport {
        let resources = vec![
            validate_resource("objects", self.config.objects_path(), || {
                self.object_catalog()?
                    .validate()
                    .map_err(|error| error.to_string())
            }),
            validate_resource("stars", self.config.stars_path(), || {
                self.star_catalog()?
                    .validate()
                    .map_err(|error| error.to_string())
            }),
            validate_resource(
                "star_identifiers",
                self.config.star_identifiers_path(),
                || {
                    self.star_identifier_catalog()?
                        .validate()
                        .map_err(|error| error.to_string())
                },
            ),
            validate_resource("blind_index", self.config.blind_index_path(), || {
                self.blind_index()?
                    .validate()
                    .map_err(|error| error.to_string())
            }),
            validate_resource("transients", self.config.transients_path(), || {
                self.transient_catalog()?
                    .validate()
                    .map_err(|error| error.to_string())
            }),
            validate_resource("minor_bodies", self.config.minor_bodies_path(), || {
                self.minor_body_catalog()?
                    .validate()
                    .map_err(|error| error.to_string())
            }),
        ];
        let configured: Vec<_> = resources
            .iter()
            .filter(|resource| resource.status != AstrometryResourceStatus::NotConfigured)
            .collect();
        let all_configured_valid = !configured.is_empty()
            && configured
                .iter()
                .all(|resource| resource.validated && resource.error.is_none());
        AstrometryValidationReport {
            all_configured_valid,
            resources,
        }
    }
}

struct AstrometryValidationGuard<'a> {
    running: &'a AtomicBool,
}

impl Drop for AstrometryValidationGuard<'_> {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

fn load_cached<T>(
    cache: &RwLock<Option<Arc<T>>>,
    path: Option<PathBuf>,
    open: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<Arc<T>, String> {
    if let Some(value) = cache.read().unwrap().as_ref() {
        return Ok(Arc::clone(value));
    }
    let path = path.ok_or_else(|| "resource is not configured".to_string())?;
    if !path.is_file() {
        return Err(format!("resource file does not exist: {}", path.display()));
    }
    let mut guard = cache.write().unwrap();
    if let Some(value) = guard.as_ref() {
        return Ok(Arc::clone(value));
    }
    let value = Arc::new(open(&path).map_err(|error| format!("{}: {error}", path.display()))?);
    *guard = Some(Arc::clone(&value));
    Ok(value)
}

fn capability(
    name: &str,
    path: Option<PathBuf>,
    opened: Result<(), String>,
) -> AstrometryResourceCapability {
    let Some(path) = path else {
        return AstrometryResourceCapability {
            name: name.to_string(),
            status: AstrometryResourceStatus::NotConfigured,
            path: None,
            format: None,
            size_bytes: None,
            modified_unix_seconds: None,
            error: None,
        };
    };
    let path_string = path.to_string_lossy().into_owned();
    if !path.is_file() {
        return AstrometryResourceCapability {
            name: name.to_string(),
            status: AstrometryResourceStatus::Missing,
            path: Some(path_string),
            format: None,
            size_bytes: None,
            modified_unix_seconds: None,
            error: Some("configured resource file does not exist".to_string()),
        };
    }
    let metadata = path.metadata().ok();
    let size_bytes = metadata.as_ref().map(|metadata| metadata.len());
    let modified_unix_seconds = metadata
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    let format = read_magic(&path).ok();
    match opened {
        Ok(()) => AstrometryResourceCapability {
            name: name.to_string(),
            status: AstrometryResourceStatus::Available,
            path: Some(path_string),
            format,
            size_bytes,
            modified_unix_seconds,
            error: None,
        },
        Err(error) => AstrometryResourceCapability {
            name: name.to_string(),
            status: AstrometryResourceStatus::Invalid,
            path: Some(path_string),
            format,
            size_bytes,
            modified_unix_seconds,
            error: Some(error),
        },
    }
}

fn read_magic(path: &Path) -> std::io::Result<String> {
    let mut bytes = [0u8; 8];
    File::open(path)?.read_exact(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn validate_resource(
    name: &str,
    path: Option<PathBuf>,
    validate: impl FnOnce() -> Result<(), String>,
) -> AstrometryResourceValidation {
    let Some(path) = path else {
        return AstrometryResourceValidation {
            name: name.to_string(),
            status: AstrometryResourceStatus::NotConfigured,
            path: None,
            validated: false,
            error: None,
        };
    };
    let path_string = path.to_string_lossy().into_owned();
    if !path.is_file() {
        return AstrometryResourceValidation {
            name: name.to_string(),
            status: AstrometryResourceStatus::Missing,
            path: Some(path_string),
            validated: false,
            error: Some("configured resource file does not exist".to_string()),
        };
    }
    match validate() {
        Ok(()) => AstrometryResourceValidation {
            name: name.to_string(),
            status: AstrometryResourceStatus::Available,
            path: Some(path_string),
            validated: true,
            error: None,
        },
        Err(error) => AstrometryResourceValidation {
            name: name.to_string(),
            status: AstrometryResourceStatus::Invalid,
            path: Some(path_string),
            validated: false,
            error: Some(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiza::objects::{ObjectCatalog, ObjectKind, ObjectMetadata, SkyObject};

    fn test_object() -> SkyObject {
        SkyObject {
            kind: ObjectKind::Galaxy,
            ra: 10.6848,
            dec: 41.2691,
            mag: Some(3.44),
            major_arcmin: Some(190.0),
            minor_arcmin: Some(60.0),
            position_angle_deg: Some(35.0),
            name: "NGC 224".to_string(),
            common_name: "Andromeda Galaxy".to_string(),
            metadata: ObjectMetadata {
                id: "openngc:NGC224".to_string(),
                source: "OpenNGC".to_string(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn canonical_paths_resolve_below_data_dir() {
        let config = AstrometryConfig {
            data_dir: Some("/catalogs".to_string()),
            objects: None,
            stars: Some("custom-stars.bin".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config.objects_path().unwrap(),
            PathBuf::from("/catalogs/objects.bin")
        );
        assert_eq!(
            config.stars_path().unwrap(),
            PathBuf::from("/catalogs/custom-stars.bin")
        );
    }

    #[test]
    fn no_configuration_reports_no_capabilities() {
        let capabilities = AstrometryContext::default().capabilities();
        assert_eq!(
            capabilities.resources.objects.status,
            AstrometryResourceStatus::NotConfigured
        );
        assert!(!capabilities.features.object_association);
        assert!(!capabilities.features.hinted_solve);
        assert!(!capabilities.features.blind_solve);
    }

    #[test]
    fn exhaustive_validation_is_singleton() {
        let context = AstrometryContext::default();
        let first = context.begin_validation().unwrap();
        assert_eq!(
            context.begin_validation().err().as_deref(),
            Some("astrometry catalog validation is already running")
        );
        drop(first);
        assert!(context.begin_validation().is_ok());
    }

    #[test]
    fn indexed_object_catalog_is_available_and_validates() {
        let directory = tempfile::tempdir().unwrap();
        let objects_path = directory.path().join(OBJECTS_FILE);
        ObjectCatalog::new(vec![test_object()])
            .write_to(&objects_path)
            .unwrap();

        let context = AstrometryContext::new(AstrometryConfig {
            objects: Some(objects_path.to_string_lossy().into_owned()),
            ..Default::default()
        });
        let capabilities = context.capabilities();
        assert_eq!(
            capabilities.resources.objects.status,
            AstrometryResourceStatus::Available
        );
        assert_eq!(
            capabilities.resources.objects.format.as_deref(),
            Some("SEIZAOB3")
        );
        assert!(capabilities.features.object_association);

        let validation = context.try_validate_all().unwrap();
        let objects = validation
            .resources
            .iter()
            .find(|resource| resource.name == "objects")
            .unwrap();
        assert!(objects.validated);
        assert_eq!(objects.status, AstrometryResourceStatus::Available);
        assert!(validation.all_configured_valid);
    }

    #[test]
    fn legacy_object_catalog_remains_readable() {
        let directory = tempfile::tempdir().unwrap();
        let objects_path = directory.path().join("objects-v1.bin");
        ObjectCatalog::new(vec![test_object()])
            .write_v1_to(&objects_path)
            .unwrap();

        let context = AstrometryContext::new(AstrometryConfig {
            objects: Some(objects_path.to_string_lossy().into_owned()),
            ..Default::default()
        });
        let capabilities = context.capabilities();
        assert_eq!(
            capabilities.resources.objects.status,
            AstrometryResourceStatus::Available
        );
        assert_eq!(
            capabilities.resources.objects.format.as_deref(),
            Some("SEIZAOB1")
        );
        assert!(context.try_validate_all().unwrap().all_configured_valid);
    }

    #[test]
    fn malformed_object_catalog_is_reported_as_invalid() {
        let directory = tempfile::tempdir().unwrap();
        let objects_path = directory.path().join(OBJECTS_FILE);
        std::fs::write(&objects_path, b"not a seiza catalog").unwrap();

        let context = AstrometryContext::new(AstrometryConfig {
            objects: Some(objects_path.to_string_lossy().into_owned()),
            ..Default::default()
        });
        let capabilities = context.capabilities();
        assert_eq!(
            capabilities.resources.objects.status,
            AstrometryResourceStatus::Invalid
        );
        assert!(!capabilities.features.object_association);
        let validation = context.try_validate_all().unwrap();
        assert!(!validation.all_configured_valid);
        assert_eq!(
            validation.resources[0].status,
            AstrometryResourceStatus::Invalid
        );
    }

    #[test]
    fn configured_missing_resource_is_distinct_from_not_configured() {
        let context = AstrometryContext::new(AstrometryConfig {
            objects: Some("/definitely/missing/objects.bin".to_string()),
            ..Default::default()
        });
        let capabilities = context.capabilities();
        assert_eq!(
            capabilities.resources.objects.status,
            AstrometryResourceStatus::Missing
        );
        assert!(capabilities.resources.objects.error.is_some());
    }

    #[test]
    fn analysis_contract_keeps_hint_expected_and_solution_separate() {
        let analysis = AstrometryAnalysis {
            image_id: 42,
            status: AstrometryAnalysisStatus::CatalogOnly,
            mode: None,
            hint_source: Some(AstrometryCoordinateSource {
                ra_deg: 10.5,
                dec_deg: 20.5,
                source: "fits_header".to_string(),
                header_keywords: vec!["RA".to_string(), "DEC".to_string()],
            }),
            expected_source: Some(AstrometryCoordinateSource {
                ra_deg: 11.0,
                dec_deg: 21.0,
                source: "target_scheduler".to_string(),
                header_keywords: Vec::new(),
            }),
            solution: None,
            catalog_hits: Vec::new(),
            pointing: None,
            source_fingerprint: AstrometrySourceFingerprint {
                canonical_path: "/images/frame.fits".to_string(),
                size_bytes: 1234,
                modified_unix_seconds: 5678,
            },
            catalog_signature: None,
            computed_at: 9999,
            error: None,
        };

        let json = serde_json::to_value(&analysis).unwrap();
        assert_eq!(json["status"], "catalog_only");
        assert_eq!(json["hint_source"]["source"], "fits_header");
        assert_eq!(json["expected_source"]["source"], "target_scheduler");
        assert!(json.get("solution").is_none());
    }
}
