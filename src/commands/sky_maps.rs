//! `psf-guard sky-maps`: install and list N.I.N.A. offline sky maps, which
//! Planning's framing view offers as layers. Settings › Offline sky maps
//! installs the same sets through the same code.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::cli::SkyMapsCommand;
use crate::sky_map_install::{self as install, Source, Step};
use crate::sky_maps;

/// What to install for a name or a URL given on the command line.
fn source(given: &str) -> Result<Source> {
    if let Some(set) = install::set(given) {
        return Ok(Source::Set(set));
    }
    if given.starts_with("https://") || given.starts_with("http://") {
        return Ok(Source::Url(given.to_owned()));
    }
    bail!("unknown set '{given}'; use full, nsns-ohs, nsns-ohs-starless, or a zip URL")
}

/// The maps' folder: `--dir`, else the default.
fn root(dir: Option<String>) -> PathBuf {
    dir.map(PathBuf::from)
        .unwrap_or_else(sky_maps::default_root)
}

pub fn run(action: SkyMapsCommand) -> Result<()> {
    match action {
        SkyMapsCommand::List { dir, cache_dir } => {
            list(&root(dir), &sky_maps::root_for(Path::new(&cache_dir)));
            Ok(())
        }
        SkyMapsCommand::Install { source: given, dir } => {
            let root = root(dir);
            let source = source(&given)?;
            let started = Instant::now();
            // One line every few seconds; these sets are gigabytes.
            let last = std::sync::Mutex::new(Instant::now() - std::time::Duration::from_secs(60));
            let report = move |step: Step| {
                let mut last = last.lock().unwrap();
                match (step.bytes_done, step.bytes_total) {
                    (Some(done), total) => {
                        if last.elapsed().as_secs() < 5 {
                            return;
                        }
                        let megabytes = done as f64 / 1_048_576.0;
                        match total {
                            Some(total) => eprintln!(
                                "  {megabytes:.0} MB of {:.0} MB",
                                total as f64 / 1_048_576.0
                            ),
                            None => eprintln!("  {megabytes:.0} MB"),
                        }
                    }
                    _ => eprintln!("{}", step.message),
                }
                *last = Instant::now();
            };
            eprintln!("Installing into {}", root.display());
            let runtime = tokio::runtime::Runtime::new()?;
            let map = runtime
                .block_on(install::install(
                    &root,
                    source,
                    crate::concurrency::Stop::new(),
                    || (),
                    &report,
                ))
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            println!(
                "Installed {} ({} tiles) as layer {} in {:.0} s",
                map.name,
                map.tile_count(),
                map.id,
                started.elapsed().as_secs_f64()
            );
            println!("A running server offers it within half a minute; no restart needed.");
            Ok(())
        }
    }
}

fn list(own: &Path, older: &Path) {
    let mut found = false;
    for (root, note) in [(own, ""), (older, " (under the cache folder)")] {
        for map in sky_maps::discover(root) {
            found = true;
            println!(
                "{}\t{}\t{} tiles\t{}{note}",
                map.id,
                map.name,
                map.tile_count(),
                map.dir.display()
            );
        }
    }
    if !found {
        println!("No offline sky maps in {}.", own.display());
        println!("Install one with: psf-guard sky-maps install full");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names_are_the_published_sets_and_a_url_is_taken_as_given() {
        for (given, file) in [
            ("full", "FramingAssistantCache_Full.zip"),
            ("dss", "FramingAssistantCache_Full.zip"),
            ("nsns-ohs", "OHS_withStars.zip"),
            ("nsns-ohs-starless", "OHS_starless.zip"),
        ] {
            match source(given).unwrap() {
                Source::Set(set) => assert!(set.url().ends_with(file), "{given}"),
                Source::Url(url) => panic!("{given} was taken as the URL {url}"),
            }
        }
        assert_eq!(
            source("https://x.example/a.zip").unwrap(),
            Source::Url("https://x.example/a.zip".into())
        );
        assert!(source("moon").is_err());
    }

    #[test]
    fn the_folder_is_the_one_given_or_the_default() {
        assert_eq!(root(Some("/maps".into())), PathBuf::from("/maps"));
        assert_eq!(root(None), sky_maps::default_root());
    }
}
