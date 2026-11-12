//! Planning settings push (our DB → telescope DB).
//!
//! Copies scheduler structure only: projects, targets, exposure templates,
//! exposure plans, and rule weights. It does not touch acquired images,
//! image data, or grades. Existing telescope plan progress (`acquired` and
//! `accepted`) stays unchanged; a new plan starts with both counts at zero.

use super::pull::{
    sync_structure, SourceDeletionContext, SourceDeletionRules, SyncDestination, TemplateScope,
};
use super::{require_planning_capable, TableCounts};
use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction, TransactionBehavior};

/// Options for a planning-only settings push.
pub struct PlanningOptions {
    /// Compute and report the changes, then roll them back.
    pub dry_run: bool,
    /// Restrict the push to projects whose names contain this text.
    pub project_filter: Option<String>,
    /// What the push writes: a Target Scheduler database, where a project
    /// the source deleted only loses its rows, or a registered PSF Guard
    /// catalog, where the deletion is also recorded as a tombstone.
    pub destination: SyncDestination,
    /// What may stop the push from deleting a project its source deleted.
    pub deletions: SourceDeletionRules,
}

/// Outcome of a planning settings push.
#[derive(Debug, Default)]
pub struct PlanningSummary {
    pub exposuretemplate: TableCounts,
    pub project: TableCounts,
    pub ruleweight: TableCounts,
    pub target: TableCounts,
    pub exposureplan: TableCounts,
    /// Per-entity trace lines for verbose CLI output.
    pub changes: Vec<String>,
    /// GUIDs of the destination projects this push deleted because the
    /// source catalog deleted them.
    pub deleted_project_guids: Vec<String>,
}

impl PlanningSummary {
    pub fn total_inserted(&self) -> usize {
        self.exposuretemplate.inserted
            + self.project.inserted
            + self.ruleweight.inserted
            + self.target.inserted
            + self.exposureplan.inserted
    }

    pub fn total_updated(&self) -> usize {
        self.exposuretemplate.updated
            + self.project.updated
            + self.ruleweight.updated
            + self.target.updated
            + self.exposureplan.updated
    }
}

/// Push planning settings into a scheduler database without copying capture
/// history or overwriting the scheduler's plan progress counters.
pub fn sync_planning(
    src: &Connection,
    dest: &Connection,
    opts: &PlanningOptions,
) -> Result<PlanningSummary> {
    require_planning_capable(src).context("source database")?;
    require_planning_capable(dest).context("destination database")?;

    // A dry run writes too, then rolls back.
    let tx = Transaction::new_unchecked(dest, TransactionBehavior::Immediate)?;
    let summary = sync_planning_in_transaction(src, &tx, opts)?;
    if opts.dry_run {
        tx.rollback()?;
    } else {
        tx.commit()?;
    }
    Ok(summary)
}

/// Stage one planning sync inside a transaction owned by the caller.
///
/// The guarded server path uses this form so it can verify destination
/// preconditions after taking SQLite's write lock, then apply in that same
/// transaction.
pub(crate) fn sync_planning_in_transaction(
    src: &Connection,
    tx: &Transaction<'_>,
    opts: &PlanningOptions,
) -> Result<PlanningSummary> {
    require_planning_capable(src).context("source database")?;
    require_planning_capable(tx).context("destination database")?;

    let mut summary = PlanningSummary::default();
    let structure = sync_structure(
        src,
        tx,
        opts.project_filter.as_deref(),
        &["acquired", "accepted"],
        false,
        TemplateScope::UsedByPlans,
        SourceDeletionContext {
            destination: opts.destination,
            rules: &opts.deletions,
        },
        &mut summary.changes,
    )?;
    summary.deleted_project_guids = structure.deleted_project_guids;

    summary.exposuretemplate = structure.exposuretemplate;
    summary.project = structure.project;
    summary.ruleweight = structure.ruleweight;
    summary.target = structure.target;
    summary.exposureplan = structure.exposureplan;

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE project (Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT, description TEXT, state INTEGER, priority INTEGER, guid TEXT);
             CREATE TABLE target (Id INTEGER PRIMARY KEY, name TEXT, active INTEGER, ra REAL, dec REAL, epochcode INTEGER, projectid INTEGER, guid TEXT);
             CREATE TABLE exposuretemplate (Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT, filtername TEXT, gain INTEGER, guid TEXT);
             CREATE TABLE exposureplan (Id INTEGER PRIMARY KEY, profileId TEXT, exposure REAL, desired INTEGER, acquired INTEGER, accepted INTEGER, enabled INTEGER, targetid INTEGER, exposureTemplateId INTEGER, guid TEXT);
             CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, projectId INTEGER, targetId INTEGER, gradingStatus INTEGER, guid TEXT);
             CREATE TABLE ruleweight (Id INTEGER PRIMARY KEY, name TEXT, weight REAL, projectid INTEGER);",
        )
        .unwrap();
    }

    fn local() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        schema(&conn);
        conn.execute_batch(
            "INSERT INTO project VALUES (1,'p','Orion','new notes',2,9,'project-guid');
             INSERT INTO target VALUES (1,'M42',1,5.5,-5.4,0,1,'target-guid');
             INSERT INTO exposuretemplate VALUES (1,'p','Ha 300','Ha',120,'template-guid');
             INSERT INTO exposureplan VALUES (1,'p',300,40,18,16,1,1,1,'plan-guid');
             INSERT INTO ruleweight VALUES (1,'Priority',2.5,1);
             INSERT INTO acquiredimage VALUES (1,1,1,2,'image-guid');",
        )
        .unwrap();
        conn
    }

    fn telescope() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        schema(&conn);
        conn.execute_batch(
            "INSERT INTO project VALUES (20,'p','Orion','old notes',1,2,'project-guid');
             INSERT INTO target VALUES (30,'Old name',1,5.4,-5.3,0,20,'target-guid');
             INSERT INTO exposuretemplate VALUES (40,'p','Old Ha','Ha',100,'template-guid');
             INSERT INTO exposureplan VALUES (50,'p',180,20,7,6,0,30,40,'plan-guid');
             INSERT INTO ruleweight VALUES (60,'Priority',1.0,20);
             INSERT INTO acquiredimage VALUES (70,20,30,1,'telescope-image');",
        )
        .unwrap();
        conn
    }

    fn opts() -> PlanningOptions {
        PlanningOptions {
            dry_run: false,
            project_filter: None,
            destination: SyncDestination::Scheduler,
            deletions: SourceDeletionRules::default(),
        }
    }

    fn one<T: rusqlite::types::FromSql>(conn: &Connection, sql: &str) -> T {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn updates_settings_but_keeps_capture_history() {
        let src = local();
        let dest = telescope();
        let summary = sync_planning(&src, &dest, &opts()).unwrap();

        assert_eq!(summary.project.updated, 1);
        assert_eq!(summary.target.updated, 1);
        assert_eq!(summary.exposuretemplate.updated, 1);
        assert_eq!(summary.exposureplan.updated, 1);
        assert_eq!(summary.ruleweight.updated, 1);
        assert_eq!(
            one::<String>(&dest, "SELECT description FROM project"),
            "new notes"
        );
        assert_eq!(one::<i64>(&dest, "SELECT desired FROM exposureplan"), 40);
        assert_eq!(one::<i64>(&dest, "SELECT acquired FROM exposureplan"), 7);
        assert_eq!(one::<i64>(&dest, "SELECT accepted FROM exposureplan"), 6);
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM acquiredimage"), 1);
        assert_eq!(
            one::<i64>(&dest, "SELECT gradingStatus FROM acquiredimage"),
            1
        );
    }

    #[test]
    fn inserts_new_tree_with_zero_plan_progress() {
        let src = local();
        let dest = Connection::open_in_memory().unwrap();
        schema(&dest);

        let summary = sync_planning(&src, &dest, &opts()).unwrap();
        assert_eq!(summary.project.inserted, 1);
        assert_eq!(summary.exposureplan.inserted, 1);
        assert_eq!(one::<i64>(&dest, "SELECT acquired FROM exposureplan"), 0);
        assert_eq!(one::<i64>(&dest, "SELECT accepted FROM exposureplan"), 0);
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM acquiredimage"), 0);
        assert_eq!(
            one::<i64>(
                &dest,
                "SELECT COUNT(*) FROM exposureplan ep JOIN target t ON ep.targetid=t.Id JOIN exposuretemplate et ON ep.exposureTemplateId=et.Id"
            ),
            1
        );
    }

    #[test]
    fn dry_run_reports_without_writing() {
        let src = local();
        let dest = Connection::open_in_memory().unwrap();
        schema(&dest);
        let summary = sync_planning(
            &src,
            &dest,
            &PlanningOptions {
                dry_run: true,
                project_filter: None,
                destination: SyncDestination::Scheduler,
                deletions: SourceDeletionRules::default(),
            },
        )
        .unwrap();
        assert_eq!(summary.project.inserted, 1);
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM project"), 0);
    }

    #[test]
    fn project_filter_limits_projects_and_children() {
        let src = local();
        src.execute_batch(
            "INSERT INTO project VALUES (2,'p','Andromeda','',1,1,'project-guid-2');
             INSERT INTO target VALUES (2,'M31',1,0.7,41.2,0,2,'target-guid-2');
             INSERT INTO exposureplan VALUES (2,'p',120,10,4,3,1,2,1,'plan-guid-2');",
        )
        .unwrap();
        let dest = Connection::open_in_memory().unwrap();
        schema(&dest);

        sync_planning(
            &src,
            &dest,
            &PlanningOptions {
                dry_run: false,
                project_filter: Some("Orion".into()),
                destination: SyncDestination::Scheduler,
                deletions: SourceDeletionRules::default(),
            },
        )
        .unwrap();
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM project"), 1);
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM target"), 1);
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM exposureplan"), 1);
    }

    fn template_rows(conn: &Connection) -> Vec<(i64, String, String)> {
        conn.prepare("SELECT Id, name, guid FROM exposuretemplate ORDER BY Id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn plan_templates(conn: &Connection) -> Vec<i64> {
        conn.prepare("SELECT exposureTemplateId FROM exposureplan ORDER BY Id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn a_planning_push_sends_only_the_templates_its_plans_use() {
        let src = local();
        // A template an import made in the catalog, used by no plan.
        src.execute(
            "INSERT INTO exposuretemplate VALUES (2,'p','Ha G139 O50 1x1','Ha',139,'import-guid')",
            [],
        )
        .unwrap();
        let dest = Connection::open_in_memory().unwrap();
        schema(&dest);
        let summary = sync_planning(&src, &dest, &opts()).unwrap();
        assert_eq!(summary.exposuretemplate.inserted, 1);
        assert_eq!(
            template_rows(&dest),
            [(1, "Ha 300".to_string(), "template-guid".to_string())]
        );

        // So does a registered database, which may be a rig's own Target
        // Scheduler database opened directly.
        let catalog = Connection::open_in_memory().unwrap();
        schema(&catalog);
        sync_planning(
            &src,
            &catalog,
            &PlanningOptions {
                destination: SyncDestination::Catalog,
                ..opts()
            },
        )
        .unwrap();
        assert_eq!(template_rows(&catalog).len(), 1);
    }

    #[test]
    fn a_template_under_another_guid_takes_the_rigs_identical_template_and_its_guid() {
        let src = local();
        let dest = Connection::open_in_memory().unwrap();
        schema(&dest);
        // The rig's own copy of the catalog's template: the same settings,
        // but a GUID of its own, as an upgrade gives each copy of a
        // database.
        dest.execute_batch(
            "INSERT INTO exposuretemplate VALUES (40,'p',' ha 300','HA',120,'rig-guid');",
        )
        .unwrap();
        for push in 0..2 {
            let summary = sync_planning(&src, &dest, &opts()).unwrap();
            assert_eq!(summary.exposuretemplate.inserted, 0);
            assert_eq!(
                summary
                    .changes
                    .iter()
                    .any(|line| line.contains("identical template")),
                push == 0,
                "only the first push matches by settings: {:?}",
                summary.changes
            );
            assert_eq!(template_rows(&dest).len(), 1);
            assert_eq!(
                template_rows(&dest)[0].2,
                "template-guid",
                "the rig's template takes the catalog's GUID"
            );
            assert_eq!(plan_templates(&dest), [40]);
        }

        // Another gain is another template.
        let dest = Connection::open_in_memory().unwrap();
        schema(&dest);
        dest.execute(
            "INSERT INTO exposuretemplate VALUES (40,'p','Ha 300','Ha',100,'rig-guid')",
            [],
        )
        .unwrap();
        let summary = sync_planning(&src, &dest, &opts()).unwrap();
        assert_eq!(summary.exposuretemplate.inserted, 1);
        assert_eq!(plan_templates(&dest), [41]);
    }

    #[test]
    fn two_source_templates_of_the_same_name_and_settings_arrive_as_one() {
        let src = local();
        src.execute_batch(
            "INSERT INTO exposuretemplate VALUES (2,'p','Ha 300','Ha',120,'second-guid');
             INSERT INTO target VALUES (2,'M43',1,5.5,-5.2,0,1,'target-guid-2');
             INSERT INTO exposureplan VALUES (2,'p',300,10,0,0,1,2,2,'plan-guid-2');",
        )
        .unwrap();
        let dest = Connection::open_in_memory().unwrap();
        schema(&dest);
        let summary = sync_planning(&src, &dest, &opts()).unwrap();
        assert_eq!(summary.exposuretemplate.inserted, 1);
        assert_eq!(template_rows(&dest).len(), 1);
        assert_eq!(plan_templates(&dest), [1, 1]);
    }

    #[test]
    fn a_push_merges_the_templates_the_rig_repeats() {
        let src = local();
        let dest = telescope();
        // A copy of the rig's template an older push added under its own
        // GUID, with a plan of its own.
        dest.execute_batch(
            "INSERT INTO exposuretemplate VALUES (41,'p','Old Ha','Ha',100,'copy-guid');
             INSERT INTO exposureplan VALUES (51,'p',120,5,0,0,1,30,41,'other-plan');",
        )
        .unwrap();
        let preview = sync_planning(
            &src,
            &dest,
            &PlanningOptions {
                dry_run: true,
                ..opts()
            },
        )
        .unwrap();
        assert_eq!(preview.exposuretemplate.deleted, 1);
        assert_eq!(template_rows(&dest).len(), 2, "a preview writes nothing");

        let summary = sync_planning(&src, &dest, &opts()).unwrap();
        assert_eq!(summary.exposuretemplate.deleted, 1);
        assert_eq!(summary.exposuretemplate.inserted, 0);
        assert_eq!(
            template_rows(&dest),
            [(40, "Ha 300".to_string(), "template-guid".to_string())]
        );
        assert_eq!(plan_templates(&dest), [40, 40]);
    }

    #[test]
    fn a_copy_merged_on_the_rig_stays_merged_when_the_catalog_still_has_it() {
        // The catalog still has both copies the rig once had, each with a
        // plan; the rig has them too.
        let src = local();
        src.execute_batch(
            "INSERT INTO exposuretemplate VALUES (2,'p','Ha 300','Ha',120,'copy-guid');
             INSERT INTO target VALUES (2,'M43',1,5.5,-5.2,0,1,'target-guid-2');
             INSERT INTO exposureplan VALUES (2,'p',300,10,0,0,1,2,2,'plan-guid-2');",
        )
        .unwrap();
        let dest = Connection::open_in_memory().unwrap();
        schema(&dest);
        dest.execute_batch(
            "INSERT INTO exposuretemplate VALUES (40,'p','Ha 300','Ha',120,'template-guid'),
                                                 (41,'p','Ha 300','Ha',120,'copy-guid');",
        )
        .unwrap();
        for push in 0..2 {
            let summary = sync_planning(&src, &dest, &opts()).unwrap();
            assert_eq!(summary.exposuretemplate.inserted, 0, "push {push}");
            assert_eq!(
                summary.exposuretemplate.deleted,
                usize::from(push == 0),
                "push {push}"
            );
            assert_eq!(template_rows(&dest).len(), 1, "push {push}");
            assert_eq!(plan_templates(&dest), [40, 40], "push {push}");
        }
    }

    fn with_default_exposure(conn: &Connection) {
        conn.execute_batch("ALTER TABLE exposuretemplate ADD COLUMN defaultexposure REAL")
            .unwrap();
    }

    fn default_exposures(conn: &Connection) -> Vec<(i64, f64)> {
        conn.prepare("SELECT Id, defaultexposure FROM exposuretemplate ORDER BY Id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn a_template_takes_the_identical_one_and_never_rewrites_another() {
        let src = local();
        with_default_exposure(&src);
        src.execute("UPDATE exposuretemplate SET defaultexposure = 180", [])
            .unwrap();
        let rig = || {
            let dest = Connection::open_in_memory().unwrap();
            schema(&dest);
            with_default_exposure(&dest);
            // Two templates of the same name and camera settings that
            // differ in their default exposure: neither repeats the other.
            dest.execute_batch(
                "INSERT INTO exposuretemplate VALUES (40,'p','Ha 300','Ha',120,'rig-a',300),
                                                     (41,'p','Ha 300','Ha',120,'rig-b',180);",
            )
            .unwrap();
            dest
        };
        let dest = rig();
        let summary = sync_planning(&src, &dest, &opts()).unwrap();
        assert_eq!(summary.exposuretemplate.inserted, 0);
        assert_eq!(plan_templates(&dest), [41], "the identical one");
        assert_eq!(default_exposures(&dest), [(40, 300.0), (41, 180.0)]);

        // Identical to neither: which one is meant cannot be told, so
        // neither is rewritten.
        src.execute("UPDATE exposuretemplate SET defaultexposure = 600", [])
            .unwrap();
        let dest = rig();
        let summary = sync_planning(&src, &dest, &opts()).unwrap();
        assert_eq!(summary.exposuretemplate.inserted, 1);
        assert_eq!(
            default_exposures(&dest),
            [(40, 300.0), (41, 180.0), (42, 600.0)]
        );
    }

    #[test]
    fn an_edited_template_never_rewrites_the_rigs_older_copy() {
        let src = local();
        with_default_exposure(&src);
        // The catalog's edited template, and an older copy no plan uses any
        // more, which carries the rig's GUID.
        src.execute_batch(
            "UPDATE exposuretemplate SET defaultexposure = 180;
             INSERT INTO exposuretemplate VALUES (2,'p','Ha 300','Ha',120,'rig-guid',300);",
        )
        .unwrap();
        let dest = Connection::open_in_memory().unwrap();
        schema(&dest);
        with_default_exposure(&dest);
        dest.execute(
            "INSERT INTO exposuretemplate VALUES (40,'p','Ha 300','Ha',120,'rig-guid',300)",
            [],
        )
        .unwrap();
        let summary = sync_planning(&src, &dest, &opts()).unwrap();
        assert_eq!(
            summary.exposuretemplate.inserted, 1,
            "{:?}",
            summary.changes
        );
        assert_eq!(default_exposures(&dest), [(40, 300.0), (41, 180.0)]);
        assert_eq!(plan_templates(&dest), [41]);
    }
}
