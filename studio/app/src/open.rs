//! Opening a picture from inside Studio.
//!
//! A picture is opened from stack files a person picks, or from a link a
//! PSF Guard page sends: `psf-studio://open?server=…&db=…&project=…&target=…`.
//! Either way Studio runs `psf-guard studio`, the command a person can run
//! by hand, so the stacks are fetched, registered and prepared by the same
//! code, and then shows the picture it prepared.
//!
//! Pictures are kept in Studio's own folder, never beside a person's
//! stacks: a store and a document per picture. Opening the same target or
//! the same files again finds the same picture, with its document as it
//! was left.

use super::Studio;
use gpui::{Context, PathPromptOptions};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

/// The links the system has handed this program and no window has taken
/// yet.
static LINKS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Keeps links for the window to open. Given to the application before it
/// runs, so a link that starts the program is not lost.
pub fn keep_links(links: Vec<String>) {
    LINKS.lock().expect("links").extend(links);
}

/// The folder Studio keeps its pictures in.
pub(super) fn pictures_folder() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/psf-studio/pictures")
    } else {
        home.join(".local/share/psf-studio/pictures")
    }
}

/// A name for a picture's files: letters, digits, dashes and underscores.
fn file_name(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// The value of one field of a link's query, with its escapes undone.
fn field(query: &str, name: &str) -> Option<String> {
    let value = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)?
        .1;
    let bytes = value.as_bytes();
    let mut text = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'%' if at + 2 < bytes.len() => {
                let code = std::str::from_utf8(&bytes[at + 1..at + 3]).ok()?;
                text.push(u8::from_str_radix(code, 16).ok()?);
                at += 3;
            }
            b'+' => {
                text.push(b' ');
                at += 1;
            }
            byte => {
                text.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(text).ok()
}

/// What a link asks Studio to open.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TargetLink {
    pub server: String,
    pub database: String,
    pub project: i32,
    pub target: i32,
    /// A one-time ticket from a signed-in click, which Studio trades for a
    /// token when it is not connected to the server yet.
    pub ticket: Option<String>,
}

/// Reads a `psf-studio://open?…` link. `None` for any other text.
pub(super) fn target_link(link: &str) -> Option<TargetLink> {
    let query = link.strip_prefix("psf-studio://open?")?;
    let server = field(query, "server")?;
    // A link opens stacks on a server over HTTP, and nothing else.
    if !(server.starts_with("http://") || server.starts_with("https://")) {
        return None;
    }
    Some(TargetLink {
        server,
        database: field(query, "db")?,
        project: field(query, "project")?.parse().ok()?,
        target: field(query, "target")?.parse().ok()?,
        ticket: field(query, "ticket").filter(|ticket| !ticket.is_empty()),
    })
}

/// The name of a server target's picture: the server and the database as
/// given are part of it, so two servers' targets never share one picture.
pub(super) fn target_picture_name(link: &TargetLink) -> String {
    let identity = {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(format!("{}\0{}", link.server, link.database));
        hash.iter()
            .take(4)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    file_name(&format!(
        "{}-p{}-t{}-{identity}",
        link.database, link.project, link.target
    ))
}

impl Studio {
    /// Starts `psf-guard studio` for one picture and opens it when the
    /// run ends.
    ///
    /// A server's token goes to `psf-guard` in `PSF_GUARD_TOKEN`, not as an
    /// argument, which every process on the computer can read.
    fn prepare_and_open(
        &mut self,
        name: &str,
        arguments: Vec<std::ffi::OsString>,
        token: Option<String>,
    ) {
        let Some(binary) = self.psf_guard() else {
            return;
        };
        let folder = pictures_folder();
        let name = file_name(name);
        let mut command = Command::new(binary);
        command
            .arg("studio")
            .args(arguments)
            .arg("--folder")
            .arg(&folder)
            .arg("--name")
            .arg(&name)
            .arg("--no-open");
        match token {
            Some(token) => command.env("PSF_GUARD_TOKEN", token),
            None => command.env_remove("PSF_GUARD_TOKEN"),
        };
        let store = folder.join(format!("{name}.pgtiles"));
        let document = folder.join(format!("{name}.json"));
        self.start_run(command, Some((store, document)));
    }

    /// Opens a target's channel stacks from a PSF Guard server as the
    /// picture `name`, with the server's token when it signs people in.
    pub(super) fn open_target_named(
        &mut self,
        link: &TargetLink,
        token: Option<String>,
        name: &str,
    ) {
        self.prepare_and_open(
            name,
            vec![
                "--server".into(),
                link.server.clone().into(),
                "--db".into(),
                link.database.clone().into(),
                "--project".into(),
                link.project.to_string().into(),
                "--target".into(),
                link.target.to_string().into(),
            ],
            token,
        );
    }

    /// Opens stack files, one per channel. `psf-guard` reads each file's
    /// channel from its FILTER header or its name.
    pub(super) fn open_files(&mut self, mut files: Vec<PathBuf>) {
        if files.is_empty() {
            return;
        }
        files.sort();
        // The same files are the same picture.
        let identity = {
            use sha2::{Digest, Sha256};
            let mut hash = Sha256::new();
            for file in &files {
                hash.update(file.to_string_lossy().as_bytes());
                hash.update([0]);
            }
            hash.finalize()
                .iter()
                .take(6)
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let stem = files[0].file_stem().map_or_else(
            || "picture".into(),
            |stem| stem.to_string_lossy().into_owned(),
        );
        let mut arguments = Vec::new();
        for file in files {
            arguments.push("--file".into());
            arguments.push(file.into());
        }
        self.prepare_and_open(&format!("{stem}-{identity}"), arguments, None);
    }

    /// Asks which stack files to open, then opens them.
    pub(super) fn pick_files(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(files))) = picked.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.open_files(files);
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens the links the system handed this program, one at a time.
    pub(super) fn follow_links(&mut self) {
        if self.is_running() {
            return;
        }
        // The newest link wins: links clicked while a picture was being
        // prepared would otherwise replace the picture one after another.
        let link = {
            let mut links = LINKS.lock().expect("links");
            let newest = links.pop();
            links.clear();
            newest
        };
        let Some(link) = link else {
            return;
        };
        match target_link(&link) {
            Some(target) => self.open_link(target),
            None => self.say("Not opened: the link does not name a target on a server"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_names_a_target_on_a_server_and_nothing_else_is_taken() {
        assert_eq!(
            target_link(
                "psf-studio://open?server=http%3A%2F%2Flocalhost%3A3000&db=my-db&project=11&target=17"
            ),
            Some(TargetLink {
                server: "http://localhost:3000".to_string(),
                database: "my-db".to_string(),
                project: 11,
                target: 17,
                ticket: None,
            })
        );
        assert_eq!(
            target_link(
                "psf-studio://open?server=https%3A%2F%2Fguard&db=a&project=1&target=2&ticket=ab12"
            )
            .and_then(|link| link.ticket),
            Some("ab12".to_string())
        );
        // A server that is not HTTP, a missing field and another scheme.
        assert_eq!(
            target_link("psf-studio://open?server=file%3A%2F%2F%2Fetc&db=a&project=1&target=2"),
            None
        );
        assert_eq!(
            target_link("psf-studio://open?server=http%3A%2F%2Fa&db=a&project=1"),
            None
        );
        assert_eq!(
            target_link("https://example.com/open?server=http://a"),
            None
        );
        assert_eq!(file_name("my db/../x"), "my-db----x");
    }
}
