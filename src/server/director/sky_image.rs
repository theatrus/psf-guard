//! Survey imagery behind the framing view: fixed-provider HiPS2FITS cutouts,
//! fetched off the request path and cached under the cache root, and
//! N.I.N.A.'s offline sky maps rendered from their tiles under
//! `<cache>/director/sky-maps`. A cutout is a composition aid; it proves
//! nothing about pointing, grade or coverage.

use super::*;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fmt::Write as _,
    path::PathBuf,
    sync::OnceLock,
    time::{Duration, Instant},
};

pub const DEFAULT_BASE_URL: &str = "https://alasky.cds.unistra.fr/hips-image-services/hips2fits";
/// CDS Sesame, the same name resolver N.I.N.A.'s framing assistant uses.
pub const DEFAULT_RESOLVER_URL: &str = "https://cds.unistra.fr/cgi-bin/nph-sesame/-oI/A";
const MAX_RESOLVE_BYTES: usize = 64 * 1024;
const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(45);
const FAILURE_MEMORY: Duration = Duration::from_secs(60);
const CONCURRENT_FETCHES: usize = 2;
/// How long a listing of the offline map folders is trusted before the
/// folder is read again, so an install shows up without a restart.
const SKY_MAPS_REFRESH: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SurveyKind {
    Broadband,
    Narrowband,
    Panorama,
}

/// The same choices N.I.N.A.'s framing assistant offers, by HiPS identifier.
/// H-alpha, O III and S II layers stay distinct; nothing substitutes for them.
#[derive(Clone, Copy, Serialize)]
pub struct Survey {
    pub id: &'static str,
    pub name: &'static str,
    pub hips: &'static str,
    pub kind: SurveyKind,
    pub bandpass: &'static str,
    pub attribution: &'static str,
}

pub const SURVEYS: &[Survey] = &[
    Survey {
        id: "dss2_color",
        name: "DSS2 color",
        hips: "CDS/P/DSS2/color",
        kind: SurveyKind::Broadband,
        bandpass: "Blue, red and near infrared plates",
        attribution: "Digitized Sky Survey 2, STScI/AAO/CDS",
    },
    Survey {
        id: "dss2_red",
        name: "DSS2 red",
        hips: "CDS/P/DSS2/red",
        kind: SurveyKind::Broadband,
        bandpass: "Red plates",
        attribution: "Digitized Sky Survey 2, STScI/AAO/CDS",
    },
    Survey {
        id: "dss2_blue",
        name: "DSS2 blue",
        hips: "CDS/P/DSS2/blue",
        kind: SurveyKind::Broadband,
        bandpass: "Blue plates",
        attribution: "Digitized Sky Survey 2, STScI/AAO/CDS",
    },
    Survey {
        id: "dss2_nir",
        name: "DSS2 near infrared",
        hips: "CDS/P/DSS2/NIR",
        kind: SurveyKind::Broadband,
        bandpass: "Near infrared plates",
        attribution: "Digitized Sky Survey 2, STScI/AAO/CDS",
    },
    Survey {
        id: "sdss9_color",
        name: "SDSS DR9 color",
        hips: "CDS/P/SDSS9/color",
        kind: SurveyKind::Broadband,
        bandpass: "g, r, i",
        attribution: "Sloan Digital Sky Survey DR9, sdss.org, via CDS",
    },
    Survey {
        id: "desi_legacy_color",
        name: "DESI Legacy Surveys color",
        hips: "CDS/P/DESI-Legacy-Surveys/DR10/color",
        kind: SurveyKind::Broadband,
        bandpass: "g, r, z",
        attribution: "DESI Legacy Imaging Surveys DR10, legacysurvey.org, via CDS",
    },
    Survey {
        id: "skymapper_dr4_color",
        name: "SkyMapper DR4 color",
        hips: "CDS/P/Skymapper/DR4/color",
        kind: SurveyKind::Broadband,
        bandpass: "Southern sky, g, r, i",
        attribution: "SkyMapper Southern Survey DR4, via CDS",
    },
    Survey {
        id: "twomass_color",
        name: "2MASS color",
        hips: "CDS/P/2MASS/color",
        kind: SurveyKind::Broadband,
        bandpass: "J, H, K infrared",
        attribution: "Two Micron All Sky Survey, UMass/IPAC, via CDS",
    },
    Survey {
        id: "cta_fram_color",
        name: "CTA-FRAM sky survey color",
        hips: "fzu.cz/P/CTA-FRAM/survey/color",
        kind: SurveyKind::Broadband,
        bandpass: "Wide-field color",
        attribution: "CTA-FRAM, FZU Prague",
    },
    Survey {
        id: "mellinger_color",
        name: "Mellinger Milky Way panorama",
        hips: "CDS/P/Mellinger/color",
        kind: SurveyKind::Panorama,
        bandpass: "Wide-field color",
        attribution: "Axel Mellinger, Milky Way Panorama 2.0, via CDS",
    },
    Survey {
        id: "finkbeiner_halpha",
        name: "Finkbeiner H-alpha composite",
        hips: "CDS/P/Finkbeiner",
        kind: SurveyKind::Narrowband,
        bandpass: "H-alpha, about 6 arcminute resolution",
        attribution: "Finkbeiner 2003 composite of VTSS, SHASSA and WHAM, via CDS",
    },
    Survey {
        id: "nsns_halpha",
        name: "Northern Sky Narrowband Survey H-alpha",
        hips: "simg.de/P/NSNS/DR0_2/halpha8",
        kind: SurveyKind::Narrowband,
        bandpass: "H-alpha, northern sky",
        attribution: "Northern Sky Narrowband Survey DR0.2, simg.de",
    },
    Survey {
        id: "nsns_oiii",
        name: "Northern Sky Narrowband Survey O III",
        hips: "simg.de/P/NSNS/DR0_2/oiii8",
        kind: SurveyKind::Narrowband,
        bandpass: "O III, northern sky",
        attribution: "Northern Sky Narrowband Survey DR0.2, simg.de",
    },
    Survey {
        id: "nsns_ohs",
        name: "Northern Sky Narrowband Survey O III, H-alpha, S II",
        hips: "simg.de/P/NSNS/DR0_2/ohs8",
        kind: SurveyKind::Narrowband,
        bandpass: "O III, H-alpha and S II as color, northern sky",
        attribution: "Northern Sky Narrowband Survey DR0.2, simg.de",
    },
    Survey {
        id: "nsns_halpha_continuum",
        name: "Northern Sky Narrowband Survey H-alpha and continuum",
        hips: "simg.de/P/NSNS/DR0_2/hbr8",
        kind: SurveyKind::Narrowband,
        bandpass: "H-alpha over broadband, northern sky",
        attribution: "Northern Sky Narrowband Survey DR0.2, simg.de",
    },
    Survey {
        id: "nsns_rgb",
        name: "Northern Sky Narrowband Survey RGB continuum",
        hips: "simg.de/P/NSNS/DR0_2/rgb8",
        kind: SurveyKind::Broadband,
        bandpass: "Continuum color, northern sky",
        attribution: "Northern Sky Narrowband Survey DR0.2, simg.de",
    },
    Survey {
        id: "nsns_dr01_color",
        name: "Northern Sky Narrowband Survey DR0.1 color",
        hips: "simg.de/P/NSNS/DR0_1/tc8",
        kind: SurveyKind::Narrowband,
        bandpass: "Narrowband color, northern sky",
        attribution: "Northern Sky Narrowband Survey DR0.1, simg.de",
    },
];

/// A layer the framing view can ask for: one of the HiPS surveys above or an
/// offline sky map found on disk.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SurveyInfo {
    pub id: String,
    pub name: String,
    /// Empty for an offline map.
    pub hips: String,
    pub kind: SurveyKind,
    pub bandpass: String,
    pub attribution: String,
    /// Rendered from tiles on this server; no network is used.
    pub offline: bool,
}

impl From<&Survey> for SurveyInfo {
    fn from(survey: &Survey) -> Self {
        Self {
            id: survey.id.to_owned(),
            name: survey.name.to_owned(),
            hips: survey.hips.to_owned(),
            kind: survey.kind,
            bandpass: survey.bandpass.to_owned(),
            attribution: survey.attribution.to_owned(),
            offline: false,
        }
    }
}

impl From<&crate::sky_maps::SkyMap> for SurveyInfo {
    fn from(map: &crate::sky_maps::SkyMap) -> Self {
        Self {
            id: map.id.clone(),
            name: map.name.clone(),
            hips: String::new(),
            kind: match map.kind {
                crate::sky_maps::Kind::Broadband => SurveyKind::Broadband,
                crate::sky_maps::Kind::Narrowband => SurveyKind::Narrowband,
            },
            bandpass: map.bandpass.clone(),
            attribution: map.attribution.clone(),
            offline: true,
        }
    }
}

pub fn builtin_surveys() -> Vec<SurveyInfo> {
    SURVEYS.iter().map(SurveyInfo::from).collect()
}

/// One requested view. Bounds keep the provider request small and the cache
/// finite; a framing view never needs more than a few megapixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Cutout {
    pub survey: String,
    /// The provider's identifier; `None` for an offline map.
    pub hips: Option<String>,
    pub ra_degrees: f64,
    pub dec_degrees: f64,
    /// Angular width of the image.
    pub fov_degrees: f64,
    pub width_px: u32,
    pub height_px: u32,
    /// Position angle of the image's up direction, east of north.
    pub rotation_degrees: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CutoutQuery {
    survey: String,
    ra: f64,
    dec: f64,
    fov: f64,
    #[serde(default = "default_size")]
    width: u32,
    #[serde(default = "default_size")]
    height: u32,
    #[serde(default)]
    rotation: f64,
}
fn default_size() -> u32 {
    1024
}

impl Cutout {
    pub(super) fn from_query(query: &CutoutQuery, known: &[SurveyInfo]) -> Result<Self, Error> {
        let survey = known
            .iter()
            .find(|survey| survey.id == query.survey)
            .ok_or(Error::Invalid)?;
        let finite = |v: f64, lo: f64, hi: f64| v.is_finite() && (lo..=hi).contains(&v);
        if !finite(query.ra, 0.0, 360.0)
            || query.ra >= 360.0
            || !finite(query.dec, -90.0, 90.0)
            // Up to a hemisphere and a bit, so the framing view has a picture
            // behind it at every zoom.
            || !finite(query.fov, 0.02, 180.0)
            || !(64..=2048).contains(&query.width)
            || !(64..=2048).contains(&query.height)
            || !finite(query.rotation, 0.0, 360.0)
            || query.rotation >= 360.0
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            survey: survey.id.clone(),
            hips: (!survey.offline).then(|| survey.hips.clone()),
            ra_degrees: query.ra,
            dec_degrees: query.dec,
            fov_degrees: query.fov,
            width_px: query.width,
            height_px: query.height,
            rotation_degrees: query.rotation,
        })
    }

    /// Round so a nudge below display precision reuses the cached image.
    /// The key names the projection: v2 images are stereographic, which the
    /// framing stage draws in, so a v1 tangent-plane image is never reused.
    pub fn digest(&self) -> String {
        let key = format!(
            "sky-cutout-v2|STG|{}|{:.5}|{:.5}|{:.5}|{}|{}|{:.2}",
            self.survey,
            self.ra_degrees,
            self.dec_degrees,
            self.fov_degrees,
            self.width_px,
            self.height_px,
            self.rotation_degrees
        );
        let mut digest = String::with_capacity(64);
        for byte in Sha256::digest(key.as_bytes()) {
            write!(digest, "{byte:02x}").expect("writing to a String cannot fail");
        }
        digest
    }

    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("hips", self.hips.clone().unwrap_or_default()),
            ("width", self.width_px.to_string()),
            ("height", self.height_px.to_string()),
            ("fov", format!("{:.6}", self.fov_degrees)),
            // Stereographic, like the framing stage and the offline renderer:
            // it matches the tangent plane at the center and stays bounded
            // out to a hemisphere, so one projection serves every zoom.
            ("projection", "STG".to_owned()),
            ("coordsys", "icrs".to_owned()),
            ("ra", format!("{:.6}", self.ra_degrees)),
            ("dec", format!("{:.6}", self.dec_degrees)),
            ("rotation_angle", format!("{:.3}", self.rotation_degrees)),
            ("format", "jpg".to_owned()),
        ]
    }
}

enum Job {
    Generating,
    Failed { message: String, at: Instant },
}

/// The offline maps as last read from disk, shared with render tasks.
type MapList = Arc<Vec<Arc<crate::sky_maps::SkyMap>>>;

pub struct SkyImageService {
    client: reqwest::Client,
    base_url: String,
    resolver_url: String,
    cache_dir: PathBuf,
    /// Folders of N.I.N.A. offline sky maps, one per layer.
    sky_maps_root: PathBuf,
    maps: Mutex<Option<(Instant, MapList)>>,
    jobs: Mutex<HashMap<String, Job>>,
    admission: Arc<Semaphore>,
}

/// A resolved name: where the catalog puts it, never a pointing solution.
#[derive(Serialize)]
pub(super) struct Resolved {
    query: String,
    name: String,
    ra_degrees: f64,
    dec_degrees: f64,
    source: &'static str,
}

impl SkyImageService {
    pub fn new(cache_root: &FilePath, base_url: &str) -> Self {
        Self::with_resolver(cache_root, base_url, DEFAULT_RESOLVER_URL)
    }

    pub fn with_resolver(cache_root: &FilePath, base_url: &str, resolver_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("psf-guard/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("sky image HTTP client should build");
        Self {
            client,
            base_url: base_url.to_owned(),
            resolver_url: resolver_url.to_owned(),
            cache_dir: cache_root.join("director").join("sky"),
            sky_maps_root: crate::sky_maps::root_for(cache_root),
            maps: Mutex::new(None),
            jobs: Mutex::new(HashMap::new()),
            admission: Arc::new(Semaphore::new(CONCURRENT_FETCHES)),
        }
    }

    /// Look for offline maps somewhere else than under the cache root.
    #[cfg(test)]
    pub fn with_sky_maps_root(mut self, root: &FilePath) -> Self {
        self.sky_maps_root = root.to_path_buf();
        self
    }

    /// The offline maps on disk, reread every half minute so a fresh install
    /// appears without a restart. Reading the indexes is quick; the tiles
    /// are opened only when a view is rendered.
    fn maps(&self) -> MapList {
        let mut guard = self.maps.lock().expect("sky map list");
        if let Some((at, maps)) = guard.as_ref()
            && at.elapsed() < SKY_MAPS_REFRESH
        {
            return maps.clone();
        }
        let maps = Arc::new(
            crate::sky_maps::discover(&self.sky_maps_root)
                .into_iter()
                .map(Arc::new)
                .collect::<Vec<_>>(),
        );
        *guard = Some((Instant::now(), maps.clone()));
        maps
    }

    /// Every layer on offer: the HiPS surveys, then the offline maps.
    pub fn surveys(&self) -> Vec<SurveyInfo> {
        let mut list = builtin_surveys();
        list.extend(self.maps().iter().map(|map| SurveyInfo::from(map.as_ref())));
        list
    }

    pub(super) fn cutout_from_query(&self, query: &CutoutQuery) -> Result<Cutout, Error> {
        Cutout::from_query(query, &self.surveys())
    }

    fn map_for(&self, survey: &str) -> Option<Arc<crate::sky_maps::SkyMap>> {
        self.maps().iter().find(|map| map.id == survey).cloned()
    }

    fn path_for(&self, digest: &str) -> PathBuf {
        self.cache_dir.join(format!("{digest}.jpg"))
    }

    /// Serve the cached image, or start one fetch and say so. Never fetches in
    /// the request; a failed fetch is remembered briefly so the view can say why.
    pub(super) async fn respond(self: Arc<Self>, cutout: Cutout) -> Response {
        let digest = cutout.digest();
        let path = self.path_for(&digest);
        if let Ok(bytes) = tokio::fs::read(&path).await {
            return (
                StatusCode::OK,
                [
                    (CONTENT_TYPE, "image/jpeg"),
                    (CACHE_CONTROL, "private, max-age=86400"),
                ],
                bytes,
            )
                .into_response();
        }
        {
            let mut jobs = self.jobs.lock().expect("sky image job map");
            match jobs.get(&digest) {
                Some(Job::Generating) => return generating(),
                Some(Job::Failed { message, at }) if at.elapsed() < FAILURE_MEMORY => {
                    let message = message.clone();
                    return failed(&message);
                }
                _ => {}
            }
            jobs.insert(digest.clone(), Job::Generating);
        }
        let service = self.clone();
        tokio::spawn(async move {
            let result = match service.map_for(&cutout.survey) {
                Some(map) => service.render_offline(map, &cutout, &path).await,
                None => service.fetch(&cutout, &path).await,
            };
            let mut jobs = service.jobs.lock().expect("sky image job map");
            match result {
                Ok(()) => {
                    jobs.remove(&digest);
                }
                Err(message) => {
                    tracing::warn!(survey = cutout.survey, %message, "Sky survey cutout failed");
                    jobs.insert(
                        digest,
                        Job::Failed {
                            message,
                            at: Instant::now(),
                        },
                    );
                }
            }
        });
        generating()
    }

    /// Ask Sesame for a name. The reply is plain text; the `%J` line carries
    /// ICRS degrees. Anything else is a miss, reported as one.
    async fn resolve(&self, query: &str) -> Result<Option<Resolved>, String> {
        let _permit = self
            .admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "Sky image service is shutting down".to_owned())?;
        let url = format!("{}?{}", self.resolver_url, percent_encode(query));
        let response = self.client.get(url).send().await.map_err(|error| {
            if error.is_timeout() {
                "Name resolver timed out".to_owned()
            } else {
                "Name resolver could not be reached".to_owned()
            }
        })?;
        if !response.status().is_success() {
            return Err(format!(
                "Name resolver answered {}",
                response.status().as_u16()
            ));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "Name resolver transfer failed".to_owned())?;
        if bytes.len() > MAX_RESOLVE_BYTES {
            return Err("Name resolver answer is larger than allowed".to_owned());
        }
        let text = String::from_utf8_lossy(&bytes);
        Ok(parse_sesame(query, &text))
    }

    async fn fetch(&self, cutout: &Cutout, path: &FilePath) -> Result<(), String> {
        let _permit = self
            .admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "Sky image service is shutting down".to_owned())?;
        let response = self
            .client
            .get(&self.base_url)
            .query(&cutout.query_pairs())
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    "Survey service timed out".to_owned()
                } else {
                    "Survey service could not be reached".to_owned()
                }
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(match status.as_u16() {
                404 | 400 | 422 => "Survey has no coverage here or refused the request".to_owned(),
                code => format!("Survey service answered {code}"),
            });
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !content_type.starts_with("image/") {
            return Err("Survey service did not return an image".to_owned());
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "Survey image transfer failed".to_owned())?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err("Survey image is larger than allowed".to_owned());
        }
        if bytes.is_empty() {
            return Err("Survey service returned an empty image".to_owned());
        }
        publish(path, &bytes).await
    }

    /// Render a view from an offline map's tiles on a blocking thread and
    /// cache it like a fetched cutout.
    async fn render_offline(
        &self,
        map: Arc<crate::sky_maps::SkyMap>,
        cutout: &Cutout,
        path: &FilePath,
    ) -> Result<(), String> {
        let _permit = self
            .admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "Sky image service is shutting down".to_owned())?;
        let view = crate::sky_maps::View {
            ra_degrees: cutout.ra_degrees,
            dec_degrees: cutout.dec_degrees,
            fov_degrees: cutout.fov_degrees,
            width_px: cutout.width_px,
            height_px: cutout.height_px,
            rotation_degrees: cutout.rotation_degrees,
        };
        let bytes = tokio::task::spawn_blocking(move || map.render(&view))
            .await
            .map_err(|_| "Offline sky map rendering was interrupted".to_owned())??;
        publish(path, &bytes).await
    }
}

/// Write a finished image beside the cache and move it into place, so a
/// reader never sees a half-written file.
async fn publish(path: &FilePath, bytes: &[u8]) -> Result<(), String> {
    {
        let dir = path.parent().expect("cache file has a parent");
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|_| "Cache directory could not be created".to_owned())?;
        let temp = dir.join(format!(
            ".{}.{}.part",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("cutout"),
            std::process::id()
        ));
        tokio::fs::write(&temp, &bytes)
            .await
            .map_err(|_| "Cache file could not be written".to_owned())?;
        if let Err(error) = tokio::fs::rename(&temp, path).await {
            let _ = tokio::fs::remove_file(&temp).await;
            if !path.exists() {
                return Err(format!("Cache file could not be published: {error}"));
            }
        }
        Ok(())
    }
}

/// Query-string encoding for a catalog name; Sesame takes the raw query.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn parse_sesame(query: &str, text: &str) -> Option<Resolved> {
    let mut name = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("%I.0 ") {
            name = Some(rest.trim().to_owned());
        }
        if let Some(rest) = line.strip_prefix("%J ") {
            let mut parts = rest.split_whitespace();
            let ra: f64 = parts.next()?.parse().ok()?;
            let dec: f64 = parts.next()?.parse().ok()?;
            if !(0.0..360.0).contains(&ra) || !(-90.0..=90.0).contains(&dec) {
                return None;
            }
            return Some(Resolved {
                query: query.to_owned(),
                name: name.unwrap_or_else(|| query.to_owned()),
                ra_degrees: ra,
                dec_degrees: dec,
                source: "CDS Sesame (Simbad, NED, VizieR)",
            });
        }
    }
    None
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolveQuery {
    name: String,
}

pub(super) async fn resolve(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ResolveQuery>,
) -> Result<Response, Error> {
    enabled(&state)?;
    let name = query.name.trim();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(Error::Invalid);
    }
    match service(&state).resolve(name).await {
        Ok(Some(resolved)) => Ok(Json(ApiResponse::success(resolved)).into_response()),
        Ok(None) => Ok((
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!(
                "No object named '{name}' in the catalogs"
            ))),
        )
            .into_response()),
        Err(message) => Ok(failed(&message)),
    }
}

fn generating() -> Response {
    let mut response = (
        StatusCode::ACCEPTED,
        Json(ApiResponse::success(
            serde_json::json!({"state": "generating"}),
        )),
    )
        .into_response();
    response
        .headers_mut()
        .insert(RETRY_AFTER, "1".parse().unwrap());
    response
        .headers_mut()
        .insert(CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

fn failed(message: &str) -> Response {
    let mut response = (
        StatusCode::BAD_GATEWAY,
        Json(ApiResponse::<()>::error(message.to_owned())),
    )
        .into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

static SERVICE: OnceLock<Arc<SkyImageService>> = OnceLock::new();

/// One process-wide service. Tests install their own before the first request.
pub(super) fn service(state: &AppState) -> Arc<SkyImageService> {
    SERVICE
        .get_or_init(|| {
            Arc::new(SkyImageService::new(
                FilePath::new(&state.cache_dir_root),
                DEFAULT_BASE_URL,
            ))
        })
        .clone()
}

#[cfg(test)]
pub(super) fn install_for_test(service: SkyImageService) -> Arc<SkyImageService> {
    SERVICE.get_or_init(|| Arc::new(service)).clone()
}

pub(super) async fn surveys(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<SurveyInfo>>>, Error> {
    enabled(&state)?;
    Ok(Json(ApiResponse::success(service(&state).surveys())))
}

pub(super) async fn cutout(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CutoutQuery>,
) -> Result<Response, Error> {
    enabled(&state)?;
    let service = service(&state);
    let cutout = service.cutout_from_query(&query)?;
    Ok(service.respond(cutout).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(survey: &str) -> CutoutQuery {
        CutoutQuery {
            survey: survey.into(),
            ra: 10.68,
            dec: 41.27,
            fov: 3.0,
            width: 800,
            height: 600,
            rotation: 12.0,
        }
    }

    #[test]
    fn only_listed_surveys_and_bounded_views_are_accepted() {
        assert!(Cutout::from_query(&query("dss2_color"), &builtin_surveys()).is_ok());
        assert!(Cutout::from_query(&query("CDS/P/DSS2/color"), &builtin_surveys()).is_err());
        assert!(Cutout::from_query(&query("../etc"), &builtin_surveys()).is_err());
        let mut q = query("dss2_color");
        q.width = 4096;
        assert!(Cutout::from_query(&q, &builtin_surveys()).is_err());
        let mut q = query("dss2_color");
        q.fov = 0.0;
        assert!(Cutout::from_query(&q, &builtin_surveys()).is_err());
        let mut q = query("dss2_color");
        q.ra = 360.0;
        assert!(Cutout::from_query(&q, &builtin_surveys()).is_err());
        let mut q = query("dss2_color");
        q.dec = f64::NAN;
        assert!(Cutout::from_query(&q, &builtin_surveys()).is_err());
    }

    #[test]
    fn the_cache_key_ignores_sub_display_nudges_but_not_the_survey_or_size() {
        let base = Cutout::from_query(&query("dss2_color"), &builtin_surveys()).unwrap();
        let mut nudged = query("dss2_color");
        nudged.ra += 0.000_001;
        assert_eq!(
            base.digest(),
            Cutout::from_query(&nudged, &builtin_surveys())
                .unwrap()
                .digest()
        );
        let mut other = query("finkbeiner_halpha");
        other.width = 800;
        assert_ne!(
            base.digest(),
            Cutout::from_query(&other, &builtin_surveys())
                .unwrap()
                .digest()
        );
        let mut sized = query("dss2_color");
        sized.height = 601;
        assert_ne!(
            base.digest(),
            Cutout::from_query(&sized, &builtin_surveys())
                .unwrap()
                .digest()
        );
        assert_eq!(base.digest().len(), 64);
    }

    #[test]
    fn provider_requests_carry_the_hips_id_and_a_stereographic_projection() {
        let pairs = Cutout::from_query(&query("finkbeiner_halpha"), &builtin_surveys())
            .unwrap()
            .query_pairs();
        let get = |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("hips"), Some("CDS/P/Finkbeiner"));
        assert_eq!(get("projection"), Some("STG"));
        assert_eq!(get("coordsys"), Some("icrs"));
        assert_eq!(get("format"), Some("jpg"));
        assert_eq!(get("rotation_angle"), Some("12.000"));
    }

    #[test]
    fn sesame_text_yields_icrs_degrees_or_nothing() {
        let text = "# M31\t#Q8306215\n%@ @1575544\n%I.0 M  31\n%C.0 AGN\n%J 10.68470833 +41.26875000 = 00 42 44.330  +41 16 07.50\n%V v -300.0\n";
        let hit = parse_sesame("m31", text).unwrap();
        assert_eq!(hit.name, "M  31");
        assert!(
            (hit.ra_degrees - 10.6847).abs() < 1e-4 && (hit.dec_degrees - 41.26875).abs() < 1e-6
        );
        assert!(parse_sesame(
            "nothing",
            "# nothing\n#!SIMBAD: No known catalog could be found\n"
        )
        .is_none());
        assert!(parse_sesame("bad", "%J 400 12\n").is_none());
        assert_eq!(percent_encode("NGC 7000"), "NGC+7000");
        assert_eq!(percent_encode("Sh2-155/é"), "Sh2-155%2F%C3%A9");
    }

    #[test]
    fn survey_ids_are_unique_and_narrowband_layers_say_so() {
        let mut ids: Vec<_> = SURVEYS.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SURVEYS.len());
        assert!(SURVEYS
            .iter()
            .filter(|s| s.kind == SurveyKind::Narrowband)
            .all(|s| s.bandpass.contains("H-alpha")
                || s.bandpass.contains("O III")
                || s.bandpass.contains("Narrowband")));
    }
}
