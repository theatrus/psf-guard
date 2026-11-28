//! The PSF Guard servers Studio is connected to, and how it talks to them.
//!
//! Studio keeps a list of servers in its settings folder: each server's
//! address, the account it connected as, and whether that account may save
//! documents. The list holds no secrets. A server with sign-in gives Studio a
//! token, which Studio keeps in the macOS Keychain, one entry per address.
//!
//! Every call here blocks, so Studio makes them on threads of their own.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The Keychain service Studio's tokens are kept under.
const KEYCHAIN_SERVICE: &str = "PSF Studio";
/// How long a request may take before Studio gives up on it.
const REQUEST_SECONDS: u64 = 30;

/// A server Studio connected to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerRecord {
    /// The server's origin: scheme, host and port, with no path.
    pub address: String,
    /// Whether the server signs people in. A server without sign-in needs
    /// no token.
    pub signs_in: bool,
    /// The account Studio connected as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Whether that account may save documents to the server.
    #[serde(default)]
    pub saves_documents: bool,
    /// When the token expires, in Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

/// The list of servers, as Studio keeps it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerList {
    #[serde(default)]
    pub servers: Vec<ServerRecord>,
}

/// Studio's settings folder.
pub fn settings_folder() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/psf-studio")
    } else {
        home.join(".local/share/psf-studio")
    }
}

impl ServerList {
    pub fn path() -> PathBuf {
        settings_folder().join("servers.json")
    }

    /// The list in `path`; an empty list when there is none or it cannot be
    /// read.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the list beside `path` and renames it over, so a crash never
    /// leaves half a list.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        let temporary = path.with_extension("json.saving");
        std::fs::write(&temporary, text)
            .and_then(|()| std::fs::rename(&temporary, path))
            .map_err(|error| error.to_string())
    }

    pub fn find(&self, address: &str) -> Option<&ServerRecord> {
        self.servers.iter().find(|server| server.address == address)
    }

    /// Adds a server, or replaces the one at its address.
    pub fn keep(&mut self, record: ServerRecord) {
        match self
            .servers
            .iter_mut()
            .find(|server| server.address == record.address)
        {
            Some(known) => *known = record,
            None => self.servers.push(record),
        }
    }

    pub fn remove(&mut self, address: &str) {
        self.servers.retain(|server| server.address != address);
    }
}

/// A server's address as a person typed it, as an origin: `localhost:3000`
/// reads `http://localhost:3000`, `guard.example.com` reads
/// `https://guard.example.com`. A path is dropped.
pub fn normal_address(text: &str) -> Result<String, String> {
    let text = text.trim();
    // Slashes after the scheme's are dropped, not those of `http://`.
    let text = match text.split_once("://") {
        Some((scheme, rest)) if !rest.is_empty() => {
            &text[..scheme.len() + 3 + rest.trim_end_matches('/').len()]
        }
        Some(_) => text,
        None => text.trim_end_matches('/'),
    };
    if text.is_empty() {
        return Err("Enter the server's address, such as http://localhost:3000".to_string());
    }
    let (scheme, rest) = match text.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => {
            let host = text.split(['/', ':']).next().unwrap_or_default();
            let local = host == "localhost"
                || host.ends_with(".local")
                || host.parse::<std::net::IpAddr>().is_ok();
            (if local { "http" } else { "https" }.to_string(), text)
        }
    };
    if scheme != "http" && scheme != "https" {
        return Err(format!("PSF Guard servers use http or https, not {scheme}"));
    }
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() || authority.contains(char::is_whitespace) {
        return Err("The address has no server name".to_string());
    }
    Ok(format!("{scheme}://{}", authority.to_ascii_lowercase()))
}

/// Whether a token to this address travels unencrypted over the network:
/// plain HTTP to another computer.
pub fn sends_in_the_clear(address: &str) -> bool {
    let Some(rest) = address.strip_prefix("http://") else {
        return false;
    };
    let host = rest.rsplit_once(':').map_or(rest, |(host, _)| host);
    !(host == "localhost" || host == "127.0.0.1" || host == "[::1]")
}

/// This computer's name, for the label of the token a server mints.
pub fn computer_name() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "a Mac".to_string())
}

/// The token Studio keeps for a server, if any.
pub fn kept_token(address: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, address)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = address;
        None
    }
}

/// Keeps a server's token in the Keychain.
pub fn keep_token(address: &str, token: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            address,
            token.as_bytes(),
        )
        .map_err(|error| format!("The token could not be kept in the Keychain: {error}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (address, token);
        Err("Studio keeps tokens in the macOS Keychain only".to_string())
    }
}

/// Deletes a server's token from the Keychain. A token that is not there is
/// no error.
pub fn forget_token(address: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, address);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = address;
    }
}

/// Why a request to a server did not succeed.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// The server did not answer.
    Unreachable(String),
    /// The token was revoked or has expired, or a token is needed: 401.
    SignedOut,
    /// The account may not do this: 403, with the server's reason.
    Refused(String),
    /// Anything else, with the server's reason.
    Failed(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(reason) => write!(formatter, "The server did not answer: {reason}"),
            Self::SignedOut => formatter.write_str("Studio is signed out of this server"),
            Self::Refused(reason) | Self::Failed(reason) => formatter.write_str(reason),
        }
    }
}

/// A connection to one server, with its token when it signs people in.
#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
    pub address: String,
    token: Option<String>,
}

/// What `/api/auth/status` answers.
#[derive(Clone, Debug, Deserialize)]
pub struct AuthStatus {
    pub authentication_required: bool,
}

/// A connect waiting for a person to approve its code.
#[derive(Clone, Debug, Deserialize)]
pub struct ConnectStarted {
    pub connect_id: String,
    pub user_code: String,
    /// The approval page, from the server's root.
    pub approve_path: String,
    pub poll_seconds: u64,
}

/// What trading a connect id or a ticket answers.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TokenAnswer {
    /// `waiting`, `denied`, `expired` or `approved`.
    pub status: String,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub saves_documents: Option<bool>,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Database {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Project {
    pub id: i32,
    pub display_name: String,
    #[serde(default)]
    pub latest_image_date: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Target {
    pub id: i32,
    pub name: String,
    #[serde(default)]
    pub accepted_count: i32,
}

/// One channel stack of a target, as the color stack list gives it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct ChannelStack {
    pub filter_name: String,
    #[serde(default)]
    pub accepted_frames: usize,
    #[serde(default)]
    pub total_exposure_seconds: Option<f64>,
    #[serde(default)]
    pub job_id: String,
    #[serde(default)]
    pub group_index: usize,
    #[serde(default)]
    pub artifact_revision: String,
    /// The frame the stack was registered to.
    #[serde(default)]
    pub reference_image_id: Option<i32>,
    #[serde(default)]
    pub sky_orientation: Option<SkyOrientation>,
}

/// The size a stack is drawn at.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct SkyOrientation {
    pub output_width: usize,
    pub output_height: usize,
}

/// A stack a document was made from, as its record keeps it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct StackFingerprint {
    pub filter_name: String,
    pub job_id: String,
    pub group_index: usize,
    pub artifact_revision: String,
    #[serde(default)]
    pub reference_image_id: Option<i32>,
    #[serde(default)]
    pub width: Option<usize>,
    #[serde(default)]
    pub height: Option<usize>,
}

/// How a target's stacks now differ from those a document was made from.
#[derive(Clone, Debug, PartialEq)]
pub enum StacksChanged {
    /// The same stacks, or the document records none.
    No,
    /// Built again, registered to the same frame at the same size: masks,
    /// heals and the crop still fall where they did.
    Rebuilt,
    /// Registered to another frame, or another size.
    Reframed,
}

/// Compares the stacks a document was made from with a target's stacks now.
/// A size or a frame that either side does not know is taken as the same,
/// and so is a side with no stacks: there is nothing to compare.
pub fn stacks_changed(recorded: &[StackFingerprint], now: &[ChannelStack]) -> StacksChanged {
    if recorded.is_empty() || now.is_empty() {
        return StacksChanged::No;
    }
    let same = |old: &StackFingerprint, new: &ChannelStack| {
        old.job_id == new.job_id
            && old.group_index == new.group_index
            && old.artifact_revision == new.artifact_revision
    };
    if recorded.len() == now.len()
        && recorded
            .iter()
            .all(|old| now.iter().any(|new| same(old, new)))
    {
        return StacksChanged::No;
    }
    let differs =
        |old: Option<usize>, new: Option<usize>| matches!((old, new), (Some(a), Some(b)) if a != b);
    let reframed =
        recorded.iter().any(|old| {
            now.iter().any(|new| {
            matches!((old.reference_image_id, new.reference_image_id), (Some(a), Some(b)) if a != b)
                || differs(old.width, new.sky_orientation.as_ref().map(|sky| sky.output_width))
                || differs(old.height, new.sky_orientation.as_ref().map(|sky| sky.output_height))
        })
        });
    if reframed {
        StacksChanged::Reframed
    } else {
        StacksChanged::Rebuilt
    }
}

/// A target with its channel stacks.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StackedTarget {
    pub target_id: i32,
    #[serde(default)]
    pub target_name: String,
    #[serde(default)]
    pub source_candidates: Vec<ChannelStack>,
}

impl Client {
    pub fn new(address: &str, token: Option<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(REQUEST_SECONDS)))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            agent,
            address: address.to_string(),
            token,
        }
    }

    /// The `data` of a reply, or why there is none.
    fn answer<T: DeserializeOwned>(
        response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<T, Failure> {
        let mut response = response.map_err(|error| Failure::Unreachable(error.to_string()))?;
        let status = response.status().as_u16();
        let body: Value = response.body_mut().read_json().unwrap_or(Value::Null);
        let reason = || {
            body["error"]
                .as_str()
                .map_or_else(|| format!("The server answered {status}"), str::to_string)
        };
        match status {
            200..=299 => serde_json::from_value(body["data"].clone()).map_err(|error| {
                Failure::Failed(format!("The server's reply was not understood: {error}"))
            }),
            401 => Err(Failure::SignedOut),
            403 => Err(Failure::Refused(reason())),
            _ => Err(Failure::Failed(reason())),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api{path}", self.address)
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Failure> {
        let mut request = self.agent.get(self.url(path));
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        Self::answer(request.call())
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: Value) -> Result<T, Failure> {
        let mut request = self.agent.post(self.url(path));
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        Self::answer(request.send_json(body))
    }

    pub fn auth_status(&self) -> Result<AuthStatus, Failure> {
        self.get("/auth/status")
    }

    pub fn start_connect(&self, client_name: &str) -> Result<ConnectStarted, Failure> {
        self.post(
            "/auth/studio/connect",
            json!({ "client_name": client_name }),
        )
    }

    pub fn trade_connect(&self, connect_id: &str) -> Result<TokenAnswer, Failure> {
        self.post("/auth/studio/token", json!({ "connect_id": connect_id }))
    }

    pub fn trade_ticket(&self, ticket: &str, client_name: &str) -> Result<TokenAnswer, Failure> {
        self.post(
            "/auth/studio/token",
            json!({ "ticket": ticket, "client_name": client_name }),
        )
    }

    /// Revokes this client's own token.
    pub fn disconnect(&self) -> Result<(), Failure> {
        self.post::<Value>("/auth/studio/disconnect", Value::Null)
            .map(|_| ())
    }

    pub fn databases(&self) -> Result<Vec<Database>, Failure> {
        self.get("/databases")
    }

    pub fn projects(&self, database: &str) -> Result<Vec<Project>, Failure> {
        self.get(&format!("/db/{}/projects", path_part(database)))
    }

    pub fn targets(&self, database: &str, project: i32) -> Result<Vec<Target>, Failure> {
        self.get(&format!(
            "/db/{}/projects/{project}/targets",
            path_part(database)
        ))
    }

    /// A project's targets that have channel stacks, with those stacks.
    pub fn stacked_targets(
        &self,
        database: &str,
        project: i32,
    ) -> Result<Vec<StackedTarget>, Failure> {
        #[derive(Deserialize)]
        struct Catalog {
            #[serde(default)]
            targets: Vec<StackedTarget>,
        }
        self.get::<Catalog>(&format!(
            "/db/{}/projects/{project}/stack-previews/color",
            path_part(database)
        ))
        .map(|catalog| catalog.targets)
    }
}

/// A Studio document on a server, as its list shows it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DocumentSummary {
    pub id: String,
    pub name: String,
    pub revision: u32,
    pub saved_at: i64,
    pub saved_by: String,
}

/// A file a revision uses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct FileUse {
    pub role: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RevisionEntry {
    pub revision: u32,
    #[serde(default)]
    pub saved_by: String,
    #[serde(default)]
    pub files: Vec<FileUse>,
}

/// A document's record on a server.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DocumentRecord {
    pub id: String,
    pub name: String,
    pub revisions: Vec<RevisionEntry>,
    /// The stacks the document was made from.
    #[serde(default)]
    pub stacks: Vec<StackFingerprint>,
}

impl DocumentRecord {
    pub fn current(&self) -> Option<&RevisionEntry> {
        self.revisions.last()
    }
}

/// Why a save did not reach the server.
#[derive(Clone, Debug, PartialEq)]
pub enum SaveFailure {
    /// Another computer saved first: the server's current revision.
    Conflict(DocumentRecord),
    Failed(Failure),
}

/// The largest document Studio reads from a server.
const MAX_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;

impl Client {
    fn documents_url(&self, database: &str) -> String {
        format!("/db/{}/studio-documents", path_part(database))
    }

    fn with_token<B>(&self, request: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        match &self.token {
            Some(token) => request.header("Authorization", format!("Bearer {token}")),
            None => request,
        }
    }

    /// The documents of a target, most recently saved first.
    pub fn documents(
        &self,
        database: &str,
        project: i32,
        target: i32,
    ) -> Result<Vec<DocumentSummary>, Failure> {
        self.get(&format!(
            "{}?project={project}&target={target}",
            self.documents_url(database)
        ))
    }

    pub fn document_record(&self, database: &str, id: &str) -> Result<DocumentRecord, Failure> {
        self.get(&format!(
            "{}/{}",
            self.documents_url(database),
            path_part(id)
        ))
    }

    /// A revision's document, as text.
    pub fn revision_text(
        &self,
        database: &str,
        id: &str,
        revision: u32,
    ) -> Result<String, Failure> {
        let url = self.url(&format!(
            "{}/{}/revisions/{revision}",
            self.documents_url(database),
            path_part(id)
        ));
        let mut response = self
            .with_token(self.agent.get(url))
            .call()
            .map_err(|error| Failure::Unreachable(error.to_string()))?;
        match response.status().as_u16() {
            200 => response
                .body_mut()
                .with_config()
                .limit(MAX_DOCUMENT_BYTES)
                .read_to_string()
                .map_err(|error| Failure::Failed(error.to_string())),
            401 => Err(Failure::SignedOut),
            status => Err(Failure::Failed(format!("The server answered {status}"))),
        }
    }

    fn file_url(&self, database: &str, sha256: &str) -> String {
        self.url(&format!("{}/files/{sha256}", self.documents_url(database)))
    }

    /// Uploads a file unless the server has it already.
    pub fn upload_file(&self, database: &str, sha256: &str, path: &Path) -> Result<(), Failure> {
        let url = self.file_url(database, sha256);
        let known = self
            .with_token(self.agent.head(&url))
            .call()
            .map_err(|error| Failure::Unreachable(error.to_string()))?;
        if known.status().as_u16() == 200 {
            return Ok(());
        }
        let file = std::fs::File::open(path).map_err(|error| {
            Failure::Failed(format!("{} could not be read: {error}", path.display()))
        })?;
        Self::answer::<Value>(self.with_token(self.agent.put(&url)).send(&file)).map(|_| ())
    }

    /// Downloads a file to `path`, through a temporary file beside it.
    pub fn download_file(&self, database: &str, sha256: &str, path: &Path) -> Result<(), Failure> {
        let mut response = self
            .with_token(self.agent.get(self.file_url(database, sha256)))
            .call()
            .map_err(|error| Failure::Unreachable(error.to_string()))?;
        match response.status().as_u16() {
            200 => {}
            401 => return Err(Failure::SignedOut),
            status => {
                return Err(Failure::Failed(format!(
                    "The server answered {status} for a file"
                )))
            }
        }
        let temporary = path.with_extension("downloading");
        let written = (|| {
            let mut file = std::fs::File::create(&temporary)?;
            let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
            std::io::copy(&mut reader, &mut file)?;
            file.sync_all()?;
            std::fs::rename(&temporary, path)
        })();
        written.map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            Failure::Failed(format!("The file was not downloaded: {error}"))
        })
    }

    pub fn create_document(&self, database: &str, body: Value) -> Result<DocumentRecord, Failure> {
        self.post(&self.documents_url(database), body)
    }

    /// Saves a revision, or answers the server's current one when another
    /// computer saved first.
    pub fn save_document(
        &self,
        database: &str,
        id: &str,
        body: Value,
    ) -> Result<DocumentRecord, SaveFailure> {
        let url = self.url(&format!(
            "{}/{}",
            self.documents_url(database),
            path_part(id)
        ));
        let mut response = self
            .with_token(self.agent.put(url))
            .send_json(body)
            .map_err(|error| SaveFailure::Failed(Failure::Unreachable(error.to_string())))?;
        if response.status().as_u16() == 409 {
            let body: Value = response.body_mut().read_json().unwrap_or(Value::Null);
            return match serde_json::from_value(body["data"].clone()) {
                Ok(record) => Err(SaveFailure::Conflict(record)),
                Err(error) => Err(SaveFailure::Failed(Failure::Failed(error.to_string()))),
            };
        }
        Self::answer(Ok(response)).map_err(SaveFailure::Failed)
    }

    pub fn add_export(&self, database: &str, id: &str, body: Value) -> Result<(), Failure> {
        self.post::<Value>(
            &format!("{}/{}/exports", self.documents_url(database), path_part(id)),
            body,
        )
        .map(|_| ())
    }
}

/// A name as one part of a URL path: everything but letters, digits and
/// `-_.~` escaped.
fn path_part(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_read_as_an_origin() {
        assert_eq!(
            normal_address("localhost:3000").unwrap(),
            "http://localhost:3000"
        );
        assert_eq!(
            normal_address("192.168.1.5:3000/").unwrap(),
            "http://192.168.1.5:3000"
        );
        assert_eq!(
            normal_address("observatory.local").unwrap(),
            "http://observatory.local"
        );
        assert_eq!(
            normal_address(" Guard.Example.com/#/stacks ").unwrap(),
            "https://guard.example.com"
        );
        assert_eq!(
            normal_address("HTTP://Guard.example.com:8080/x").unwrap(),
            "http://guard.example.com:8080"
        );
        assert!(normal_address("").is_err());
        assert!(normal_address("ftp://guard").is_err());
        assert!(normal_address("http://").is_err());
    }

    #[test]
    fn a_token_travels_in_the_clear_over_http_to_another_computer() {
        assert!(sends_in_the_clear("http://observatory.local:3000"));
        assert!(!sends_in_the_clear("http://localhost:3000"));
        assert!(!sends_in_the_clear("http://127.0.0.1:3000"));
        assert!(!sends_in_the_clear("https://guard.example.com"));
    }

    #[test]
    fn the_server_list_keeps_one_record_per_address_and_no_token() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("servers.json");
        assert_eq!(ServerList::load(&path), ServerList::default());
        let mut list = ServerList::default();
        let record = ServerRecord {
            address: "http://localhost:3000".to_string(),
            signs_in: true,
            username: Some("ana".to_string()),
            saves_documents: true,
            expires_at: Some(1_800_000_000),
        };
        list.keep(record.clone());
        list.keep(ServerRecord {
            saves_documents: false,
            ..record.clone()
        });
        assert_eq!(list.servers.len(), 1);
        assert!(!list.servers[0].saves_documents);
        list.save(&path).unwrap();
        assert_eq!(ServerList::load(&path), list);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("psfg_"));
        list.remove("http://localhost:3000");
        assert!(list.servers.is_empty());
    }

    #[test]
    fn stacks_built_again_are_told_from_stacks_registered_anew() {
        let recorded = |revision: &str, reference: i32, width: usize| StackFingerprint {
            filter_name: "Ha".to_string(),
            job_id: "j".to_string(),
            group_index: 0,
            artifact_revision: revision.to_string(),
            reference_image_id: Some(reference),
            width: Some(width),
            height: Some(4000),
        };
        let now = |revision: &str, reference: i32, width: usize| ChannelStack {
            filter_name: "Ha".to_string(),
            job_id: "j".to_string(),
            artifact_revision: revision.to_string(),
            reference_image_id: Some(reference),
            sky_orientation: Some(SkyOrientation {
                output_width: width,
                output_height: 4000,
            }),
            ..ChannelStack::default()
        };
        assert_eq!(stacks_changed(&[], &[now("r", 1, 6000)]), StacksChanged::No);
        assert_eq!(
            stacks_changed(&[recorded("r", 1, 6000)], &[]),
            StacksChanged::No
        );
        assert_eq!(
            stacks_changed(&[recorded("r", 1, 6000)], &[now("r", 1, 6000)]),
            StacksChanged::No
        );
        assert_eq!(
            stacks_changed(&[recorded("r", 1, 6000)], &[now("r2", 1, 6000)]),
            StacksChanged::Rebuilt
        );
        assert_eq!(
            stacks_changed(&[recorded("r", 1, 6000)], &[now("r2", 2, 6000)]),
            StacksChanged::Reframed
        );
        assert_eq!(
            stacks_changed(&[recorded("r", 1, 6000)], &[now("r2", 1, 6100)]),
            StacksChanged::Reframed
        );
    }

    #[test]
    fn a_database_name_is_escaped_in_a_path() {
        assert_eq!(path_part("my-db_1"), "my-db_1");
        assert_eq!(path_part("a b/c"), "a%20b%2Fc");
    }

    #[test]
    fn a_stacked_target_reads_from_the_color_stack_list() {
        let target: StackedTarget = serde_json::from_value(json!({
            "target_id": 17,
            "target_name": "M 31",
            "source_candidates": [
                {"filter_name": "Ha", "accepted_frames": 40, "total_exposure_seconds": 12000.0,
                 "role": "red", "job_id": "j", "group_index": 0, "artifact_revision": "r"}
            ],
            "other": true
        }))
        .unwrap();
        assert_eq!(target.target_id, 17);
        assert_eq!(target.source_candidates[0].accepted_frames, 40);
    }
}
