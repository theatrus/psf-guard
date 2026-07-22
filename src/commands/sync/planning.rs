//! Planning settings push (our DB → telescope DB).
//!
//! Copies scheduler structure only: projects, targets, exposure templates,
//! exposure plans, and rule weights. It does not touch acquired images,
//! image data, or grades. Existing telescope plan progress (`acquired` and
//! `accepted`) stays unchanged; a new plan starts with both counts at zero.

use super::pull::sync_structure;
use super::{require_planning_capable, TableCounts};
use anyhow::{Context, Result};
use rusqlite::Connection;

/// Options for a planning-only settings push.
pub struct PlanningOptions {
    /// Compute and report the changes, then roll them back.
    pub dry_run: bool,
    /// Restrict the push to projects whose names contain this text.
    pub project_filter: Option<String>,
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

    let tx = dest.unchecked_transaction()?;
    let mut summary = PlanningSummary::default();
    let structure = sync_structure(
        src,
        &tx,
        opts.project_filter.as_deref(),
        true,
        &mut summary.changes,
    )?;

    summary.exposuretemplate = structure.exposuretemplate;
    summary.project = structure.project;
    summary.ruleweight = structure.ruleweight;
    summary.target = structure.target;
    summary.exposureplan = structure.exposureplan;

    if opts.dry_run {
        tx.rollback()?;
    } else {
        tx.commit()?;
    }
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
            },
        )
        .unwrap();
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM project"), 1);
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM target"), 1);
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM exposureplan"), 1);
    }
}
