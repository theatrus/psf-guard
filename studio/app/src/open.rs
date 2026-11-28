//! Opening a picture from inside Studio.
//!
//! A picture is opened from stack files a person picks, or from a link a
//! PSF Guard page sends: `psf-studio://open?server=…&db=…&project=…&target=…`.
//! Studio runs `psf-guard studio`, the command a person can run by hand,
//! when PSF Guard is on the computer: it fetches, registers and prepares
//! the stacks, runs the processing tools and calibrates colors against
//! Gaia. Without PSF Guard, Studio prepares stack files itself with the
//! same code, from `psf-studio-core`; their tools wait for PSF Guard and
//! their colors stay uncalibrated. Either way Studio then shows the picture
//! it prepared.
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
    super::servers::settings_folder().join("pictures")
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
    /// The Studio document to open; the target's latest when there is none.
    pub document: Option<String>,
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
        document: field(query, "document").filter(|document| !document.is_empty()),
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

    /// Opens stack files, one per channel, each file's channel read from
    /// its FILTER header or its name.
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
        let name = format!("{stem}-{identity}");
        if !self.psf_guard_installed() {
            self.prepare_files_here(&name, files);
            return;
        }
        let mut arguments = Vec::new();
        for file in files {
            arguments.push("--file".into());
            arguments.push(file.into());
        }
        self.prepare_and_open(&name, arguments, None);
    }

    /// Prepares the picture `name` from stack files in this program, on a
    /// thread of its own, and opens it when it is ready.
    fn prepare_files_here(&mut self, name: &str, files: Vec<PathBuf>) {
        let folder = pictures_folder();
        let name = file_name(name);
        let store = folder.join(format!("{name}.pgtiles"));
        let document = folder.join(format!("{name}.json"));
        let opens = Some((store.clone(), document.clone()));
        self.start_job(opens, move |run| {
            prepare_files(
                &files,
                &store,
                &document,
                &|line| super::tools::set_line(run, line),
                &|| super::tools::is_stopped(run),
            )
            .map_err(|error| format!("{error:#}"))
        });
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

/// Prepares a picture from stack files: its store at `store`, and its
/// document at `document`, which is kept when it is there already. Each
/// file's channel comes from its FILTER header or its name. `line` is
/// called with what is being done; `stopped` says whether a person stopped
/// the run. The files are listed in the store's sources folder, so PSF
/// Guard's Run tools can prepare the picture again once it is installed.
fn prepare_files(
    files: &[PathBuf],
    store: &std::path::Path,
    document: &std::path::Path,
    line: &dyn Fn(&str),
    stopped: &dyn Fn() -> bool,
) -> anyhow::Result<()> {
    use psf_studio_core::prepare::{
        channel_of_file, picture_document, picture_title, prepare_store, write_atomically,
        write_file_sources, Calibration, NoTools, Prepare, StackFile,
    };
    if let Some(folder) = store.parent() {
        std::fs::create_dir_all(folder)?;
    }
    line("Reading the stack files");
    let mut stacks: Vec<StackFile> = Vec::new();
    for file in files {
        let role = channel_of_file(file).map_err(|error| {
            error.context(
                "Rename the file to end in its filter, such as M42_Ha.fits, or set its FILTER header",
            )
        })?;
        if let Some(other) = stacks.iter().find(|stack| stack.role == role) {
            anyhow::bail!(
                "{} and {} are both {}: open one file per channel",
                other.path.display(),
                file.display(),
                role.label()
            );
        }
        stacks.push(StackFile::new(role, file)?);
    }
    let Some(first) = stacks.first() else {
        anyhow::bail!("no stack files were given");
    };
    let header = seiza_image_io::read_header(&first.path)?;
    let title = picture_title(&header.cards, &first.path);
    let roles: Vec<_> = stacks.iter().map(|stack| stack.role).collect();
    let saved = document.is_file().then_some(document);
    let picture = picture_document(&title, &roles, saved)?;
    // Colors are calibrated against Gaia from a plate solve, which PSF
    // Guard makes. A step that is off, or whose settings turn it off, asks
    // for nothing.
    let wants_calibration = {
        use crate::studio_model::{flow_operations, StageKind, StepKind};
        let rgb = ["red", "green", "blue"]
            .iter()
            .all(|channel| picture.channels().iter().any(|own| own == channel));
        let step = picture
            .stage(StageKind::Linear)
            .steps
            .iter()
            .find(|step| step.kind.name() == flow_operations::COLOR_CALIBRATION);
        let on = step.is_none_or(|step| {
            step.enabled
                && match &step.kind {
                    StepKind::Operation { settings, .. } => {
                        settings["enabled"].as_bool().unwrap_or(true)
                    }
                    _ => true,
                }
        });
        rgb && on
    };
    let prepare_stacks: Vec<_> = stacks.iter().map(StackFile::stack).collect();
    let folder = store.with_extension("sources");
    let shape = prepare_store(Prepare {
        stacks: &prepare_stacks,
        document: &picture,
        tools: None::<&NoTools>,
        folder: &folder,
        output: store,
        calibrate: &|_| {
            if wants_calibration {
                Calibration::Skipped {
                    why: "Gaia color calibration needs PSF Guard, which is not on this computer"
                        .into(),
                }
            } else {
                Calibration::NotAsked
            }
        },
        report: &|text| line(text.trim()),
        progress: &|channel, tool, fraction| {
            line(&format!("{channel}: {tool} ({:.0}%)", fraction * 100.0))
        },
        stopped,
    })?;
    write_file_sources(&folder, &stacks, (shape.width, shape.height))?;
    if saved.is_none() {
        write_atomically(document, picture.to_json())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_stack_again_keeps_the_document_in_a_path_with_spaces_and_unicode() {
        use seiza_fits::{write_f32_image, F32ImageData, HeaderValue, WriteHeaderCard};

        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("Nébuleuse L.fits");
        let (width, height) = (192, 128);
        let stars: Vec<(f32, f32)> = (0..24)
            .map(|at| ((12 + at * 37 % 168) as f32, (10 + at * 23 % 108) as f32))
            .collect();
        let mut seed = 7_u32;
        let pixels: Vec<f32> = (0..width * height)
            .map(|at| {
                let (x, y) = ((at % width) as f32, (at / width) as f32);
                let starlight: f32 = stars
                    .iter()
                    .map(|(cx, cy)| 0.6 * (-((x - cx).powi(2) + (y - cy).powi(2)) / 4.0).exp())
                    .sum();
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 8) as f32 / (1 << 24) as f32 - 0.5;
                65535.0 * (0.05 + 0.02 * x / width as f32 + 0.002 * noise + starlight)
            })
            .collect();
        write_f32_image(
            &path,
            width,
            height,
            F32ImageData::Mono(&pixels),
            &[
                WriteHeaderCard::new("OBJECT", HeaderValue::String("M 42".into())),
                WriteHeaderCard::new("FILTER", HeaderValue::String("L".into())),
                WriteHeaderCard::new("IMAGETYP", HeaderValue::String("LIGHT".into())),
            ],
        )
        .unwrap();
        let workspace = folder.path().join("Studio pictures");
        let store = workspace.join("M 42.pgtiles");
        let document = workspace.join("M 42.json");

        let files = [path];
        prepare_files(&files, &store, &document, &|_| {}).unwrap();
        let saved = std::fs::read_to_string(&document).unwrap();
        let mut picture = crate::studio_model::Document::from_json(&saved).unwrap();
        assert_eq!(picture.name, "M 42");
        picture.name = "Edited M 42".into();
        std::fs::write(&document, picture.to_json()).unwrap();

        prepare_files(&files, &store, &document, &|_| {}).unwrap();
        assert_eq!(
            std::fs::read_to_string(document).unwrap(),
            picture.to_json()
        );
        assert!(crate::store_tiles::open_store(&store).is_ok());
    }

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
                document: None,
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
