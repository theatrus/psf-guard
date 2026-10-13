//! Which files an import reads, and which lights it keeps.
//!
//! A database's scan rules leave parts of its image folders out of every
//! import: exclude patterns skip files and whole folders before any header
//! is read, include patterns keep only the files that match, and the light
//! filters drop frames whose headers do not look like a light worth a
//! scheduler row. A `.psfguardignore` file skips its folder on every import,
//! with or without rules. See `docs/IMPORTING.md`.

use super::headers::FrameMeta;
use anyhow::{bail, Context, Result};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A file of this name skips the folder it sits in and everything below
/// it. Its contents are not read.
pub const IGNORE_FILE: &str = ".psfguardignore";

/// A database's scan rules, as stored in its registry entry. The defaults
/// change nothing: every image file is read, every frame that looks like a
/// light is one, and a light attaches to a target by name or by position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanRules {
    /// Patterns for files and folders to leave out, relative to the image
    /// folder that holds them. A pattern that matches a folder skips
    /// everything below it.
    pub exclude: Vec<String>,
    /// When any are set, only files matching one of these, or inside a
    /// folder matching one, are read.
    pub include: Vec<String>,
    /// A frame with no IMAGETYP (or FRAME) is left out instead of being
    /// taken for a light.
    pub lights_must_say_light: bool,
    /// A light with no OBJECT is left out instead of becoming
    /// "Unknown Target".
    pub require_object: bool,
    /// Lights shorter than this, or of unknown length, are left out.
    pub min_exposure_seconds: Option<f64>,
    /// Attach a light whose OBJECT matches no target name to a target
    /// within 0.5° of it, including a light with no OBJECT. On by default;
    /// off, only an exact name match attaches.
    pub match_by_position: bool,
}

impl Default for ScanRules {
    fn default() -> Self {
        Self {
            exclude: Vec::new(),
            include: Vec::new(),
            lights_must_say_light: false,
            require_object: false,
            min_exposure_seconds: None,
            match_by_position: true,
        }
    }
}

impl ScanRules {
    /// The same rules with blank patterns dropped and the rest trimmed, as
    /// they are stored.
    pub fn normalized(&self) -> Self {
        let clean = |patterns: &[String]| {
            patterns
                .iter()
                .map(|pattern| pattern.trim().to_string())
                .filter(|pattern| !pattern.is_empty())
                .collect()
        };
        Self {
            exclude: clean(&self.exclude),
            include: clean(&self.include),
            min_exposure_seconds: self.min_exposure_seconds.filter(|seconds| *seconds > 0.0),
            ..self.clone()
        }
    }

    /// Check every pattern and the exposure. The error says which pattern
    /// is wrong and why.
    pub fn validate(&self) -> Result<()> {
        if let Some(seconds) = self.min_exposure_seconds
            && !(seconds.is_finite() && seconds >= 0.0)
        {
            bail!("the shortest exposure must be a number of seconds, 0 or more");
        }
        self.path_filter(&[]).map(|_| ())
    }

    /// The patterns, compiled for a walk over `roots`, the database's image
    /// folders. A path is matched relative to the root that holds it.
    pub fn path_filter(&self, roots: &[PathBuf]) -> Result<PathFilter> {
        let mut filter = PathFilter {
            roots: roots.to_vec(),
            ..PathFilter::default()
        };
        if !self.exclude.is_empty() {
            let mut files = GlobSetBuilder::new();
            let mut folders = GlobSetBuilder::new();
            for raw in &self.exclude {
                let Some(pattern) = normalize_pattern(raw) else {
                    continue;
                };
                files.add(compile("exclude", raw, &pattern)?);
                folders.add(compile("exclude", raw, &pattern)?);
                // `**/Hocus Focus/**` matches what is below a folder; the
                // folder itself is skipped too, so the walk never enters it.
                if let Some(folder) = pattern.strip_suffix("/**")
                    && !folder.is_empty()
                    && folder != "**"
                {
                    folders.add(compile("exclude", raw, folder)?);
                }
            }
            filter.exclude_files = Some(files.build().context("building exclude patterns")?);
            filter.exclude_folders = Some(folders.build().context("building exclude patterns")?);
        }
        if !self.include.is_empty() {
            let mut files = GlobSetBuilder::new();
            for raw in &self.include {
                let Some(pattern) = normalize_pattern(raw) else {
                    continue;
                };
                files.add(compile("include", raw, &pattern)?);
            }
            filter.include = Some(files.build().context("building include patterns")?);
        }
        Ok(filter)
    }

    /// The filters applied to frames that would become lights.
    pub fn light_filter(&self) -> LightFilter {
        LightFilter {
            must_say_light: self.lights_must_say_light,
            require_object: self.require_object,
            min_exposure_seconds: self.min_exposure_seconds.filter(|seconds| *seconds > 0.0),
        }
    }
}

/// Turn a pattern as a person typed it into the form the matcher takes:
/// `\` and `/` both separate folders, a leading `/` ties the pattern to the
/// image folder, and a pattern with no separator matches a file or folder
/// name at any depth, as in `.gitignore`.
fn normalize_pattern(raw: &str) -> Option<String> {
    let mut pattern = raw.trim().replace('\\', "/");
    while pattern.starts_with("./") {
        pattern.drain(..2);
    }
    while pattern.len() > 1 && pattern.ends_with('/') {
        pattern.pop();
    }
    if pattern.is_empty() || pattern == "/" {
        return None;
    }
    if let Some(anchored) = pattern.strip_prefix('/') {
        return Some(anchored.to_string());
    }
    if pattern.contains('/') {
        Some(pattern)
    } else {
        Some(format!("**/{pattern}"))
    }
}

fn compile(list: &str, raw: &str, pattern: &str) -> Result<globset::Glob> {
    GlobBuilder::new(pattern)
        .case_insensitive(true)
        .literal_separator(true)
        .backslash_escape(false)
        .build()
        .map_err(|error| {
            anyhow::anyhow!(
                "{list} pattern \"{}\" is not valid: {}",
                raw.trim(),
                error.kind()
            )
        })
}

/// Compiled exclude and include patterns for one walk.
#[derive(Debug, Clone, Default)]
pub struct PathFilter {
    roots: Vec<PathBuf>,
    exclude_files: Option<GlobSet>,
    exclude_folders: Option<GlobSet>,
    include: Option<GlobSet>,
}

impl PathFilter {
    /// `path` relative to the image folder that holds it (the longest
    /// configured root, else the folder being walked), with `/` between
    /// parts whatever the platform.
    fn relative(&self, path: &Path, walk_root: &Path) -> String {
        let root = self
            .roots
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
            .map(PathBuf::as_path)
            .unwrap_or(walk_root);
        let relative = path.strip_prefix(root).unwrap_or(path);
        relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Whether an exclude pattern skips this folder and everything below.
    pub fn skips_folder(&self, folder: &Path, walk_root: &Path) -> bool {
        let Some(patterns) = &self.exclude_folders else {
            return false;
        };
        let relative = self.relative(folder, walk_root);
        !relative.is_empty() && patterns.is_match(&relative)
    }

    /// Whether the patterns keep this file. An include pattern keeps it
    /// when it matches the file or any folder above it, so `LIGHT` keeps
    /// everything in N.I.N.A.'s `…/LIGHT/` folders.
    pub fn keeps_file(&self, file: &Path, walk_root: &Path) -> bool {
        if self.exclude_files.is_none() && self.include.is_none() {
            return true;
        }
        let relative = self.relative(file, walk_root);
        if self
            .exclude_files
            .as_ref()
            .is_some_and(|patterns| patterns.is_match(&relative))
        {
            return false;
        }
        self.include.as_ref().is_none_or(|patterns| {
            // The file's path, then each folder above it up to the root.
            std::iter::successors(Some(relative.as_str()), |path| {
                path.rfind('/').map(|end| &path[..end])
            })
            .any(|path| patterns.is_match(path))
        })
    }
}

/// What the scan rules left out of a walk, unread.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanRuleReport {
    /// Image files an exclude or include pattern left out.
    pub files_skipped: usize,
    /// Folders an exclude pattern left out, with everything below them.
    pub folders_skipped: usize,
    /// Folders a `.psfguardignore` file left out, with everything below.
    pub ignored_folders: usize,
}

impl ScanRuleReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Why a frame that looked like a light was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeftOutReason {
    /// No IMAGETYP or FRAME, and the rules want lights to say LIGHT.
    NoImageType,
    /// No OBJECT, and the rules want a target name.
    NoTargetName,
    /// Shorter than the rules' shortest exposure, or of unknown length.
    ShortExposure,
}

/// The light filters of a database's scan rules.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LightFilter {
    pub must_say_light: bool,
    pub require_object: bool,
    pub min_exposure_seconds: Option<f64>,
}

impl LightFilter {
    /// Why `frame`, which would otherwise become a light, is left out, or
    /// `None` when it stays.
    pub fn refuses(&self, frame: &FrameMeta) -> Option<LeftOutReason> {
        if self.must_say_light && frame.image_type.is_none() {
            return Some(LeftOutReason::NoImageType);
        }
        if self.require_object && frame.object.is_none() {
            return Some(LeftOutReason::NoTargetName);
        }
        if let Some(minimum) = self.min_exposure_seconds
            && frame.exposure_s.is_none_or(|seconds| seconds < minimum)
        {
            return Some(LeftOutReason::ShortExposure);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(exclude: &[&str], include: &[&str]) -> ScanRules {
        ScanRules {
            exclude: exclude.iter().map(|s| s.to_string()).collect(),
            include: include.iter().map(|s| s.to_string()).collect(),
            ..ScanRules::default()
        }
    }

    fn root() -> PathBuf {
        PathBuf::from("/data/Images")
    }

    #[test]
    fn a_folder_pattern_skips_the_folder_in_any_case_and_with_either_slash() {
        let root = root();
        let folder = root.join("Hocus Focus");
        for pattern in [
            "**/Hocus Focus/**",
            "**/Hocus Focus",
            "Hocus Focus",
            "hocus focus",
            "**\\Hocus Focus\\**",
            "/Hocus Focus/",
        ] {
            let filter = rules(&[pattern], &[])
                .path_filter(std::slice::from_ref(&root))
                .unwrap();
            assert!(filter.skips_folder(&folder, &root), "{pattern}");
            assert!(
                !filter.skips_folder(&root.join("M31"), &root),
                "{pattern} must leave M31 alone"
            );
        }
    }

    #[test]
    fn a_nested_folder_matches_at_depth() {
        let root = root();
        let filter = rules(&["AutoFocus_*"], &[])
            .path_filter(std::slice::from_ref(&root))
            .unwrap();
        assert!(filter.skips_folder(&root.join("Hocus Focus/AutoFocus_20261001"), &root));
        assert!(!filter.skips_folder(&root.join("Hocus Focus"), &root));
    }

    #[test]
    fn an_anchored_pattern_only_matches_at_the_image_folder() {
        let root = root();
        let filter = rules(&["/FLAT"], &[])
            .path_filter(std::slice::from_ref(&root))
            .unwrap();
        assert!(filter.skips_folder(&root.join("FLAT"), &root));
        assert!(!filter.skips_folder(&root.join("M31/FLAT"), &root));
    }

    #[test]
    fn a_star_stays_inside_one_folder() {
        let root = root();
        let filter = rules(&["Test/*.fits"], &[])
            .path_filter(std::slice::from_ref(&root))
            .unwrap();
        assert!(!filter.keeps_file(&root.join("Test/a.fits"), &root));
        assert!(filter.keeps_file(&root.join("Test/deeper/a.fits"), &root));
    }

    #[test]
    fn file_patterns_and_include_patterns() {
        let root = root();
        let filter = rules(&["*_snapshot_*"], &["*LIGHT*"])
            .path_filter(std::slice::from_ref(&root))
            .unwrap();
        assert!(filter.keeps_file(&root.join("M31/2026-10-01_LIGHT_001.fits"), &root));
        assert!(!filter.keeps_file(&root.join("M31/2026-10-01_FLAT_001.fits"), &root));
        assert!(!filter.keeps_file(&root.join("M31/x_snapshot_LIGHT.fits"), &root));
    }

    #[test]
    fn an_include_pattern_keeps_files_inside_a_matching_folder() {
        let root = root();
        let filter = rules(&[], &["LIGHT"])
            .path_filter(std::slice::from_ref(&root))
            .unwrap();
        assert!(filter.keeps_file(&root.join("M31/2026-01-11/LIGHT/x.fits"), &root));
        assert!(filter.keeps_file(&root.join("LIGHT/x.fits"), &root));
        assert!(!filter.keeps_file(&root.join("M31/2026-01-11/FLAT/x.fits"), &root));
        // The folder name must match whole, not as part of a longer name.
        assert!(!filter.keeps_file(&root.join("M31/LIGHTS_OLD/x.fits"), &root));
        // A file-name pattern still matches the file.
        let filter = rules(&[], &["*.xisf"])
            .path_filter(std::slice::from_ref(&root))
            .unwrap();
        assert!(filter.keeps_file(&root.join("M31/LIGHT/x.xisf"), &root));
        assert!(!filter.keeps_file(&root.join("M31/LIGHT/x.fits"), &root));
    }

    #[test]
    fn paths_match_relative_to_the_configured_root_not_the_folder_walked() {
        let root = root();
        let filter = rules(&["/M31/Test"], &[])
            .path_filter(std::slice::from_ref(&root))
            .unwrap();
        // The wizard walks a folder inside the root; the pattern still
        // reads from the root.
        let walked = root.join("M31");
        assert!(filter.skips_folder(&walked.join("Test"), &walked));
    }

    #[test]
    fn a_bad_pattern_says_which_and_why() {
        let error = rules(&["**/ok/**", "[abc"], &[]).validate().unwrap_err();
        let text = error.to_string();
        assert!(text.contains("exclude pattern \"[abc\""), "{text}");
        let error = rules(&[], &["{a,b"]).validate().unwrap_err();
        assert!(error.to_string().contains("include pattern \"{a,b\""));
    }

    #[test]
    fn rules_that_only_keep_position_matching_on_are_the_defaults() {
        let stored: ScanRules = serde_json::from_str(r#"{"exclude":[]}"#).unwrap();
        assert!(stored.match_by_position);
        assert_eq!(stored, ScanRules::default());
        let off: ScanRules = serde_json::from_str(r#"{"match_by_position":false}"#).unwrap();
        assert_ne!(off.normalized(), ScanRules::default());
    }

    #[test]
    fn a_negative_exposure_is_refused() {
        let error = ScanRules {
            min_exposure_seconds: Some(-1.0),
            ..ScanRules::default()
        }
        .validate()
        .unwrap_err();
        assert!(error.to_string().contains("shortest exposure"));
    }

    #[test]
    fn normalized_drops_blank_patterns_and_a_zero_exposure() {
        let normalized = ScanRules {
            exclude: vec!["  **/Test/**  ".into(), "   ".into()],
            include: vec!["".into()],
            min_exposure_seconds: Some(0.0),
            ..ScanRules::default()
        }
        .normalized();
        assert_eq!(normalized.exclude, vec!["**/Test/**".to_string()]);
        assert!(normalized.include.is_empty());
        assert_eq!(normalized.min_exposure_seconds, None);
        assert_eq!(
            ScanRules {
                exclude: vec![" ".into()],
                ..ScanRules::default()
            }
            .normalized(),
            ScanRules::default()
        );
    }

    #[test]
    fn the_light_filter_gives_one_reason() {
        let light = FrameMeta {
            readable: true,
            image_type: Some("LIGHT".into()),
            object: Some("M31".into()),
            exposure_s: Some(120.0),
            ..FrameMeta::default()
        };
        let filter = LightFilter {
            must_say_light: true,
            require_object: true,
            min_exposure_seconds: Some(10.0),
        };
        assert_eq!(filter.refuses(&light), None);
        let untyped = FrameMeta {
            image_type: None,
            ..light.clone()
        };
        assert_eq!(filter.refuses(&untyped), Some(LeftOutReason::NoImageType));
        let unnamed = FrameMeta {
            object: None,
            ..light.clone()
        };
        assert_eq!(filter.refuses(&unnamed), Some(LeftOutReason::NoTargetName));
        let short = FrameMeta {
            exposure_s: Some(2.0),
            ..light.clone()
        };
        assert_eq!(filter.refuses(&short), Some(LeftOutReason::ShortExposure));
        let unknown = FrameMeta {
            exposure_s: None,
            ..light.clone()
        };
        assert_eq!(filter.refuses(&unknown), Some(LeftOutReason::ShortExposure));
        assert_eq!(LightFilter::default().refuses(&unnamed), None);
    }
}
