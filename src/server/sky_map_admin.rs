//! Offline sky maps in Settings: which sets N.I.N.A. publishes, which are
//! installed and where, the folder they are kept in, and the install a
//! person starts. The install itself is `crate::sky_map_install`.
//!
//! PSF Guard never downloads a map by itself. A map is gigabytes, so an
//! install runs as one job server-wide with a row in the job line and
//! Stop. A person starts it, so it holds the interactive guard while it
//! unpacks, which takes the disk and a processor; the download itself,
//! which may take hours on a slow line, leaves background work running.
//! Stop ends the download at once and the unpack between files; an install
//! past its last check finishes, and the install removes its own files
//! either way.

use std::path::{Path as FilePath, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

use crate::server::api::ApiResponse;
use crate::server::auth::RequestAccess;
use crate::server::handlers::{require_database_management_allowed, update_registry, AppError};
use crate::server::state::AppState;
use crate::sky_map_install::{self as install, Set, Source};
use crate::sky_maps;

/// The setting the registry stores.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkyMapsSettings {
    /// The folder offline sky maps are kept in. Absent uses the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// Use `settings` from now on.
pub fn configure(settings: Option<&SkyMapsSettings>) {
    sky_maps::configure(settings.and_then(|settings| settings.folder.as_deref()));
}

/// An install's state, for Settings and the job line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct InstallProgress {
    pub running: bool,
    /// The set being installed, or the last one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_completed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
}

/// The one install that may run at a time.
#[derive(Clone, Default)]
pub struct SkyMapInstalls {
    progress: Arc<RwLock<InstallProgress>>,
    /// The stop of the install under way, or of the last one.
    stop: Arc<RwLock<crate::concurrency::Stop>>,
}

impl SkyMapInstalls {
    pub fn progress(&self) -> InstallProgress {
        self.progress
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set(&self, change: impl FnOnce(&mut InstallProgress)) {
        change(
            &mut self
                .progress
                .write()
                .unwrap_or_else(PoisonError::into_inner),
        );
    }

    /// Stop the install under way. False when none runs.
    pub fn stop(&self) -> bool {
        if !self.progress().running {
            return false;
        }
        self.stop
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .request();
        true
    }

    /// Start installing `source` into `root` in the background and return at
    /// once. While one install runs, another start returns its progress
    /// without starting a second. Stop, or `server_stop` ending, ends it and
    /// leaves nothing partly installed. `hold` is kept for the whole install,
    /// and `while_unpacking`'s value for the unpack only. `done` runs when it
    /// ends, before the progress says so.
    #[allow(clippy::too_many_arguments)]
    pub fn start<G, U, E>(
        &self,
        set_key: &str,
        source: Source,
        root: PathBuf,
        hold: G,
        while_unpacking: impl FnOnce() -> U + Send + 'static,
        server_stop: E,
        done: impl FnOnce() + Send + 'static,
    ) -> InstallProgress
    where
        G: Send + 'static,
        U: Send + 'static,
        E: std::future::Future<Output = ()> + Send + 'static,
    {
        {
            let mut progress = self
                .progress
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            if progress.running {
                return progress.clone();
            }
            *progress = InstallProgress {
                running: true,
                set: Some(set_key.to_string()),
                message: "Starting…".into(),
                started_at: Some(unix_now()),
                ..Default::default()
            };
        }
        let stop = crate::concurrency::Stop::new();
        *self.stop.write().unwrap_or_else(PoisonError::into_inner) = stop.clone();
        let installs = self.clone();
        tokio::spawn(async move {
            let _hold = hold;
            let reporting = installs.clone();
            let report = move |step: install::Step| {
                reporting.set(|progress| {
                    progress.message = step.message;
                    progress.bytes_completed = step.bytes_done;
                    progress.bytes_total = step.bytes_total;
                })
            };
            // A stopping server stops the install the way Stop does. The
            // install is awaited to its end either way: it removes its own
            // files, and an install past its last check finishes, so the
            // message below says what happened.
            let person = stop.clone();
            let install_stop = crate::concurrency::Stop::new();
            let (on_server, on_person) = (install_stop.clone(), install_stop.clone());
            let server_stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let marked = Arc::clone(&server_stopped);
            let watcher = tokio::spawn(async move {
                tokio::select! {
                    () = server_stop => {
                        marked.store(true, std::sync::atomic::Ordering::Relaxed);
                        on_server.request();
                    }
                    () = person.stopped() => on_person.request(),
                }
            });
            let ended = install::install(&root, source, install_stop, while_unpacking, &report)
                .await
                .map(|_| ());
            watcher.abort();
            let ended = match ended {
                Err(error)
                    if error == "Stopped"
                        && server_stopped.load(std::sync::atomic::Ordering::Relaxed) =>
                {
                    Err("The server stopped".to_string())
                }
                other => other,
            };
            // Framing's layers are read again before the progress says the
            // install ended, so a page that refreshes on that word sees the
            // new map.
            done();
            installs.set(|progress| {
                progress.running = false;
                progress.bytes_completed = None;
                progress.bytes_total = None;
                progress.finished_at = Some(unix_now());
                match ended {
                    Ok(()) => progress.message = "Installed".into(),
                    Err(error) => {
                        progress.message = if stop.requested() {
                            "Stopped. Nothing was installed".into()
                        } else {
                            tracing::error!("Offline sky map install failed: {error}");
                            format!("The install failed: {error}")
                        };
                        progress.error = Some(error);
                    }
                }
            });
        });
        self.progress()
    }
}

/// One set N.I.N.A. publishes, as Settings lists it.
#[derive(Debug, Clone, Serialize)]
pub struct SetView {
    pub key: &'static str,
    pub name: &'static str,
    pub covers: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub licence: Option<&'static str>,
    /// The download's size.
    pub bytes: u64,
    /// The free space an install needs while it runs.
    pub room_needed: u64,
    pub installed: bool,
}

/// One installed map.
#[derive(Debug, Clone, Serialize)]
pub struct MapView {
    /// The survey layer's id in Framing.
    pub id: String,
    /// The map's folder name, which removes or moves it.
    pub folder: String,
    pub name: String,
    pub attribution: String,
    pub tiles: usize,
    pub bytes: u64,
    /// The published set it is, when it is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set: Option<&'static str>,
    /// It is still under the cache folder, where maps were installed
    /// before they had a folder of their own.
    pub in_cache: bool,
}

/// What Settings › Offline sky maps shows.
#[derive(Debug, Clone, Serialize)]
pub struct SkyMapsView {
    /// The folder maps are installed in. A server path: absent for a caller
    /// who may not manage the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// The folder used when none is chosen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_folder: Option<String>,
    /// A folder is chosen in Settings.
    pub chosen: bool,
    /// Free space on the folder's volume, when it can be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_bytes: Option<u64>,
    pub sets: Vec<SetView>,
    pub installed: Vec<MapView>,
    /// The survey pictures fetched online and kept under the cache.
    pub cutouts_bytes: u64,
    pub install: InstallProgress,
}

impl SkyMapsView {
    /// Without server paths, for a caller who may not manage the server.
    fn for_viewer(mut self) -> Self {
        self.folder = None;
        self.default_folder = None;
        if self.install.error.is_some() {
            self.install.error = Some("The install failed".into());
            self.install.message = "The install failed".into();
        }
        self
    }
}

/// The folder nearest `path` that exists, to read a volume's free space
/// before the maps' folder is made.
fn existing(path: &FilePath) -> &FilePath {
    let mut at = path;
    while !at.exists() {
        match at.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => at = parent,
            _ => break,
        }
    }
    at
}

fn view(state: &AppState) -> SkyMapsView {
    let cache = state.storage_roots.cache.as_path();
    let own = sky_maps::data_root();
    let older = sky_maps::root_for(cache);
    let installed: Vec<MapView> = install::installed(cache)
        .into_iter()
        .map(|(map, root)| MapView {
            folder: map
                .dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            set: install::set_of(&map).map(|set| set.key),
            bytes: install::folder_bytes(&map.dir),
            tiles: map.tile_count(),
            in_cache: root == older && older != own,
            id: map.id,
            name: map.name,
            attribution: map.attribution,
        })
        .collect();
    SkyMapsView {
        free_bytes: crate::server::cache_budget::available_bytes(existing(&own)),
        sets: install::SETS
            .iter()
            .map(|set: &Set| SetView {
                key: set.key,
                name: set.name,
                covers: set.covers,
                licence: set.licence,
                bytes: set.bytes,
                room_needed: install::room_needed(set.bytes),
                installed: installed.iter().any(|map| map.set == Some(set.key)),
            })
            .collect(),
        installed,
        cutouts_bytes: install::folder_bytes(&cache.join("director").join("sky")),
        folder: Some(own.display().to_string()),
        default_folder: Some(sky_maps::default_root().display().to_string()),
        chosen: sky_maps::chosen_folder().is_some(),
        install: state.sky_map_installs.progress(),
    }
}

/// The view, read off the request path: it sizes folders of thousands of
/// files.
async fn read_view(state: &Arc<AppState>, manages: bool) -> Result<SkyMapsView, AppError> {
    let reading = Arc::clone(state);
    let view = tokio::task::spawn_blocking(move || view(&reading))
        .await
        .map_err(|error| {
            AppError::InternalError(format!("Reading the sky maps failed: {error}"))
        })?;
    Ok(if manages { view } else { view.for_viewer() })
}

/// Whether the caller may see server paths and full errors: an editor, on a
/// server that allows database management, as the job list decides.
fn manages(state: &AppState, access: &Option<Extension<RequestAccess>>) -> bool {
    RequestAccess::manages(access.as_ref().map(|Extension(access)| access), state)
}

/// GET /api/sky-maps
pub async fn get_view(
    State(state): State<Arc<AppState>>,
    access: Option<Extension<RequestAccess>>,
) -> Result<Json<ApiResponse<SkyMapsView>>, AppError> {
    let manages = manages(&state, &access);
    Ok(Json(ApiResponse::success(
        read_view(&state, manages).await?,
    )))
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstallRequest {
    /// A published set's key: `full`, `nsns-ohs` or `nsns-ohs-starless`.
    pub set: String,
}

/// POST /api/sky-maps/install
///
/// The set comes from PSF Guard's own list and goes to the server's own
/// folder, so the request gives neither a URL nor a path. The work
/// continues after the request returns.
pub async fn start_install(
    State(state): State<Arc<AppState>>,
    Json(request): Json<InstallRequest>,
) -> Result<Json<ApiResponse<InstallProgress>>, AppError> {
    let set = install::set(&request.set).ok_or_else(|| {
        AppError::BadRequest(format!("{} is not an offline sky map", request.set))
    })?;
    let hold = state.shutdown.hold("an offline sky map install");
    // A person started it, so background work waits for the unpack, which
    // takes the disk and a processor. The download, which may take hours
    // on a slow line and takes neither, leaves background work running.
    let guarding = Arc::clone(&state);
    let while_unpacking =
        move || guarding.begin_named_interactive_job("an offline sky map install");
    let shutdown = Arc::clone(&state.shutdown);
    let refreshing = Arc::downgrade(&state);
    let progress = state.sky_map_installs.start(
        set.key,
        Source::Set(set),
        sky_maps::data_root(),
        hold,
        while_unpacking,
        async move { shutdown.begun().await },
        move || {
            // Framing offers the new map at once, not half a minute on.
            if let Some(state) = refreshing.upgrade() {
                crate::server::director::forget_sky_maps(&state);
            }
        },
    );
    Ok(Json(ApiResponse::success(progress)))
}

/// POST /api/sky-maps/install/stop
pub async fn stop_install(State(state): State<Arc<AppState>>) -> Json<ApiResponse<bool>> {
    Json(ApiResponse::success(state.sky_map_installs.stop()))
}

/// Refuse to change the maps while one installs: a move or a removal could
/// take the folder from under it.
fn require_no_install(state: &AppState) -> Result<(), AppError> {
    if state.sky_map_installs.progress().running {
        return Err(AppError::BadRequest(
            "An offline sky map is being installed. Wait for it, or stop it, then try again."
                .into(),
        ));
    }
    Ok(())
}

/// The root an installed map's folder is in.
fn root_of(state: &AppState, folder: &str) -> Result<PathBuf, AppError> {
    install::installed(state.storage_roots.cache.as_path())
        .into_iter()
        .find(|(map, _)| {
            map.dir
                .file_name()
                .is_some_and(|name| name.to_string_lossy() == folder)
        })
        .map(|(_, root)| root)
        .ok_or(AppError::NotFound)
}

/// DELETE /api/sky-maps/installed/{folder}
///
/// Deletes a map's files from the server, so it needs database management.
pub async fn remove_map(
    State(state): State<Arc<AppState>>,
    Path(folder): Path<String>,
) -> Result<Json<ApiResponse<SkyMapsView>>, AppError> {
    require_database_management_allowed(&state)?;
    require_no_install(&state)?;
    let working = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let root = root_of(&working, &folder)?;
        install::remove(&root, &folder).map_err(AppError::BadRequest)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("Removing the map failed: {error}")))??;
    crate::server::director::forget_sky_maps(&state);
    Ok(Json(ApiResponse::success(read_view(&state, true).await?)))
}

/// POST /api/sky-maps/installed/{folder}/move
///
/// Moves a map left under the cache folder into the maps' own folder.
pub async fn move_map(
    State(state): State<Arc<AppState>>,
    Path(folder): Path<String>,
) -> Result<Json<ApiResponse<SkyMapsView>>, AppError> {
    require_database_management_allowed(&state)?;
    require_no_install(&state)?;
    let working = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let from = root_of(&working, &folder)?;
        let own = sky_maps::data_root();
        if from == own {
            return Err(AppError::BadRequest(format!(
                "{folder} is already in the offline sky maps folder"
            )));
        }
        install::relocate(&from, &folder, &own)
            .map(|_| ())
            .map_err(AppError::BadRequest)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("Moving the map failed: {error}")))??;
    crate::server::director::forget_sky_maps(&state);
    Ok(Json(ApiResponse::success(read_view(&state, true).await?)))
}

/// PUT /api/settings/sky-maps
///
/// The request gives a path on the server, so it needs database management.
/// Maps already installed stay where they are: the new folder starts
/// empty, and the old one's maps are no longer listed.
pub async fn update_settings(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SkyMapsSettings>,
) -> Result<Json<ApiResponse<SkyMapsView>>, AppError> {
    require_database_management_allowed(&state)?;
    require_no_install(&state)?;
    let settings = SkyMapsSettings {
        folder: request
            .folder
            .map(|folder| folder.trim().to_string())
            .filter(|folder| !folder.is_empty()),
    };
    if let Some(folder) = &settings.folder {
        let path = PathBuf::from(folder);
        if !path.is_absolute() {
            return Err(AppError::BadRequest(
                "Enter the folder's full path, such as /data/sky-maps".into(),
            ));
        }
        std::fs::create_dir_all(&path).map_err(|error| {
            AppError::BadRequest(format!("{folder} could not be created: {error}"))
        })?;
    }
    let stored = (settings != SkyMapsSettings::default()).then(|| settings.clone());
    update_registry(&state, |registry| {
        registry.sky_maps = stored;
        Ok(())
    })
    .await?;
    configure(Some(&settings));
    crate::server::director::forget_sky_maps(&state);
    Ok(Json(ApiResponse::success(read_view(&state, true).await?)))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn settled(installs: &SkyMapInstalls) -> InstallProgress {
        for _ in 0..400 {
            let progress = installs.progress();
            if !progress.running {
                return progress;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the install did not end");
    }

    /// Serve `body` at a local address, after `delay`.
    async fn serve(body: Vec<u8>, delay: std::time::Duration) -> String {
        use axum::routing::get;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/map.zip",
            get(move || {
                let body = body.clone();
                async move {
                    tokio::time::sleep(delay).await;
                    body
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}/map.zip")
    }

    fn names(root: &FilePath) -> Vec<String> {
        std::fs::read_dir(root)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| name != ".install.lock")
                    .collect()
            })
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn an_install_reports_its_set_and_ends_installed() {
        let root = tempfile::tempdir().unwrap();
        let url = serve(
            install::tests::map_zip("FramingAssistantCache"),
            std::time::Duration::ZERO,
        )
        .await;
        let installs = SkyMapInstalls::default();
        let (told, done) = std::sync::mpsc::channel();
        let started = installs.start(
            "full",
            Source::Url(url),
            root.path().to_path_buf(),
            (),
            || (),
            std::future::pending(),
            move || told.send(()).unwrap(),
        );
        assert!(started.running);
        assert_eq!(started.set.as_deref(), Some("full"));
        let ended = settled(&installs).await;
        assert_eq!(ended.message, "Installed");
        assert_eq!(ended.error, None);
        assert!(ended.finished_at.is_some());
        done.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(names(root.path()), ["FramingAssistantCache"]);
    }

    #[tokio::test]
    async fn a_second_start_joins_the_install_and_stop_leaves_nothing_behind() {
        let root = tempfile::tempdir().unwrap();
        let slow = serve(
            install::tests::map_zip("FramingAssistantCache"),
            std::time::Duration::from_secs(30),
        )
        .await;
        let installs = SkyMapInstalls::default();
        let start = |set: &str| {
            installs.start(
                set,
                Source::Url(slow.clone()),
                root.path().to_path_buf(),
                (),
                || (),
                std::future::pending(),
                || {},
            )
        };
        assert!(!installs.stop(), "nothing runs yet");
        start("full");
        // The second start is given the first's progress.
        assert_eq!(start("nsns-ohs").set.as_deref(), Some("full"));
        assert!(installs.stop());
        let ended = settled(&installs).await;
        assert_eq!(ended.message, "Stopped. Nothing was installed");
        assert!(names(root.path()).is_empty(), "{:?}", names(root.path()));
    }

    #[tokio::test]
    async fn a_stopping_server_ends_an_install_and_a_failure_gives_its_reason() {
        let root = tempfile::tempdir().unwrap();
        let slow = serve(Vec::new(), std::time::Duration::from_secs(30)).await;
        let installs = SkyMapInstalls::default();
        installs.start(
            "full",
            Source::Url(slow),
            root.path().to_path_buf(),
            (),
            || (),
            std::future::ready(()),
            || {},
        );
        let ended = settled(&installs).await;
        assert_eq!(ended.error.as_deref(), Some("The server stopped"));

        let broken = serve(b"not a zip".to_vec(), std::time::Duration::ZERO).await;
        installs.start(
            "full",
            Source::Url(broken),
            root.path().to_path_buf(),
            (),
            || (),
            std::future::pending(),
            || {},
        );
        let ended = settled(&installs).await;
        assert!(
            ended.message.starts_with("The install failed: "),
            "{}",
            ended.message
        );
        assert!(names(root.path()).is_empty());
    }

    #[test]
    fn a_viewer_is_given_no_server_paths() {
        let view = SkyMapsView {
            folder: Some("/data/sky-maps".into()),
            default_folder: Some("/root/.local/share/psf-guard/sky-maps".into()),
            chosen: true,
            free_bytes: Some(1),
            sets: Vec::new(),
            installed: Vec::new(),
            cutouts_bytes: 0,
            install: InstallProgress {
                error: Some("/data/sky-maps has 1.0 GB free".into()),
                message: "The install failed: /data/sky-maps has 1.0 GB free".into(),
                ..Default::default()
            },
        }
        .for_viewer();
        let sent = serde_json::to_string(&view).unwrap();
        assert!(!sent.contains("/data"), "{sent}");
        assert!(!sent.contains("/root"), "{sent}");
    }

    #[test]
    fn only_an_editor_on_a_managed_server_is_given_paths() {
        use crate::server::auth::AccessRole;
        let state = AppState::from_databases(
            Vec::new(),
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .into_owned(),
            crate::cli::PregenerationConfig::default(),
        )
        .unwrap();
        let access = |role| {
            Some(Extension(RequestAccess {
                role,
                username: None,
                api_token: false,
            }))
        };
        let (reader, editor) = (access(AccessRole::ReadOnly), access(AccessRole::ReadWrite));
        state.set_allow_database_management(true);
        assert!(!manages(&state, &reader));
        assert!(manages(&state, &editor));
        assert!(manages(&state, &None), "a server without accounts");
        state.set_allow_database_management(false);
        assert!(!manages(&state, &editor));
    }

    #[test]
    fn free_space_is_read_from_the_nearest_folder_that_exists() {
        let root = tempfile::tempdir().unwrap();
        let deep = root.path().join("not").join("made").join("yet");
        assert_eq!(existing(&deep), root.path());
        assert_eq!(existing(root.path()), root.path());
    }
}
