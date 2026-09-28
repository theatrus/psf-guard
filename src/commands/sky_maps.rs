//! `psf-guard sky-maps`: install and list N.I.N.A. offline sky maps under a
//! cache root, where the Director framing view picks them up as layers.

use anyhow::{bail, Context, Result};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Instant,
};

use crate::cli::SkyMapsCommand;
use crate::sky_maps::{self, root_for as sky_maps_root};

const RELEASES: &str = "https://nighttime-imaging.eu/downloads/Setup/Releases/";
const MAX_ZIP_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// The sets N.I.N.A. offers on its download page, by short name.
fn source_url(source: &str) -> Result<String> {
    Ok(match source {
        "full" | "dss" => format!("{RELEASES}FramingAssistantCache_Full.zip"),
        "nsns-ohs" | "nsns" => {
            format!("{RELEASES}FramingAssistantCache_NorthernSkyNarrowbandSurvey_OHS_withStars.zip")
        }
        "nsns-ohs-starless" | "nsns-starless" => {
            format!("{RELEASES}FramingAssistantCache_NorthernSkyNarrowbandSurvey_OHS_starless.zip")
        }
        url if url.starts_with("https://") || url.starts_with("http://") => url.to_owned(),
        other => {
            bail!("unknown set '{other}'; use full, nsns-ohs, nsns-ohs-starless, or a zip URL")
        }
    })
}

pub fn run(action: SkyMapsCommand) -> Result<()> {
    match action {
        SkyMapsCommand::List { cache_dir } => list(&sky_maps_root(Path::new(&cache_dir))),
        SkyMapsCommand::Install { source, cache_dir } => {
            let root = sky_maps_root(Path::new(&cache_dir));
            let url = source_url(&source)?;
            std::fs::create_dir_all(&root)
                .with_context(|| format!("creating {}", root.display()))?;
            let zip_path = root.join(format!(".download-{}.zip", std::process::id()));
            let result = download(&url, &zip_path).and_then(|_| unpack(&zip_path, &root));
            let _ = std::fs::remove_file(&zip_path);
            let folder = result?;
            let map = sky_maps::SkyMap::open(&folder)
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("the unpacked set is not a framing assistant cache")?;
            println!(
                "Installed {} ({} tiles) as layer {} under {}",
                map.name,
                map.tile_count(),
                map.id,
                root.display()
            );
            println!("A running server offers it within half a minute; no restart needed.");
            Ok(())
        }
    }
}

fn list(root: &Path) -> Result<()> {
    let maps = sky_maps::discover(root);
    if maps.is_empty() {
        println!("No offline sky maps under {}.", root.display());
        println!("Install one with: psf-guard sky-maps install full --cache-dir <cache>");
        return Ok(());
    }
    for map in maps {
        println!(
            "{}\t{}\t{} tiles\t{}",
            map.id,
            map.name,
            map.tile_count(),
            map.dir.display()
        );
    }
    Ok(())
}

/// Stream the zip to disk with a line of progress; these sets are gigabytes.
fn download(url: &str, to: &Path) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .user_agent(concat!("psf-guard/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let response = client
            .get(url)
            .send()
            .await
            .with_context(|| format!("requesting {url}"))?;
        if !response.status().is_success() {
            bail!("{url} answered {}", response.status());
        }
        let total = response.content_length();
        if total.is_some_and(|t| t > MAX_ZIP_BYTES) {
            bail!(
                "{url} is larger than {} GB",
                MAX_ZIP_BYTES / (1024 * 1024 * 1024)
            );
        }
        let mut file =
            std::fs::File::create(to).with_context(|| format!("creating {}", to.display()))?;
        let mut response = response;
        let mut received: u64 = 0;
        let mut last_report = Instant::now();
        let started = Instant::now();
        eprintln!("Downloading {url}");
        while let Some(chunk) = response.chunk().await.context("reading the download")? {
            file.write_all(&chunk)?;
            received += chunk.len() as u64;
            if received > MAX_ZIP_BYTES {
                bail!(
                    "download exceeded {} GB",
                    MAX_ZIP_BYTES / (1024 * 1024 * 1024)
                );
            }
            if last_report.elapsed().as_secs() >= 5 {
                last_report = Instant::now();
                let mb = received as f64 / 1_048_576.0;
                match total {
                    Some(t) => eprintln!("  {mb:.0} MB of {:.0} MB", t as f64 / 1_048_576.0),
                    None => eprintln!("  {mb:.0} MB"),
                }
            }
        }
        file.sync_all()?;
        eprintln!(
            "  {:.0} MB in {:.0} s",
            received as f64 / 1_048_576.0,
            started.elapsed().as_secs_f64()
        );
        Ok(())
    })
}

/// Unpack into `root`, refusing entries that would leave it, and return the
/// folder that holds `CacheInfo.xml`.
fn unpack(zip_path: &Path, root: &Path) -> Result<PathBuf> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file).context("reading the zip")?;
    let mut index_dir: Option<PathBuf> = None;
    eprintln!("Unpacking {} entries", archive.len());
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(relative) = entry.enclosed_name() else {
            bail!("zip entry '{}' would leave the folder", entry.name());
        };
        let target = root.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)
            .with_context(|| format!("writing {}", target.display()))?;
        let mut buffer = [0u8; 1 << 16];
        loop {
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            out.write_all(&buffer[..read])?;
        }
        if relative.file_name().and_then(|n| n.to_str()) == Some("CacheInfo.xml") {
            index_dir = target.parent().map(Path::to_path_buf);
        }
    }
    index_dir.ok_or_else(|| anyhow::anyhow!("the zip holds no CacheInfo.xml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names_map_to_the_published_sets_and_urls_pass_through() {
        assert!(source_url("full")
            .unwrap()
            .ends_with("FramingAssistantCache_Full.zip"));
        assert!(source_url("nsns-ohs").unwrap().contains("OHS_withStars"));
        assert!(source_url("nsns-ohs-starless")
            .unwrap()
            .contains("OHS_starless"));
        assert_eq!(
            source_url("https://x.example/a.zip").unwrap(),
            "https://x.example/a.zip"
        );
        assert!(source_url("moon").is_err());
    }

    #[test]
    fn unpacking_keeps_entries_inside_the_root_and_finds_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("set.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            writer.add_directory("Set/", options).unwrap();
            writer.start_file("Set/CacheInfo.xml", options).unwrap();
            writer.write_all(b"<ImageCacheInfo/>").unwrap();
            writer.start_file("Set/t.jpg", options).unwrap();
            writer.write_all(b"jpg").unwrap();
            writer.finish().unwrap();
        }
        let root = dir.path().join("maps");
        let folder = unpack(&zip_path, &root).unwrap();
        assert_eq!(folder, root.join("Set"));
        assert!(root.join("Set").join("t.jpg").exists());
        let evil = dir.path().join("evil.zip");
        {
            let file = std::fs::File::create(&evil).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file("../escape.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"x").unwrap();
            writer.finish().unwrap();
        }
        assert!(unpack(&evil, &root).is_err());
        assert!(!dir.path().join("escape.txt").exists());
    }
}
