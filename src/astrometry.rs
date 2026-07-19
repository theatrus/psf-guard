//! Process-global Seiza catalog configuration, lazy loading, and diagnostics.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const SEIZA_VERSION: &str = "0.8.0";
pub const SEIZA_FITS_VERSION: &str = "0.1.6";

pub type AstrometryResourcePath = Result<Option<PathBuf>, seiza::data_paths::DataPathError>;

/// Process-global paths to Seiza's offline data files.
///
/// `data_dir` is enough to configure an entire Seiza bundle. Explicit resource
/// paths override it, with relative paths resolved below the directory. When
/// neither is set, Seiza's standard environment, executable-adjacent, and
/// platform data locations are searched.
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
    fn resolver_input(&self, explicit: &Option<String>) -> Option<PathBuf> {
        let data_dir = self
            .data_dir
            .as_deref()
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        match explicit.as_deref().filter(|path| !path.is_empty()) {
            Some(path) => {
                let path = PathBuf::from(path);
                if path.is_absolute() {
                    Some(path)
                } else if let Some(data_dir) = data_dir {
                    Some(data_dir.join(path))
                } else {
                    Some(path)
                }
            }
            None => data_dir,
        }
    }

    fn resolve_required(
        &self,
        explicit: &Option<String>,
        resolve: impl FnOnce(Option<&Path>) -> Result<PathBuf, seiza::data_paths::DataPathError>,
    ) -> AstrometryResourcePath {
        let input = self.resolver_input(explicit);
        match resolve(input.as_deref()) {
            Ok(path) => Ok(Some(path)),
            Err(seiza::data_paths::DataPathError::NoDefault { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn objects_path(&self) -> AstrometryResourcePath {
        self.resolve_required(&self.objects, seiza::data_paths::objects)
    }

    pub fn stars_path(&self) -> AstrometryResourcePath {
        self.resolve_required(&self.stars, seiza::data_paths::star_data)
    }

    pub fn star_identifiers_path(&self) -> AstrometryResourcePath {
        self.resolve_required(&self.star_identifiers, seiza::data_paths::star_identifiers)
    }

    pub fn blind_index_path(&self) -> AstrometryResourcePath {
        let input = self.resolver_input(&self.blind_index);
        seiza::data_paths::blind_index(input.as_deref())
    }

    pub fn transients_path(&self) -> AstrometryResourcePath {
        self.resolve_required(&self.transients, seiza::data_paths::transients)
    }

    pub fn minor_bodies_path(&self) -> AstrometryResourcePath {
        self.resolve_required(&self.minor_bodies, seiza::data_paths::minor_bodies)
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
    pub modified_subsec_nanos: Option<u32>,
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
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub modified_subsec_nanos: u32,
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
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub modified_subsec_nanos: u32,
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

/// How the object-catalog search region was established. This keeps a
/// conservative coordinate-only lookup distinct from a known image field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AstrometryCatalogScope {
    EmbeddedFootprint,
    SolvedFootprint,
    EstimatedField,
    NearbyTarget,
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

/// Seiza object identity, hierarchy, and provenance carried through PSF Guard APIs.
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
pub struct OverlayContourResponse {
    pub closed: bool,
    pub points: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlayOutlineResponse {
    pub geometry_id: String,
    pub source_record_id: String,
    pub role: String,
    pub quality: String,
    pub level: Option<String>,
    pub contours: Vec<OverlayContourResponse>,
}

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
    pub angle_deg: Option<f64>,
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
    /// Pixel-projected Seiza v4 catalog contours. The source-qualified
    /// geometry metadata stays attached so consumers can distinguish curated
    /// outlines from estimates and fallback extents.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outlines: Vec<OverlayOutlineResponse>,
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

/// Reproducibility details for a pixel-derived plate solution. Object-catalog
/// provenance remains in `catalog_signature`; this records the solver inputs
/// that established the WCS itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AstrometrySolverProvenance {
    pub seiza_version: String,
    pub detection_backend: String,
    pub star_catalog: AstrometryCatalogFileSignature,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blind_index: Option<AstrometryCatalogFileSignature>,
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

/// Stable top-level per-image response. Header-only analysis fills catalog,
/// embedded-WCS, and pointing fields; later solving phases keep the envelope.
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
    pub catalog_scope: Option<AstrometryCatalogScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_radius_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointing: Option<PointingResult>,
    pub source_fingerprint: AstrometrySourceFingerprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_signature: Option<AstrometryCatalogSignature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solver_provenance: Option<AstrometrySolverProvenance>,
    pub computed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Target Scheduler stores right ascension as decimal hours, while every
/// Seiza and overlay contract uses ICRS degrees. Keep the conversion at the
/// database boundary so a value such as `16.7` can never be mistaken for
/// sixteen degrees by pointing/drift analysis.
pub fn target_scheduler_coordinates(ra_hours: f64, dec_deg: f64) -> Option<(f64, f64)> {
    if !ra_hours.is_finite()
        || !dec_deg.is_finite()
        || !(0.0..=24.0).contains(&ra_hours)
        || !(-90.0..=90.0).contains(&dec_deg)
    {
        return None;
    }
    Some(((ra_hours * 15.0).rem_euclid(360.0), dec_deg))
}

type ObjectCatalog = seiza::objects::ObjectCatalog;
type TileCatalog = seiza::catalog::TileCatalog;
type StarIdentifierCatalog = seiza::star_ids::StarIdentifierCatalog;
type BlindIndex = seiza::blind::BlindIndex;
type MinorBodyCatalog = seiza::minor_bodies::MinorBodyCatalog;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResourceFingerprint {
    canonical_path: PathBuf,
    size_bytes: u64,
    modified: SystemTime,
    header: Vec<u8>,
}

struct LoadedResource<T> {
    value: Arc<T>,
    fingerprint: ResourceFingerprint,
}

impl<T> Clone for LoadedResource<T> {
    fn clone(&self) -> Self {
        Self {
            value: Arc::clone(&self.value),
            fingerprint: self.fingerprint.clone(),
        }
    }
}

type ResourceCache<T> = RwLock<Option<LoadedResource<T>>>;

/// Shared, lazily opened Seiza resources. This belongs on `AppState`, not on a
/// database context, because every configured scheduler database uses the same
/// sky catalogs.
pub struct AstrometryContext {
    config: AstrometryConfig,
    validation_running: AtomicBool,
    objects: ResourceCache<ObjectCatalog>,
    stars: ResourceCache<TileCatalog>,
    star_identifiers: ResourceCache<StarIdentifierCatalog>,
    blind_index: ResourceCache<BlindIndex>,
    transients: ResourceCache<ObjectCatalog>,
    minor_bodies: ResourceCache<MinorBodyCatalog>,
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
        self.load_object_catalog().map(|loaded| loaded.value)
    }

    pub fn star_catalog(&self) -> Result<Arc<TileCatalog>, String> {
        self.load_star_catalog().map(|loaded| loaded.value)
    }

    pub fn star_identifier_catalog(&self) -> Result<Arc<StarIdentifierCatalog>, String> {
        self.load_star_identifier_catalog()
            .map(|loaded| loaded.value)
    }

    pub fn blind_index(&self) -> Result<Arc<BlindIndex>, String> {
        self.load_blind_index().map(|loaded| loaded.value)
    }

    pub fn transient_catalog(&self) -> Result<Arc<ObjectCatalog>, String> {
        self.load_transient_catalog().map(|loaded| loaded.value)
    }

    pub fn minor_body_catalog(&self) -> Result<Arc<MinorBodyCatalog>, String> {
        self.load_minor_body_catalog().map(|loaded| loaded.value)
    }

    fn load_object_catalog(&self) -> Result<LoadedResource<ObjectCatalog>, String> {
        load_cached(&self.objects, self.config.objects_path(), |path| {
            ObjectCatalog::open(path).map_err(|error| error.to_string())
        })
    }

    fn load_star_catalog(&self) -> Result<LoadedResource<TileCatalog>, String> {
        load_cached(&self.stars, self.config.stars_path(), |path| {
            TileCatalog::open(path).map_err(|error| error.to_string())
        })
    }

    fn load_star_identifier_catalog(
        &self,
    ) -> Result<LoadedResource<StarIdentifierCatalog>, String> {
        load_cached(
            &self.star_identifiers,
            self.config.star_identifiers_path(),
            |path| StarIdentifierCatalog::open(path).map_err(|error| error.to_string()),
        )
    }

    fn load_blind_index(&self) -> Result<LoadedResource<BlindIndex>, String> {
        load_cached(&self.blind_index, self.config.blind_index_path(), |path| {
            BlindIndex::open(path).map_err(|error| error.to_string())
        })
    }

    fn load_transient_catalog(&self) -> Result<LoadedResource<ObjectCatalog>, String> {
        load_cached(&self.transients, self.config.transients_path(), |path| {
            ObjectCatalog::open(path).map_err(|error| error.to_string())
        })
    }

    fn load_minor_body_catalog(&self) -> Result<LoadedResource<MinorBodyCatalog>, String> {
        load_cached(
            &self.minor_bodies,
            self.config.minor_bodies_path(),
            |path| MinorBodyCatalog::open(path).map_err(|error| error.to_string()),
        )
    }

    /// Analyze one image from its FITS headers and the configured object
    /// catalog. This is intentionally header-only: it never decodes pixels or
    /// launches a plate solve. A validated standard embedded TAN WCS can still
    /// produce exact overlay geometry; otherwise the result remains a
    /// coordinate-only catalog association.
    pub fn analyze_image(
        &self,
        image_id: i32,
        path: &Path,
        expected_target: Option<(f64, f64)>,
    ) -> Result<AstrometryAnalysis, String> {
        use seiza::objects::{ObjectQuery, SkyRegion};

        let fingerprint = source_fingerprint(path)?;
        let headers = crate::astrometry_headers::FitsAstrometryHeaders::from_path(path)
            .map_err(|error| format!("failed to read FITS astrometry headers: {error}"))?;
        let dimensions = headers
            .width
            .as_ref()
            .zip(headers.height.as_ref())
            .map(|(width, height)| (width.value, height.value));

        let header_center = headers
            .center_ra_deg
            .as_ref()
            .zip(headers.center_dec_deg.as_ref())
            .map(|(ra, dec)| (ra.value, dec.value));
        let hint_source = headers
            .embedded_wcs
            .as_ref()
            .map(|wcs| AstrometryCoordinateSource {
                ra_deg: wcs.value.crval[0],
                dec_deg: wcs.value.crval[1],
                source: "fits_wcs".to_string(),
                header_keywords: wcs.sources.clone(),
            })
            .or_else(|| {
                headers
                    .center_ra_deg
                    .as_ref()
                    .zip(headers.center_dec_deg.as_ref())
                    .map(|(ra, dec)| AstrometryCoordinateSource {
                        ra_deg: ra.value,
                        dec_deg: dec.value,
                        source: "fits_header".to_string(),
                        header_keywords: [ra.sources.clone(), dec.sources.clone()].concat(),
                    })
            });
        let expected_source = expected_target.map(|(ra_deg, dec_deg)| AstrometryCoordinateSource {
            ra_deg,
            dec_deg,
            source: "target_scheduler".to_string(),
            header_keywords: Vec::new(),
        });

        let embedded = headers.embedded_wcs.as_ref().map(|value| &value.value);
        let wcs = embedded.map(|value| seiza::Wcs {
            crval: (value.crval[0], value.crval[1]),
            crpix: (value.crpix[0], value.crpix[1]),
            cd: value.cd,
            // The header parser deliberately accepts only undistorted TAN WCS
            // today. Seiza 0.7 supports SIP, but accepting those FITS headers
            // requires parsing and validating their coefficient matrices.
            sip: None,
        });
        let exact_region =
            wcs.as_ref()
                .zip(dimensions)
                .map(|(wcs, (width, height))| SkyRegion::Polygon {
                    vertices: wcs.footprint(width, height).to_vec(),
                });
        let estimated_region = if exact_region.is_none() {
            header_center
                .or(expected_target)
                .zip(dimensions)
                .zip(headers.pixel_scale_arcsec_per_pixel.as_ref())
                .and_then(|(((ra, dec), (width, height)), scale)| {
                    let width_deg = f64::from(width) * scale.value / 3600.0;
                    let height_deg = f64::from(height) * scale.value / 3600.0;
                    let radius_deg = 0.5 * width_deg.hypot(height_deg);
                    (radius_deg.is_finite() && radius_deg > 0.0).then_some(SkyRegion::Cone {
                        center: (ra, dec),
                        radius_deg,
                    })
                })
        } else {
            None
        };
        const DEFAULT_NEARBY_RADIUS_DEG: f64 = 1.0;
        let nearby_region = if exact_region.is_none() && estimated_region.is_none() {
            header_center
                .or(expected_target)
                .map(|center| SkyRegion::Cone {
                    center,
                    radius_deg: DEFAULT_NEARBY_RADIUS_DEG,
                })
        } else {
            None
        };
        let region = exact_region
            .as_ref()
            .or(estimated_region.as_ref())
            .or(nearby_region.as_ref());
        let catalog_scope = if exact_region.is_some() {
            Some(AstrometryCatalogScope::EmbeddedFootprint)
        } else if estimated_region.is_some() {
            Some(AstrometryCatalogScope::EstimatedField)
        } else if nearby_region.is_some() {
            Some(AstrometryCatalogScope::NearbyTarget)
        } else {
            None
        };
        let catalog_radius_deg = match region {
            Some(SkyRegion::Cone { radius_deg, .. }) => Some(*radius_deg),
            _ => None,
        };

        let mut analysis_error = None;
        let mut hits = Vec::new();
        let mut catalog_query_succeeded = false;
        let (catalog, signature) = match self.load_object_catalog() {
            Ok(loaded) => {
                let signature = object_catalog_signature(&loaded.fingerprint);
                (Some(loaded.value), Some(signature))
            }
            Err(error) => {
                analysis_error = Some(format!("object catalog unavailable: {error}"));
                (None, None)
            }
        };
        if let (Some(catalog), Some(region)) = (catalog.as_ref(), region) {
            let query = ObjectQuery {
                limit: Some(250),
                ..ObjectQuery::default()
            };
            match catalog.query_region(region, &query) {
                Ok(found) => {
                    hits = found;
                    catalog_query_succeeded = true;
                }
                Err(error) => analysis_error = Some(error.to_string()),
            }
        } else if region.is_none() && analysis_error.is_none() {
            analysis_error = Some("image needs coordinates for catalog association".to_string());
        }

        let catalog_hits = hits
            .iter()
            .take(100)
            .map(catalog_hit_response)
            .collect::<Vec<_>>();
        let catalog_version = signature
            .as_ref()
            .and_then(|signature| signature.files.first())
            .map(|file| file.format.clone());

        let solution = match (wcs.as_ref(), embedded, dimensions) {
            (Some(wcs), Some(embedded), Some((width, height))) => {
                let prominence = hits
                    .iter()
                    .map(|hit| (sky_object_key(&hit.object), hit.predicted_prominence))
                    .collect::<HashMap<_, _>>();
                let mut objects = if let Some(catalog) = catalog.as_ref() {
                    match catalog.objects_in_footprint(wcs, (width, height)) {
                        Ok(placed) => placed
                            .into_iter()
                            .map(|placed| {
                                let rank = prominence
                                    .get(&sky_object_key(&placed.object))
                                    .copied()
                                    .unwrap_or(0.0);
                                overlay_object_response(placed, Some(rank), catalog, wcs)
                            })
                            .collect::<Vec<_>>(),
                        Err(error) => {
                            analysis_error = Some(error.to_string());
                            Vec::new()
                        }
                    }
                } else {
                    Vec::new()
                };
                objects.sort_by(|left, right| {
                    right
                        .prominence
                        .unwrap_or(0.0)
                        .total_cmp(&left.prominence.unwrap_or(0.0))
                });
                objects.truncate(500);

                let center = wcs.pixel_to_world(
                    (f64::from(width) - 1.0) / 2.0,
                    (f64::from(height) - 1.0) / 2.0,
                );
                Some(AstrometrySolutionResponse {
                    center_ra_deg: center.0,
                    center_dec_deg: center.1,
                    pixel_scale_arcsec_per_pixel: wcs.scale_arcsec_per_px(),
                    matched_stars: 0,
                    rms_arcsec: 0.0,
                    image_width: width,
                    image_height: height,
                    wcs: WcsResponse {
                        crval: embedded.crval,
                        crpix: embedded.crpix,
                        cd: embedded.cd,
                        ctype: embedded.ctype.clone(),
                        cunit: embedded.cunit.clone(),
                        radesys: embedded.radesys.clone(),
                        equinox: embedded.equinox,
                    },
                    footprint: wcs
                        .footprint(width, height)
                        .into_iter()
                        .map(|(ra, dec)| [ra, dec])
                        .collect(),
                    objects,
                    catalog_version,
                    capture_time: headers
                        .capture_time
                        .as_ref()
                        .map(|value| value.value.clone()),
                })
            }
            _ => None,
        };

        let pointing = solution
            .as_ref()
            .zip(wcs.as_ref())
            .zip(expected_target)
            .map(|((solution, wcs), expected)| pointing_result(solution, wcs, expected));
        let status = if solution.is_some() {
            AstrometryAnalysisStatus::Solved
        } else if catalog_query_succeeded {
            AstrometryAnalysisStatus::CatalogOnly
        } else {
            AstrometryAnalysisStatus::Unavailable
        };

        Ok(AstrometryAnalysis {
            image_id,
            status,
            mode: solution.as_ref().map(|_| AstrometrySolveMode::EmbeddedWcs),
            hint_source,
            expected_source,
            solution,
            catalog_hits,
            catalog_scope,
            catalog_radius_deg,
            pointing,
            source_fingerprint: fingerprint,
            catalog_signature: signature,
            solver_provenance: None,
            computed_at: std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_secs() as i64),
            error: analysis_error,
        })
    }

    /// Return a persisted pixel-derived solution when it still describes the
    /// same source file, object catalog, Seiza version, and star catalog.
    /// Embedded FITS WCS always wins because it is part of the source itself.
    pub fn with_cached_solution(
        &self,
        cache_dir: &Path,
        fresh: AstrometryAnalysis,
    ) -> AstrometryAnalysis {
        if fresh.solution.is_some() {
            return fresh;
        }
        let path = astrometry_cache_path(cache_dir, fresh.image_id);
        let Ok(bytes) = std::fs::read(&path) else {
            return fresh;
        };
        let Ok(mut cached) = serde_json::from_slice::<AstrometryAnalysis>(&bytes) else {
            tracing::warn!("Ignoring unreadable astrometry cache {}", path.display());
            return fresh;
        };
        let Some(provenance) = cached.solver_provenance.as_ref() else {
            return fresh;
        };
        let current_stars = match self.load_star_catalog() {
            Ok(loaded) => catalog_file_signature("stars", &loaded.fingerprint),
            Err(_) => return fresh,
        };
        let valid = cached.status == AstrometryAnalysisStatus::Solved
            && cached.image_id == fresh.image_id
            && matches!(
                cached.mode,
                Some(AstrometrySolveMode::Hinted | AstrometrySolveMode::Blind)
            )
            && cached.solution.is_some()
            && cached.source_fingerprint == fresh.source_fingerprint
            && cached.catalog_signature == fresh.catalog_signature
            && provenance.seiza_version == SEIZA_VERSION
            && provenance.star_catalog == current_stars;
        if !valid {
            return fresh;
        }

        cached.image_id = fresh.image_id;
        cached.hint_source = fresh.hint_source;
        cached.expected_source = fresh.expected_source;
        cached.source_fingerprint = fresh.source_fingerprint;
        cached.catalog_signature = fresh.catalog_signature;
        cached.pointing = cached
            .solution
            .as_ref()
            .zip(cached.expected_source.as_ref())
            .map(|(solution, expected)| {
                let wcs = wcs_from_response(&solution.wcs);
                pointing_result(solution, &wcs, (expected.ra_deg, expected.dec_deg))
            });
        cached
    }

    /// Decode an ordinary acquisition FITS image, detect stars, run a hinted
    /// solve when coordinates and scale are available, then fall back to the
    /// configured blind index. A compact MTF/u8 detection pass is attempted
    /// first; linear f32 detection is the compatibility fallback used by the
    /// Seiza CLI for difficult fields.
    pub fn solve_image(
        &self,
        image_id: i32,
        path: &Path,
        expected_target: Option<(f64, f64)>,
    ) -> Result<AstrometryAnalysis, String> {
        let mut analysis = self.analyze_image(image_id, path, expected_target)?;
        if analysis.solution.is_some() {
            return Ok(analysis);
        }

        let headers = crate::astrometry_headers::FitsAstrometryHeaders::from_path(path)
            .map_err(|error| format!("failed to read FITS astrometry headers: {error}"))?;
        let hint = analysis
            .hint_source
            .as_ref()
            .or(analysis.expected_source.as_ref())
            .map(|source| (source.ra_deg, source.dec_deg));
        let scale = headers
            .pixel_scale_arcsec_per_pixel
            .as_ref()
            .map(|value| value.value);
        let stars_catalog = match self.load_star_catalog() {
            Ok(loaded) => loaded,
            Err(error) => {
                return Ok(failed_analysis(
                    analysis,
                    format!("star catalog unavailable: {error}"),
                ));
            }
        };

        let (primary_stars, mut dimensions) = match detect_fits_stars(path, DetectionPass::MtfU8) {
            Ok(result) => result,
            Err(error) => return Ok(failed_analysis(analysis, error)),
        };
        let primary = self.try_solve_stars(
            &primary_stars,
            &stars_catalog.value,
            hint,
            scale,
            dimensions,
        );
        let (mode, solved, blind_index, detection_backend) = match primary {
            Ok((mode, solved, blind_index)) => (mode, solved, blind_index, "mtf_u8".to_string()),
            Err(primary_error) => {
                let (fallback_stars, fallback_dimensions) =
                    match detect_fits_stars(path, DetectionPass::LinearF32) {
                        Ok(result) => result,
                        Err(error) => {
                            return Ok(failed_analysis(
                                analysis,
                                format!("{primary_error}; f32 detection failed: {error}"),
                            ));
                        }
                    };
                match self.try_solve_stars(
                    &fallback_stars,
                    &stars_catalog.value,
                    hint,
                    scale,
                    fallback_dimensions,
                ) {
                    Ok((mode, solved, blind_index)) => {
                        dimensions = fallback_dimensions;
                        (mode, solved, blind_index, "linear_f32".to_string())
                    }
                    Err(fallback_error) => {
                        return Ok(failed_analysis(
                            analysis,
                            format!(
                                "u8 solve failed: {primary_error}; f32 solve failed: {fallback_error}"
                            ),
                        ));
                    }
                }
            }
        };

        let provenance = AstrometrySolverProvenance {
            seiza_version: SEIZA_VERSION.to_string(),
            detection_backend,
            star_catalog: catalog_file_signature("stars", &stars_catalog.fingerprint),
            blind_index: blind_index
                .as_ref()
                .map(|loaded| catalog_file_signature("blind_index", &loaded.fingerprint)),
        };
        self.apply_pixel_solution(
            &mut analysis,
            &solved,
            dimensions,
            PixelSolutionMetadata {
                mode,
                provenance,
                capture_time: headers
                    .capture_time
                    .as_ref()
                    .map(|value| value.value.clone()),
                expected_target,
            },
        );
        Ok(analysis)
    }

    fn try_solve_stars(
        &self,
        stars: &[seiza::DetectedStar],
        catalog: &TileCatalog,
        hint: Option<(f64, f64)>,
        scale: Option<f64>,
        dimensions: (u32, u32),
    ) -> Result<
        (
            AstrometrySolveMode,
            seiza::solve::Solution,
            Option<LoadedResource<BlindIndex>>,
        ),
        String,
    > {
        let mut hinted_error = None;
        if let (Some(center), Some(scale_arcsec_px)) = (hint, scale) {
            let solve_hint = seiza::solve::SolveHint {
                center,
                radius_deg: 2.0,
                scale_arcsec_px,
                scale_tolerance: 0.25,
                sip_order: 0,
            };
            match seiza::solve::solve(stars, catalog, &solve_hint, dimensions) {
                Ok(solution) => {
                    return Ok((AstrometrySolveMode::Hinted, solution, None));
                }
                Err(error) => hinted_error = Some(error.to_string()),
            }
        }

        let blind_index = self.load_blind_index().map_err(|error| {
            if let Some(hinted_error) = hinted_error.as_deref() {
                format!("hinted solve failed: {hinted_error}; blind index unavailable: {error}")
            } else {
                format!("blind index unavailable: {error}")
            }
        })?;
        let mut params = seiza::blind::BlindParams {
            index_mag_limit: blind_index.value.index_mag_limit(),
            max_pattern_deg: blind_index.value.max_pattern_deg(),
            ..Default::default()
        };
        if let Some(scale) = scale {
            params.min_scale_arcsec_px = (scale * 0.5).max(0.01);
            params.max_scale_arcsec_px = scale * 1.5;
        }
        seiza::blind::solve_blind(stars, catalog, &blind_index.value, &params, dimensions)
            .map(|solution| (AstrometrySolveMode::Blind, solution, Some(blind_index)))
            .map_err(|error| {
                if let Some(hinted_error) = hinted_error {
                    format!("hinted solve failed: {hinted_error}; blind solve failed: {error}")
                } else {
                    format!("blind solve failed: {error}")
                }
            })
    }

    fn apply_pixel_solution(
        &self,
        analysis: &mut AstrometryAnalysis,
        solved: &seiza::solve::Solution,
        dimensions: (u32, u32),
        metadata: PixelSolutionMetadata,
    ) {
        use seiza::objects::{ObjectQuery, SkyRegion};

        let mut hits = Vec::new();
        let mut annotation_error = None;
        let catalog = match self.load_object_catalog() {
            Ok(catalog) => Some(catalog),
            Err(error) => {
                annotation_error = Some(format!("object catalog unavailable: {error}"));
                None
            }
        };
        if let Some(catalog) = catalog.as_ref() {
            let region = SkyRegion::Polygon {
                vertices: solved.wcs.footprint(dimensions.0, dimensions.1).to_vec(),
            };
            match catalog.value.query_region(
                &region,
                &ObjectQuery {
                    limit: Some(250),
                    ..ObjectQuery::default()
                },
            ) {
                Ok(found) => {
                    hits = found;
                    analysis.catalog_hits =
                        hits.iter().take(100).map(catalog_hit_response).collect();
                }
                Err(error) => annotation_error = Some(error.to_string()),
            }
            analysis.catalog_signature = Some(object_catalog_signature(&catalog.fingerprint));
        }
        let catalog_version = analysis
            .catalog_signature
            .as_ref()
            .and_then(|signature| signature.files.first())
            .map(|file| file.format.clone());
        let solution = solution_response(
            &solved.wcs,
            dimensions,
            solved.matched_stars,
            solved.rms_arcsec,
            SolutionProjection {
                catalog: catalog.as_ref().map(|loaded| loaded.value.as_ref()),
                hits: &hits,
                catalog_version,
                capture_time: metadata.capture_time,
            },
        );
        analysis.pointing = metadata
            .expected_target
            .map(|expected| pointing_result(&solution, &solved.wcs, expected));
        analysis.solution = Some(solution);
        analysis.status = AstrometryAnalysisStatus::Solved;
        analysis.mode = Some(metadata.mode);
        analysis.catalog_scope = Some(AstrometryCatalogScope::SolvedFootprint);
        analysis.catalog_radius_deg = None;
        analysis.solver_provenance = Some(metadata.provenance);
        analysis.computed_at = unix_now();
        analysis.error = annotation_error;
    }

    /// Open configured files lazily and report resource readiness separately
    /// from PSF Guard features that are actually implemented. This performs
    /// only each format's bounded normal open, never an exhaustive validation
    /// scan.
    pub fn capabilities(&self) -> AstrometryCapabilities {
        let objects = capability(
            "objects",
            self.config.objects_path(),
            self.load_object_catalog().map(|loaded| loaded.fingerprint),
        );
        let stars = capability(
            "stars",
            self.config.stars_path(),
            self.load_star_catalog().map(|loaded| loaded.fingerprint),
        );
        let star_identifiers = capability(
            "star_identifiers",
            self.config.star_identifiers_path(),
            self.load_star_identifier_catalog()
                .map(|loaded| loaded.fingerprint),
        );
        let blind_index = capability(
            "blind_index",
            self.config.blind_index_path(),
            self.load_blind_index().map(|loaded| loaded.fingerprint),
        );
        let transients = capability(
            "transients",
            self.config.transients_path(),
            self.load_transient_catalog()
                .map(|loaded| loaded.fingerprint),
        );
        let minor_bodies = capability(
            "minor_bodies",
            self.config.minor_bodies_path(),
            self.load_minor_body_catalog()
                .map(|loaded| loaded.fingerprint),
        );

        // Feature flags describe executable PSF Guard paths, not merely files
        // found on disk. Hinted solving needs the star tiles; blind solving
        // additionally needs the compatible pattern index.
        let features = AstrometryFeatures {
            object_association: objects.available(),
            object_name_search: false,
            stellar_name_search: false,
            hinted_solve: stars.available(),
            blind_solve: stars.available() && blind_index.available(),
            transient_annotations: false,
            minor_body_annotations: false,
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

#[derive(Debug, Clone, Copy)]
enum DetectionPass {
    MtfU8,
    LinearF32,
}

struct PixelSolutionMetadata {
    mode: AstrometrySolveMode,
    provenance: AstrometrySolverProvenance,
    capture_time: Option<String>,
    expected_target: Option<(f64, f64)>,
}

struct SolutionProjection<'a> {
    catalog: Option<&'a ObjectCatalog>,
    hits: &'a [seiza::objects::ObjectHit],
    catalog_version: Option<String>,
    capture_time: Option<String>,
}

fn detect_fits_stars(
    path: &Path,
    pass: DetectionPass,
) -> Result<(Vec<seiza::DetectedStar>, (u32, u32)), String> {
    let fits = seiza_fits::FitsImage::open(path)
        .map_err(|error| format!("failed to decode FITS pixels: {error}"))?;
    let width = u32::try_from(fits.width)
        .map_err(|_| "FITS width exceeds supported dimensions".to_string())?;
    let height = u32::try_from(fits.height)
        .map_err(|_| "FITS height exceeds supported dimensions".to_string())?;
    let config = seiza::DetectConfig {
        backend: match pass {
            DetectionPass::MtfU8 => seiza::DetectBackend::U8,
            DetectionPass::LinearF32 => seiza::DetectBackend::F32,
        },
        max_stars: 300,
        ..Default::default()
    };
    let stars = match pass {
        DetectionPass::MtfU8 => {
            let pixels = fits.stretch_to_u8(&seiza_fits::StretchParams::default());
            drop(fits);
            let image = image::GrayImage::from_raw(width, height, pixels)
                .ok_or_else(|| "FITS dimensions do not match decoded pixels".to_string())?;
            seiza::detect_stars(&image::DynamicImage::ImageLuma8(image), &config)
        }
        DetectionPass::LinearF32 => {
            let pixels = fits.to_luma_f32();
            drop(fits);
            seiza::detect_stars_luma_f32(&pixels, width, height, &config)
        }
    };
    Ok((stars, (width, height)))
}

fn solution_response(
    wcs: &seiza::Wcs,
    dimensions: (u32, u32),
    matched_stars: usize,
    rms_arcsec: f64,
    projection: SolutionProjection<'_>,
) -> AstrometrySolutionResponse {
    let prominence = projection
        .hits
        .iter()
        .map(|hit| (sky_object_key(&hit.object), hit.predicted_prominence))
        .collect::<HashMap<_, _>>();
    let mut objects = if let Some(catalog) = projection.catalog {
        catalog
            .objects_in_footprint(wcs, dimensions)
            .unwrap_or_default()
            .into_iter()
            .map(|placed| {
                let rank = prominence
                    .get(&sky_object_key(&placed.object))
                    .copied()
                    .unwrap_or(0.0);
                overlay_object_response(placed, Some(rank), catalog, wcs)
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    objects.sort_by(|left, right| {
        right
            .prominence
            .unwrap_or(0.0)
            .total_cmp(&left.prominence.unwrap_or(0.0))
    });
    objects.truncate(500);

    let center = wcs.pixel_to_world(
        (f64::from(dimensions.0) - 1.0) / 2.0,
        (f64::from(dimensions.1) - 1.0) / 2.0,
    );
    AstrometrySolutionResponse {
        center_ra_deg: center.0,
        center_dec_deg: center.1,
        pixel_scale_arcsec_per_pixel: wcs.scale_arcsec_per_px(),
        matched_stars,
        rms_arcsec,
        image_width: dimensions.0,
        image_height: dimensions.1,
        wcs: WcsResponse {
            crval: [wcs.crval.0, wcs.crval.1],
            crpix: [wcs.crpix.0, wcs.crpix.1],
            cd: wcs.cd,
            ctype: ["RA---TAN".to_string(), "DEC--TAN".to_string()],
            cunit: ["deg".to_string(), "deg".to_string()],
            radesys: "ICRS".to_string(),
            equinox: 2000.0,
        },
        footprint: wcs
            .footprint(dimensions.0, dimensions.1)
            .into_iter()
            .map(|(ra, dec)| [ra, dec])
            .collect(),
        objects,
        catalog_version: projection.catalog_version,
        capture_time: projection.capture_time,
    }
}

fn wcs_from_response(response: &WcsResponse) -> seiza::Wcs {
    seiza::Wcs {
        crval: (response.crval[0], response.crval[1]),
        crpix: (response.crpix[0], response.crpix[1]),
        cd: response.cd,
        sip: None,
    }
}

fn failed_analysis(mut analysis: AstrometryAnalysis, error: String) -> AstrometryAnalysis {
    analysis.status = AstrometryAnalysisStatus::Failed;
    analysis.mode = None;
    analysis.solution = None;
    analysis.pointing = None;
    analysis.solver_provenance = None;
    analysis.computed_at = unix_now();
    analysis.error = Some(error);
    analysis
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

fn astrometry_cache_path(cache_dir: &Path, image_id: i32) -> PathBuf {
    cache_dir
        .join("astrometry")
        .join(format!("{image_id}.json"))
}

/// Persist a successful pixel-derived solution below the per-database cache
/// directory. Embedded WCS is already durable in the FITS file and is not
/// duplicated here.
pub fn persist_solved_analysis(
    cache_dir: &Path,
    analysis: &AstrometryAnalysis,
) -> Result<(), String> {
    if analysis.status != AstrometryAnalysisStatus::Solved
        || !matches!(
            analysis.mode,
            Some(AstrometrySolveMode::Hinted | AstrometrySolveMode::Blind)
        )
        || analysis.solution.is_none()
    {
        return Err("only pixel-derived solved analyses can be persisted".to_string());
    }
    static CACHE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let path = astrometry_cache_path(cache_dir, analysis.image_id);
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid astrometry cache path {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    let temporary = path.with_extension(format!(
        "json.tmp.{}.{}",
        std::process::id(),
        CACHE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = serde_json::to_vec(analysis)
        .map_err(|error| format!("failed to serialize astrometry solution: {error}"))?;
    let result = std::fs::write(&temporary, bytes)
        .and_then(|_| std::fs::rename(&temporary, &path))
        .map_err(|error| format!("failed to persist {}: {error}", path.display()));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn source_fingerprint(path: &Path) -> Result<AstrometrySourceFingerprint, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize {}: {error}", path.display()))?;
    let metadata = canonical
        .metadata()
        .map_err(|error| format!("failed to stat {}: {error}", canonical.display()))?;
    let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
    let (modified_unix_seconds, modified_subsec_nanos) = unix_time_parts(modified);
    Ok(AstrometrySourceFingerprint {
        canonical_path: canonical.to_string_lossy().into_owned(),
        size_bytes: metadata.len(),
        modified_unix_seconds,
        modified_subsec_nanos,
    })
}

fn object_catalog_signature(fingerprint: &ResourceFingerprint) -> AstrometryCatalogSignature {
    AstrometryCatalogSignature {
        bundle_version: None,
        files: vec![catalog_file_signature("objects", fingerprint)],
    }
}

fn catalog_file_signature(
    name: &str,
    fingerprint: &ResourceFingerprint,
) -> AstrometryCatalogFileSignature {
    let (modified_unix_seconds, modified_subsec_nanos) = unix_time_parts(fingerprint.modified);
    AstrometryCatalogFileSignature {
        name: name.to_string(),
        path: fingerprint.canonical_path.to_string_lossy().into_owned(),
        format: fingerprint.format(),
        size_bytes: fingerprint.size_bytes,
        modified_unix_seconds,
        modified_subsec_nanos,
        sha256: None,
    }
}

fn identity(object: &seiza::objects::SkyObject) -> CatalogObjectIdentity {
    CatalogObjectIdentity {
        stable_id: object.metadata.id.clone(),
        source: object.metadata.source.clone(),
        aliases: object.metadata.aliases.clone(),
        parent_ids: object.metadata.parent_ids.clone(),
        alternate_ids: object.metadata.alternate_ids.clone(),
        alternate_sources: object.metadata.alternate_sources.clone(),
    }
}

fn catalog_hit_response(hit: &seiza::objects::ObjectHit) -> CatalogHitResponse {
    CatalogHitResponse {
        identity: identity(&hit.object),
        name: hit.object.name.clone(),
        common_name: hit.object.common_name.clone(),
        kind: hit.object.kind.as_str().to_string(),
        mag: hit.object.mag,
        major_arcmin: hit.object.major_arcmin,
        minor_arcmin: hit.object.minor_arcmin,
        position_angle_deg: hit.object.position_angle_deg,
        ra_deg: hit.object.ra,
        dec_deg: hit.object.dec,
        center_inside: hit.center_inside,
        extent_only: hit.extent_only,
        distance_from_center_deg: hit.distance_from_center_deg,
        predicted_prominence: hit.predicted_prominence,
    }
}

fn overlay_object_response(
    placed: seiza::objects::PlacedObject,
    prominence: Option<f64>,
    catalog: &seiza::objects::ObjectCatalog,
    wcs: &seiza::Wcs,
) -> OverlayObjectResponse {
    let outlines = projected_outlines(catalog, &placed.object.metadata.id, wcs);
    OverlayObjectResponse {
        identity: identity(&placed.object),
        name: placed.object.name.clone(),
        common_name: placed.object.common_name.clone(),
        kind: placed.object.kind.as_str().to_string(),
        mag: placed.object.mag,
        x: placed.x,
        y: placed.y,
        semi_major_px: placed.semi_major_px,
        semi_minor_px: placed.semi_minor_px,
        angle_deg: placed.angle_deg,
        ra_deg: placed.object.ra,
        dec_deg: placed.object.dec,
        prominence,
        discovered: None,
        near_capture: None,
        distance_au: None,
        direction_pa_deg: None,
        direction_angle_deg: None,
        outlines,
    }
}

fn projected_outlines(
    catalog: &seiza::objects::ObjectCatalog,
    canonical_id: &str,
    wcs: &seiza::Wcs,
) -> Vec<OverlayOutlineResponse> {
    use seiza::objects::{GeometryData, GeometryQuality, GeometryRole};

    let Ok(geometries) = catalog.geometries(canonical_id) else {
        return Vec::new();
    };
    geometries
        .into_iter()
        .filter_map(|geometry| {
            let GeometryData::OutlineSet { level, contours } = geometry.data else {
                return None;
            };
            let contours = contours
                .into_iter()
                .filter_map(|contour| {
                    let points = contour
                        .vertices
                        .into_iter()
                        .map(|(ra, dec)| wcs.world_to_pixel(ra, dec).map(|(x, y)| [x, y]))
                        .collect::<Option<Vec<_>>>()?;
                    let minimum_points = if contour.closed { 3 } else { 2 };
                    (points.len() >= minimum_points).then_some(OverlayContourResponse {
                        closed: contour.closed,
                        points,
                    })
                })
                .collect::<Vec<_>>();
            (!contours.is_empty()).then_some(OverlayOutlineResponse {
                geometry_id: geometry.id,
                source_record_id: geometry.source_record_id,
                role: match geometry.role {
                    GeometryRole::CatalogExtent => "catalog-extent",
                    GeometryRole::PreferredRender => "preferred-render",
                    GeometryRole::FallbackExtent => "fallback-extent",
                    GeometryRole::BrightnessLevel => "brightness-level",
                    GeometryRole::Component => "component",
                }
                .to_string(),
                quality: match geometry.quality {
                    GeometryQuality::Catalog => "catalog",
                    GeometryQuality::Curated => "curated",
                    GeometryQuality::Estimated => "estimated",
                    GeometryQuality::Derived => "derived",
                }
                .to_string(),
                level,
                contours,
            })
        })
        .collect()
}

fn sky_object_key(object: &seiza::objects::SkyObject) -> String {
    if object.metadata.id.is_empty() {
        format!("{}:{:.8}:{:.8}", object.name, object.ra, object.dec)
    } else {
        object.metadata.id.clone()
    }
}

fn pointing_result(
    solution: &AstrometrySolutionResponse,
    wcs: &seiza::Wcs,
    expected: (f64, f64),
) -> PointingResult {
    let delta_ra = (solution.center_ra_deg - expected.0 + 540.0).rem_euclid(360.0) - 180.0;
    let east_offset_arcsec = delta_ra * expected.1.to_radians().cos() * 3600.0;
    let north_offset_arcsec = (solution.center_dec_deg - expected.1) * 3600.0;
    let separation_arcsec = angular_separation_deg(
        solution.center_ra_deg,
        solution.center_dec_deg,
        expected.0,
        expected.1,
    ) * 3600.0;
    let target_pixel = wcs.world_to_pixel(expected.0, expected.1);
    let edge_margin = target_pixel.map(|(x, y)| {
        x.min(y)
            .min(f64::from(solution.image_width) - 1.0 - x)
            .min(f64::from(solution.image_height) - 1.0 - y)
    });
    PointingResult {
        expected_ra_deg: expected.0,
        expected_dec_deg: expected.1,
        east_offset_arcsec,
        north_offset_arcsec,
        separation_arcsec,
        target_in_frame: edge_margin.is_some_and(|margin| margin >= 0.0),
        target_edge_margin_px: edge_margin,
    }
}

fn angular_separation_deg(ra1: f64, dec1: f64, ra2: f64, dec2: f64) -> f64 {
    let (ra1, dec1, ra2, dec2) = (
        ra1.to_radians(),
        dec1.to_radians(),
        ra2.to_radians(),
        dec2.to_radians(),
    );
    (dec1.sin() * dec2.sin() + dec1.cos() * dec2.cos() * (ra1 - ra2).cos())
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
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
    cache: &ResourceCache<T>,
    path: AstrometryResourcePath,
    open: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<LoadedResource<T>, String> {
    let path = path
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "resource is not configured".to_string())?;
    let fingerprint = resource_fingerprint(&path)?;
    if let Some(cached) = cache.read().unwrap().as_ref()
        && cached.fingerprint == fingerprint
    {
        return Ok(cached.clone());
    }

    let mut guard = cache.write().unwrap();
    let fingerprint = resource_fingerprint(&path)?;
    if let Some(cached) = guard.as_ref()
        && cached.fingerprint == fingerprint
    {
        return Ok(cached.clone());
    }

    let value = Arc::new(
        open(&fingerprint.canonical_path)
            .map_err(|error| format!("{}: {error}", fingerprint.canonical_path.display()))?,
    );
    let confirmed = resource_fingerprint(&path)?;
    if confirmed != fingerprint {
        return Err(format!(
            "resource changed while it was being opened: {}",
            path.display()
        ));
    }

    let loaded = LoadedResource { value, fingerprint };
    *guard = Some(loaded.clone());
    Ok(loaded)
}

fn capability(
    name: &str,
    path: AstrometryResourcePath,
    opened: Result<ResourceFingerprint, String>,
) -> AstrometryResourceCapability {
    let path = match path {
        Ok(Some(path)) => path,
        Ok(None) => {
            return AstrometryResourceCapability {
                name: name.to_string(),
                status: AstrometryResourceStatus::NotConfigured,
                path: None,
                format: None,
                size_bytes: None,
                modified_unix_seconds: None,
                modified_subsec_nanos: None,
                error: None,
            };
        }
        Err(error) => {
            return AstrometryResourceCapability {
                name: name.to_string(),
                status: AstrometryResourceStatus::Missing,
                path: data_path_error_path(&error),
                format: None,
                size_bytes: None,
                modified_unix_seconds: None,
                modified_subsec_nanos: None,
                error: Some(error.to_string()),
            };
        }
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
            modified_subsec_nanos: None,
            error: Some("configured resource file does not exist".to_string()),
        };
    }
    match opened {
        Ok(fingerprint) => {
            let (modified_unix_seconds, modified_subsec_nanos) =
                unix_time_parts(fingerprint.modified);
            AstrometryResourceCapability {
                name: name.to_string(),
                status: AstrometryResourceStatus::Available,
                path: Some(fingerprint.canonical_path.to_string_lossy().into_owned()),
                format: Some(fingerprint.format()),
                size_bytes: Some(fingerprint.size_bytes),
                modified_unix_seconds: Some(modified_unix_seconds),
                modified_subsec_nanos: Some(modified_subsec_nanos),
                error: None,
            }
        }
        Err(error) => {
            let metadata = path.metadata().ok();
            let size_bytes = metadata.as_ref().map(|metadata| metadata.len());
            let modified = metadata
                .and_then(|metadata| metadata.modified().ok())
                .map(unix_time_parts);
            AstrometryResourceCapability {
                name: name.to_string(),
                status: AstrometryResourceStatus::Invalid,
                path: Some(path_string),
                format: read_magic(&path).ok(),
                size_bytes,
                modified_unix_seconds: modified.map(|parts| parts.0),
                modified_subsec_nanos: modified.map(|parts| parts.1),
                error: Some(error),
            }
        }
    }
}

fn data_path_error_path(error: &seiza::data_paths::DataPathError) -> Option<String> {
    use seiza::data_paths::DataPathError;

    match error {
        DataPathError::NotFoundInDirectory { path, .. }
        | DataPathError::Missing { path, .. }
        | DataPathError::EnvVar { path, .. } => Some(path.to_string_lossy().into_owned()),
        _ => None,
    }
}

fn resource_fingerprint(path: &Path) -> Result<ResourceFingerprint, String> {
    const HEADER_BYTES: usize = 32;

    let canonical_path = path
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize {}: {error}", path.display()))?;
    let metadata = canonical_path
        .metadata()
        .map_err(|error| format!("failed to stat {}: {error}", canonical_path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "resource path is not a file: {}",
            canonical_path.display()
        ));
    }
    let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
    let mut file = File::open(&canonical_path)
        .map_err(|error| format!("failed to open {}: {error}", canonical_path.display()))?;
    let mut bytes = [0u8; HEADER_BYTES];
    let read = file
        .read(&mut bytes)
        .map_err(|error| format!("failed to read {}: {error}", canonical_path.display()))?;

    Ok(ResourceFingerprint {
        canonical_path,
        size_bytes: metadata.len(),
        modified,
        header: bytes[..read].to_vec(),
    })
}

impl ResourceFingerprint {
    fn format(&self) -> String {
        let end = self.header.len().min(8);
        if end == 0 {
            "unknown".to_string()
        } else {
            format_magic(&self.header[..end])
        }
    }
}

fn format_magic(bytes: &[u8]) -> String {
    if bytes == b"SEIZAOB\0" {
        // Seiza's extensible object-catalog container uses a stable envelope
        // magic and reports its public schema generation as v4.
        "SEIZAOB4".to_string()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn unix_time_parts(time: SystemTime) -> (u64, u32) {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| (duration.as_secs(), duration.subsec_nanos()))
        .unwrap_or((0, 0))
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn read_magic(path: &Path) -> std::io::Result<String> {
    let mut bytes = [0u8; 8];
    File::open(path)?.read_exact(&mut bytes)?;
    Ok(format_magic(&bytes))
}

fn validate_resource(
    name: &str,
    path: AstrometryResourcePath,
    validate: impl FnOnce() -> Result<(), String>,
) -> AstrometryResourceValidation {
    let path = match path {
        Ok(Some(path)) => path,
        Ok(None) => {
            return AstrometryResourceValidation {
                name: name.to_string(),
                status: AstrometryResourceStatus::NotConfigured,
                path: None,
                validated: false,
                error: None,
            };
        }
        Err(error) => {
            return AstrometryResourceValidation {
                name: name.to_string(),
                status: AstrometryResourceStatus::Missing,
                path: data_path_error_path(&error),
                validated: false,
                error: Some(error.to_string()),
            };
        }
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
    use seiza::objects::{
        GeometryData, GeometryQuality, GeometryRole, ObjectCatalog, ObjectCatalogData,
        ObjectContour, ObjectDetails, ObjectGeometry, ObjectKind, ObjectMetadata, SkyObject,
    };

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
    fn v4_outlines_project_with_provenance_and_unknown_angle() {
        let directory = tempfile::tempdir().unwrap();
        let objects_path = directory.path().join("objects.bin");
        let mut object = test_object();
        object.kind = ObjectKind::Nebula;
        object.ra = 10.0;
        object.dec = 20.0;
        object.major_arcmin = Some(30.0);
        object.minor_arcmin = Some(10.0);
        object.position_angle_deg = None;
        object.name = "NGC 1".to_string();
        object.common_name = "Test Nebula".to_string();
        object.metadata.id = "openngc:NGC1".to_string();

        let mut details = ObjectDetails::from_canonical(&object);
        details.geometries.push(ObjectGeometry {
            id: "openngc:NGC1#outline-1".to_string(),
            source_record_id: "openngc:NGC1".to_string(),
            role: GeometryRole::BrightnessLevel,
            quality: GeometryQuality::Catalog,
            method: "OpenNGC outline".to_string(),
            evidence: String::new(),
            data: GeometryData::OutlineSet {
                level: Some("1".to_string()),
                contours: vec![ObjectContour {
                    closed: true,
                    vertices: vec![(9.99, 19.99), (10.01, 19.99), (10.0, 20.01)],
                }],
            },
        });
        ObjectCatalog::from_data(ObjectCatalogData {
            objects: vec![object],
            details: vec![details],
            provenance: Default::default(),
        })
        .unwrap()
        .write_to(&objects_path)
        .unwrap();

        let catalog = ObjectCatalog::open(&objects_path).unwrap();
        let wcs = seiza::Wcs {
            crval: (10.0, 20.0),
            crpix: (100.0, 100.0),
            cd: [[-0.001, 0.0], [0.0, -0.001]],
            sip: None,
        };
        let placed = catalog
            .objects_in_footprint(&wcs, (200, 200))
            .unwrap()
            .into_iter()
            .find(|placed| placed.object.metadata.id == "openngc:NGC1")
            .unwrap();
        let response = overlay_object_response(placed, Some(0.9), &catalog, &wcs);

        assert_eq!(response.angle_deg, None);
        assert_eq!(response.outlines.len(), 1);
        assert_eq!(response.outlines[0].role, "brightness-level");
        assert_eq!(response.outlines[0].quality, "catalog");
        assert_eq!(response.outlines[0].level.as_deref(), Some("1"));
        assert_eq!(response.outlines[0].contours[0].points.len(), 3);
        let json = serde_json::to_value(response).unwrap();
        assert!(json["angle_deg"].is_null());
        assert_eq!(json["outlines"][0]["geometry_id"], "openngc:NGC1#outline-1");
    }

    #[test]
    fn seiza_resolvers_select_bundle_resources_below_data_dir() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("objects.bin"), b"objects").unwrap();
        std::fs::write(directory.path().join("stars-lite-tycho2.bin"), b"lite").unwrap();
        std::fs::write(directory.path().join("stars-gaia.bin"), b"gaia").unwrap();
        std::fs::write(directory.path().join("custom-blind.idx"), b"index").unwrap();
        std::fs::write(
            directory.path().join("stars-lite-tycho2.ids.bin"),
            b"identifiers",
        )
        .unwrap();
        std::fs::write(directory.path().join("transients.bin"), b"transients").unwrap();
        std::fs::write(directory.path().join("minor-bodies.bin"), b"minor bodies").unwrap();

        let config = AstrometryConfig {
            data_dir: Some(directory.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        assert_eq!(
            config.objects_path().unwrap().unwrap(),
            directory.path().join("objects.bin")
        );
        assert_eq!(
            config.stars_path().unwrap().unwrap(),
            directory.path().join("stars-gaia.bin"),
            "Seiza should select the deepest catalog in the directory"
        );
        assert_eq!(
            config.blind_index_path().unwrap().unwrap(),
            directory.path().join("custom-blind.idx")
        );
        assert_eq!(
            config.star_identifiers_path().unwrap().unwrap(),
            directory.path().join("stars-lite-tycho2.ids.bin")
        );
        assert_eq!(
            config.transients_path().unwrap().unwrap(),
            directory.path().join("transients.bin")
        );
        assert_eq!(
            config.minor_bodies_path().unwrap().unwrap(),
            directory.path().join("minor-bodies.bin")
        );

        std::fs::write(directory.path().join("custom-stars.bin"), b"custom").unwrap();
        let overridden = AstrometryConfig {
            data_dir: config.data_dir.clone(),
            stars: Some("custom-stars.bin".to_string()),
            ..Default::default()
        };
        assert_eq!(
            overridden.stars_path().unwrap().unwrap(),
            directory.path().join("custom-stars.bin"),
            "relative overrides remain relative to data_dir"
        );
    }

    #[test]
    fn missing_data_directory_is_reported_as_missing() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing-catalogs");
        let capabilities = AstrometryContext::new(AstrometryConfig {
            data_dir: Some(missing.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .capabilities();
        assert_eq!(
            capabilities.resources.objects.status,
            AstrometryResourceStatus::Missing
        );
        assert_eq!(
            capabilities.resources.objects.path.as_deref(),
            Some(missing.to_string_lossy().as_ref())
        );
        assert!(
            capabilities.resources.objects.error.is_some(),
            "the upstream resolver error should remain visible"
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
    fn indexed_v4_object_catalog_is_available_and_validates() {
        let directory = tempfile::tempdir().unwrap();
        let objects_path = directory.path().join("objects.bin");
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
            Some("SEIZAOB4")
        );
        assert!(capabilities.features.object_association);
        assert!(!capabilities.features.object_name_search);
        assert!(!capabilities.features.stellar_name_search);
        assert!(!capabilities.features.hinted_solve);
        assert!(!capabilities.features.blind_solve);
        assert!(!capabilities.features.transient_annotations);
        assert!(!capabilities.features.minor_body_annotations);

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
    fn indexed_v3_object_catalog_remains_readable() {
        let directory = tempfile::tempdir().unwrap();
        let objects_path = directory.path().join("objects-v3.bin");
        ObjectCatalog::new(vec![test_object()])
            .write_v3_to(&objects_path)
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
        assert!(context.try_validate_all().unwrap().all_configured_valid);
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
        let objects_path = directory.path().join("objects.bin");
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
    fn cached_resources_reload_when_the_file_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("resource.txt");
        let cache: ResourceCache<String> = RwLock::new(None);
        std::fs::write(&path, "first").unwrap();

        let first = load_cached(&cache, Ok(Some(path.clone())), |path| {
            std::fs::read_to_string(path).map_err(|error| error.to_string())
        })
        .unwrap();
        assert_eq!(first.value.as_str(), "first");

        std::fs::write(&path, "replacement with a different size").unwrap();
        let second = load_cached(&cache, Ok(Some(path)), |path| {
            std::fs::read_to_string(path).map_err(|error| error.to_string())
        })
        .unwrap();

        assert_eq!(second.value.as_str(), "replacement with a different size");
        assert!(!Arc::ptr_eq(&first.value, &second.value));
        assert_ne!(first.fingerprint, second.fingerprint);
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
            catalog_scope: Some(AstrometryCatalogScope::NearbyTarget),
            catalog_radius_deg: Some(1.0),
            pointing: None,
            source_fingerprint: AstrometrySourceFingerprint {
                canonical_path: "/images/frame.fits".to_string(),
                size_bytes: 1234,
                modified_unix_seconds: 5678,
                modified_subsec_nanos: 9,
            },
            catalog_signature: None,
            solver_provenance: None,
            computed_at: 9999,
            error: None,
        };

        let json = serde_json::to_value(&analysis).unwrap();
        assert_eq!(json["status"], "catalog_only");
        assert_eq!(json["hint_source"]["source"], "fits_header");
        assert_eq!(json["expected_source"]["source"], "target_scheduler");
        assert!(json.get("solution").is_none());
    }

    #[test]
    fn target_scheduler_ra_hours_are_converted_at_the_boundary() {
        let (ra_deg, dec_deg) = target_scheduler_coordinates(16.694898333333335, 36.46131943888889)
            .expect("valid Target Scheduler coordinates");
        assert!((ra_deg - 250.423475).abs() < 1e-9);
        assert!((dec_deg - 36.46131943888889).abs() < 1e-12);
        assert_eq!(target_scheduler_coordinates(24.0, 0.0), Some((0.0, 0.0)));
        assert_eq!(target_scheduler_coordinates(25.0, 0.0), None);
    }
}
