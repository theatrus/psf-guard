//! Sync a local catalog with a remote PSF Guard over the network.
//!
//! The local `sync` commands need both databases on one filesystem. This one
//! needs only a URL and a key, so the review machine and the telescope can be
//! different machines — which is the arrangement the protocol was built for.
//!
//! Direction decides who holds the preview. Pulling, we build it here from a
//! bundle the peer sent, and the write is ours. Pushing, the peer holds it and
//! will only apply an ID it issued, so nothing is written there until we ask
//! for that ID by name.

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::server::database_context::open_scheduler_connection_with_flags;
use crate::server::remote_sync::SyncOperation;
use crate::sync_client::{local_bundle, materialize, SyncClient};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteDirection {
    /// Bring the peer's structure and captures down into the local catalog.
    Pull,
    /// Send local planning settings up to the peer.
    PushPlanning,
    /// Send local grading decisions up to the peer.
    PushGrades,
}

impl RemoteDirection {
    fn operation(self) -> SyncOperation {
        match self {
            RemoteDirection::Pull => SyncOperation::Merge,
            RemoteDirection::PushPlanning => SyncOperation::PushPlanning,
            RemoteDirection::PushGrades => SyncOperation::PushGrades,
        }
    }
}

pub struct RemoteSyncOptions {
    pub direction: RemoteDirection,
    /// Local scheduler database. Read for a push, written for a pull.
    pub local_path: PathBuf,
    /// How the local catalog names itself in the bundle it sends.
    pub local_id: String,
    pub peer_url: String,
    pub peer_token: String,
    /// Which of the peer's catalogs to use. Defaults to the one its key opens,
    /// which is the only one it will accept anyway.
    pub peer_catalog: Option<String>,
    /// For a grade push: send only rows someone has actually graded.
    pub reviewed_only: bool,
    /// Show what would change and stop.
    pub dry_run: bool,
    pub with_image_data: bool,
}

pub struct RemoteSyncOutcome {
    pub applied: bool,
    pub summary: BTreeMap<String, i64>,
    pub peer_product: String,
    pub peer_catalog: String,
}

pub async fn sync_remote(options: RemoteSyncOptions) -> Result<RemoteSyncOutcome> {
    let client = SyncClient::new(&options.peer_url, &options.peer_token)?;
    let capabilities = client.capabilities().await?;
    if capabilities.protocol_version != 1 {
        bail!(
            "{} speaks sync protocol {}; this build speaks 1",
            client.base_url(),
            capabilities.protocol_version
        );
    }
    let catalog = match &options.peer_catalog {
        Some(requested) => requested.clone(),
        None => capabilities.catalog()?.id.clone(),
    };
    let peer_product = format!("{} {}", capabilities.product, capabilities.product_version);
    println!(
        "Peer {} — {peer_product}, catalog {catalog}",
        client.base_url()
    );

    let summary = match options.direction {
        RemoteDirection::Pull => pull_from(&client, &catalog, &options).await?,
        RemoteDirection::PushPlanning | RemoteDirection::PushGrades => {
            push_to(&client, &catalog, &options).await?
        }
    };
    Ok(RemoteSyncOutcome {
        applied: !options.dry_run,
        summary,
        peer_product,
        peer_catalog: catalog,
    })
}

/// Peer → here. We fetch a bundle, stage it as a throwaway source database,
/// and hand it to the local merge engine, which is the same code a local
/// `sync pull` runs.
async fn pull_from(
    client: &SyncClient,
    catalog: &str,
    options: &RemoteSyncOptions,
) -> Result<BTreeMap<String, i64>> {
    use crate::commands::sync::{require_pull_capable, sync_pull, PullOptions};

    let bundle = client.export(catalog, SyncOperation::Merge, false).await?;
    let rows: usize = bundle.tables.values().map(|table| table.rows.len()).sum();
    println!(
        "Fetched {rows} row(s) across {} table(s) from {catalog}",
        bundle.tables.len()
    );

    // A named temporary file, not an in-memory database: the merge engine
    // opens the source by path.
    let staged = tempfile::Builder::new()
        .prefix("psf-guard-sync-")
        .suffix(".sqlite")
        .tempfile()
        .context("creating a staging file for the peer's bundle")?;
    let staged_path = staged.path().to_path_buf();
    materialize(&bundle, &staged_path, &options.local_path)
        .context("staging the peer's bundle as a scheduler database")?;

    let source =
        open_scheduler_connection_with_flags(&staged_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("opening the staged bundle")?;
    let destination = open_local(&options.local_path, options.dry_run)?;
    require_pull_capable(&source).context("the peer's catalog")?;
    require_pull_capable(&destination).context("the local database")?;

    let summary = sync_pull(
        &source,
        &destination,
        &PullOptions {
            dry_run: options.dry_run,
            with_image_data: options.with_image_data,
            project_filter: None,
        },
    )?;
    Ok(pull_summary(&summary))
}

/// Here → peer. The peer reviews and holds the preview; nothing is written
/// there until we apply the ID it hands back.
async fn push_to(
    client: &SyncClient,
    catalog: &str,
    options: &RemoteSyncOptions,
) -> Result<BTreeMap<String, i64>> {
    let operation = options.direction.operation();
    let bundle = local_bundle(
        &options.local_path,
        &options.local_id,
        operation,
        options.reviewed_only,
        options.with_image_data,
    )
    .context("building a bundle from the local database")?;
    let rows: usize = bundle.tables.values().map(|table| table.rows.len()).sum();
    println!(
        "Sending {rows} row(s) across {} table(s)",
        bundle.tables.len()
    );

    let preview = client.preview(catalog, operation, &bundle).await?;
    print_summary(&preview.summary);
    if options.dry_run {
        println!(
            "Dry run — nothing written. The peer holds preview {} until {}.",
            preview.preview_id, preview.expires_at
        );
        return Ok(preview.summary);
    }

    let applied = client.apply(&preview.preview_id).await.with_context(|| {
        format!(
            "the peer kept preview {} — if its catalog changed under the \
             preview, nothing was written",
            preview.preview_id
        )
    })?;
    Ok(applied.summary)
}

/// Open the local catalog. A dry run still needs write access, because the
/// merge engine runs the same statements inside a transaction it rolls back —
/// better to fail here than halfway through.
fn open_local(path: &Path, dry_run: bool) -> Result<Connection> {
    open_scheduler_connection_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).with_context(
        || {
            format!(
                "opening the local database {} for {}",
                path.display(),
                if dry_run { "a dry run" } else { "writing" }
            )
        },
    )
}

fn pull_summary(summary: &crate::commands::sync::PullSummary) -> BTreeMap<String, i64> {
    let mut counts = BTreeMap::from([
        ("total_inserted".into(), summary.total_inserted() as i64),
        ("total_updated".into(), summary.total_updated() as i64),
        ("grade_filled".into(), summary.grade_filled as i64),
        ("grade_preserved".into(), summary.grade_preserved as i64),
        ("imagedata_bytes".into(), summary.imagedata_bytes as i64),
    ]);
    for (name, table) in [
        ("exposuretemplate", &summary.exposuretemplate),
        ("project", &summary.project),
        ("ruleweight", &summary.ruleweight),
        ("target", &summary.target),
        ("exposureplan", &summary.exposureplan),
        ("acquiredimage", &summary.acquiredimage),
    ] {
        counts.insert(format!("{name}_inserted"), table.inserted as i64);
        counts.insert(format!("{name}_updated"), table.updated as i64);
    }
    if summary.imagedata_synced {
        counts.insert(
            "imagedata_inserted".into(),
            summary.imagedata.inserted as i64,
        );
    }
    counts
}

pub fn print_summary(summary: &BTreeMap<String, i64>) {
    let interesting: Vec<_> = summary
        .iter()
        .filter(|(_, count)| **count != 0)
        .map(|(name, count)| format!("{name}={count}"))
        .collect();
    if interesting.is_empty() {
        println!("Nothing to change.");
    } else {
        println!("{}", interesting.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_direction_maps_to_its_wire_operation() {
        assert_eq!(RemoteDirection::Pull.operation(), SyncOperation::Merge);
        assert_eq!(
            RemoteDirection::PushPlanning.operation(),
            SyncOperation::PushPlanning
        );
        assert_eq!(
            RemoteDirection::PushGrades.operation(),
            SyncOperation::PushGrades
        );
    }

    #[test]
    fn a_summary_hides_the_zeroes() {
        // A merge reports a couple of dozen counters. Printing every zero
        // buries the two lines that matter.
        let mut summary = BTreeMap::new();
        summary.insert("project_inserted".to_string(), 0);
        summary.insert("acquiredimage_inserted".to_string(), 3);
        let shown: Vec<_> = summary.iter().filter(|(_, count)| **count != 0).collect();
        assert_eq!(shown.len(), 1);
    }
}
