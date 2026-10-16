//! `psf-guard backup`: the archive Settings › Backups makes, from the
//! command line. It shares [`crate::backup`] with the server.

use crate::backup::{self, BackupScope, Hooks, Locations, Trigger};
use crate::db_registry::DbRegistry;
use anyhow::{Context, Result};
use std::path::PathBuf;

pub struct BackupArgs {
    pub destination: PathBuf,
    pub scope: BackupScope,
    pub include_secrets: bool,
    pub dry_run: bool,
    pub registry: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub director_meta: Option<PathBuf>,
}

pub fn run(args: BackupArgs) -> Result<()> {
    let registry_path = match args.registry {
        Some(path) => path,
        None => DbRegistry::default_path().context("resolving the default registry path")?,
    };
    if !registry_path.is_file() {
        anyhow::bail!("no registry at {}", registry_path.display());
    }
    let registry = DbRegistry::load_or_init(&registry_path)
        .with_context(|| format!("loading the registry at {}", registry_path.display()))?;
    let app_config = match &args.config {
        Some(path) => crate::config::Config::from_file(path)
            .with_context(|| format!("loading {}", path.display()))?,
        None => crate::config::Config::default(),
    };
    let (roots, _) = app_config
        .storage_config()
        .resolve(registry.storage.as_ref());
    let destination = std::path::absolute(&args.destination)
        .with_context(|| format!("resolving {}", args.destination.display()))?;
    let locations = Locations {
        registry: registry_path,
        server_config: args
            .config
            .map(|path| std::path::absolute(&path).unwrap_or(path)),
        director_meta: args.director_meta,
        cache_root: Some(roots.cache.clone()),
    };
    let plan = backup::plan(&locations, &registry, args.scope, args.include_secrets);
    let warnings = backup::destination_warnings(
        &destination,
        &registry,
        &[roots.cache, roots.stacks, roots.calibration],
    );

    println!(
        "{} backup of {} file(s), {} on disk now, {}:",
        args.scope.label(),
        plan.items.len(),
        human_bytes(plan.total_bytes()),
        if args.include_secrets {
            "secrets included"
        } else {
            "secrets left out"
        }
    );
    for item in &plan.items {
        let redacts = if item.redacts.is_empty() {
            String::new()
        } else {
            format!(" (without {})", item.redacts.join(", "))
        };
        println!(
            "  {:<44} {:>10}  {}{redacts}",
            item.archive_path,
            human_bytes(item.bytes),
            item.source.display()
        );
    }
    for omitted in &plan.omitted {
        println!("  left out: {} — {}", omitted.source, omitted.reason);
    }
    for catalog in plan.catalogs.iter().filter(|c| c.problem.is_some()) {
        println!(
            "  catalog {} left out: {}",
            catalog.id,
            catalog.problem.as_deref().unwrap_or_default()
        );
    }
    for warning in &warnings {
        println!("  warning: {warning}");
    }
    let name = backup::archive_name(chrono::Utc::now(), args.scope, Trigger::Cli);
    if args.dry_run {
        println!(
            "Dry run: would write {} (nothing written)",
            destination.join(&name).display()
        );
        return Ok(());
    }

    let last_file = std::cell::RefCell::new(None::<String>);
    let progress = |progress: &backup::Progress| {
        if progress.current != *last_file.borrow() {
            if let Some(current) = &progress.current {
                eprintln!("  copying {current}…");
            }
            *last_file.borrow_mut() = progress.current.clone();
        }
    };
    let hooks = Hooks {
        progress: &progress,
        ..Hooks::quiet()
    };
    let written = backup::write(
        &plan,
        &destination,
        Trigger::Cli,
        chrono::Utc::now(),
        &hooks,
    )?;
    for problem in &written.manifest.problems {
        eprintln!("  left out: {problem}");
    }
    println!(
        "Wrote {} ({})",
        written.path.display(),
        human_bytes(written.bytes)
    );
    Ok(())
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dry_run_writes_nothing_and_a_run_writes_one_archive() {
        let dir = tempfile::tempdir().unwrap();
        let registry = dir.path().join("registry.json");
        DbRegistry::default().save(&registry).unwrap();
        let destination = dir.path().join("out");
        let args = |dry_run| BackupArgs {
            destination: destination.clone(),
            scope: BackupScope::Meta,
            include_secrets: false,
            dry_run,
            registry: Some(registry.clone()),
            config: None,
            director_meta: None,
        };
        run(args(true)).unwrap();
        assert!(!destination.exists());
        run(args(false)).unwrap();
        let listed = backup::list(&destination);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].trigger, Trigger::Cli);
    }

    #[test]
    fn a_missing_registry_is_an_error_not_an_empty_backup() {
        let dir = tempfile::tempdir().unwrap();
        let error = run(BackupArgs {
            destination: dir.path().join("out"),
            scope: BackupScope::Full,
            include_secrets: false,
            dry_run: false,
            registry: Some(dir.path().join("absent.json")),
            config: None,
            director_meta: None,
        })
        .unwrap_err();
        assert!(error.to_string().contains("no registry"));
    }

    #[test]
    fn sizes_read_as_people_write_them() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1_500_000), "1.5 MB");
    }
}
