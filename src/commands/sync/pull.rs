//! Entity pull (telescope → our DB).
//!
//! Mirrors the telescope's scheduler structure and captured images into our
//! local DB: projects, targets (coordinates), exposure templates/plans, rule
//! weights, acquired images, and optionally the imagedata thumbnail BLOBs.
//! Entities are matched by their stable `guid` (TS schema v22+); foreign keys
//! are remapped onto the destination's local autoincrement Ids. The telescope
//! wins for structure fields (upsert), but **local grading is preserved**: an
//! existing image keeps its `gradingStatus`/`rejectreason` unless ours is still
//! Pending, in which case it adopts the telescope's grade.
//!
//! Pure DB logic; the whole pull runs in one destination transaction (rolled
//! back on `--dry-run`). CLI glue lives in `cli_main.rs`.

use super::require_pull_capable;
use anyhow::{anyhow, Context, Result};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, ToSql, Transaction};
use std::collections::{HashMap, HashSet};

/// Knobs for a pull.
pub struct PullOptions {
    /// Compute & report the plan but roll back all writes.
    pub dry_run: bool,
    /// Also copy the (large) `imagedata` thumbnail BLOBs.
    pub with_image_data: bool,
    /// Restrict the pull to projects whose name matches (substring); cascades to
    /// their targets, plans, and images.
    pub project_filter: Option<String>,
}

/// Insert/update/unchanged counters for one table.
#[derive(Debug, Default, Clone)]
pub struct TableCounts {
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
}

/// Outcome of a pull.
#[derive(Debug, Default)]
pub struct PullSummary {
    pub exposuretemplate: TableCounts,
    pub project: TableCounts,
    pub ruleweight: TableCounts,
    pub target: TableCounts,
    pub exposureplan: TableCounts,
    pub acquiredimage: TableCounts,
    /// Existing images whose Pending grade adopted the telescope's grade.
    pub grade_filled: usize,
    /// Existing images whose local (non-Pending) grade was preserved.
    pub grade_preserved: usize,
    pub imagedata: TableCounts,
    /// True when `--with-image-data` ran (else imagedata was left untouched).
    pub imagedata_synced: bool,
    /// Per-entity trace lines (for `--verbose`).
    pub changes: Vec<String>,
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(n) => Some(*n),
        Value::Real(r) => Some(*r as i64),
        _ => None,
    }
}

/// SQLite-aware value equality (treats Integer/Real numerically) so idempotent
/// re-pulls report `unchanged` rather than spurious updates.
fn value_eq(a: &Value, b: &Value) -> bool {
    use Value::*;
    match (a, b) {
        (Null, Null) => true,
        (Integer(x), Integer(y)) => x == y,
        (Real(x), Real(y)) => x == y,
        (Integer(x), Real(y)) | (Real(y), Integer(x)) => (*x as f64) == *y,
        (Text(x), Text(y)) => x == y,
        (Blob(x), Blob(y)) => x == y,
        _ => false,
    }
}

fn values_equal(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| value_eq(x, y))
}

/// Column names of `table` in declared order.
fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info('{}')", table))?;
    let cols = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(cols)
}

fn quoted(cols: &[&str]) -> String {
    cols.iter()
        .map(|c| format!("\"{}\"", c))
        .collect::<Vec<_>>()
        .join(",")
}

struct GuidUpsert {
    /// Source local Id → destination local Id (for FK remapping of children).
    id_map: HashMap<i64, i64>,
    counts: TableCounts,
}

/// Upsert every (optionally filtered) source row of a guid-keyed `table` into
/// the destination, matching by `guid` and remapping the named FK columns
/// through the provided source-Id→dest-Id maps. Telescope wins on conflict.
fn upsert_guid_table(
    src: &Connection,
    tx: &Transaction,
    table: &str,
    fk_remaps: &[(&str, &HashMap<i64, i64>)],
    allowed_src_ids: Option<&HashSet<i64>>,
    changes: &mut Vec<String>,
) -> Result<GuidUpsert> {
    let cols = table_columns(src, table)?;
    let id_pos = cols
        .iter()
        .position(|c| c.eq_ignore_ascii_case("Id"))
        .ok_or_else(|| anyhow!("table {} has no Id column", table))?;
    let guid_pos = cols
        .iter()
        .position(|c| c.eq_ignore_ascii_case("guid"))
        .ok_or_else(|| anyhow!("table {} has no guid column", table))?;

    let write_idx: Vec<usize> = (0..cols.len()).filter(|&i| i != id_pos).collect();
    let write_cols: Vec<&str> = write_idx.iter().map(|&i| cols[i].as_str()).collect();
    let guid_w = write_cols
        .iter()
        .position(|c| c.eq_ignore_ascii_case("guid"))
        .unwrap();
    let fk_positions: Vec<(usize, &HashMap<i64, i64>)> = fk_remaps
        .iter()
        .filter_map(|(name, map)| {
            write_cols
                .iter()
                .position(|c| c.eq_ignore_ascii_case(name))
                .map(|p| (p, *map))
        })
        .collect();

    // Destination guid -> (dest_id, write-col values), the pre-pull state.
    let mut dest_map: HashMap<String, (i64, Vec<Value>)> = HashMap::new();
    {
        let sql = format!("SELECT Id,{} FROM {}", quoted(&write_cols), table);
        let mut stmt = tx.prepare(&sql)?;
        let n = write_cols.len();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let dest_id: i64 = row.get(0)?;
            let vals: Vec<Value> = (0..n)
                .map(|i| row.get::<_, Value>(i + 1))
                .collect::<rusqlite::Result<_>>()?;
            if let Value::Text(g) = &vals[guid_w] {
                if !g.is_empty() {
                    dest_map.insert(g.clone(), (dest_id, vals));
                }
            }
        }
    }

    let insert_sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        table,
        quoted(&write_cols),
        write_cols.iter().map(|_| "?").collect::<Vec<_>>().join(",")
    );
    let update_sql = format!(
        "UPDATE {} SET {} WHERE Id=?",
        table,
        write_cols
            .iter()
            .map(|c| format!("\"{}\"=?", c))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut ins_stmt = tx.prepare(&insert_sql)?;
    let mut upd_stmt = tx.prepare(&update_sql)?;

    let mut counts = TableCounts::default();
    let mut id_map: HashMap<i64, i64> = HashMap::new();

    let src_sql = format!("SELECT {} FROM {}", quoted(&cols_str(&cols)), table);
    let mut src_stmt = src.prepare(&src_sql)?;
    let ncols = cols.len();
    let mut rows = src_stmt.query([])?;
    while let Some(row) = rows.next()? {
        let all_vals: Vec<Value> = (0..ncols)
            .map(|i| row.get::<_, Value>(i))
            .collect::<rusqlite::Result<_>>()?;
        let src_id = match as_i64(&all_vals[id_pos]) {
            Some(n) => n,
            None => continue,
        };
        if let Some(allowed) = allowed_src_ids {
            if !allowed.contains(&src_id) {
                continue;
            }
        }
        let guid = match &all_vals[guid_pos] {
            Value::Text(g) if !g.is_empty() => g.clone(),
            _ => continue,
        };

        let mut write_values: Vec<Value> = write_idx.iter().map(|&i| all_vals[i].clone()).collect();
        for (pos, map) in &fk_positions {
            if let Value::Integer(src_fk) = write_values[*pos] {
                if let Some(dest_fk) = map.get(&src_fk) {
                    write_values[*pos] = Value::Integer(*dest_fk);
                }
            }
        }

        match dest_map.get(&guid) {
            Some((dest_id, cur_vals)) => {
                id_map.insert(src_id, *dest_id);
                if values_equal(&write_values, cur_vals) {
                    counts.unchanged += 1;
                } else {
                    let mut p: Vec<&dyn ToSql> =
                        write_values.iter().map(|v| v as &dyn ToSql).collect();
                    let did = *dest_id;
                    p.push(&did);
                    upd_stmt.execute(p.as_slice())?;
                    counts.updated += 1;
                    changes.push(format!("update {} {}", table, guid));
                }
            }
            None => {
                ins_stmt.execute(params_from_iter(write_values.iter()))?;
                let dest_id = tx.last_insert_rowid();
                id_map.insert(src_id, dest_id);
                counts.inserted += 1;
                changes.push(format!("insert {} {}", table, guid));
            }
        }
    }

    Ok(GuidUpsert { id_map, counts })
}

fn cols_str(cols: &[String]) -> Vec<&str> {
    cols.iter().map(|s| s.as_str()).collect()
}

/// Upsert acquired-image rows with the grade rule: new rows take the telescope
/// grade; existing rows keep their local grade unless it's Pending (0), in
/// which case they adopt the telescope's grade. FK columns
/// (`projectId`/`targetId`/`exposureId`) are remapped.
fn upsert_acquired_images(
    src: &Connection,
    tx: &Transaction,
    proj_map: &HashMap<i64, i64>,
    tgt_map: &HashMap<i64, i64>,
    plan_map: &HashMap<i64, i64>,
    allowed_project_ids: Option<&HashSet<i64>>,
    summary: &mut PullSummary,
) -> Result<HashMap<i64, i64>> {
    let table = "acquiredimage";
    let cols = table_columns(src, table)?;
    let ci = |n: &str| cols.iter().position(|c| c.eq_ignore_ascii_case(n));
    let id_pos = ci("Id").ok_or_else(|| anyhow!("acquiredimage.Id missing"))?;
    let guid_pos = ci("guid").ok_or_else(|| anyhow!("acquiredimage.guid missing"))?;
    let proj_pos = ci("projectId").ok_or_else(|| anyhow!("acquiredimage.projectId missing"))?;

    let write_idx: Vec<usize> = (0..cols.len()).filter(|&i| i != id_pos).collect();
    let write_cols: Vec<&str> = write_idx.iter().map(|&i| cols[i].as_str()).collect();
    let wpos = |n: &str| write_cols.iter().position(|c| c.eq_ignore_ascii_case(n));
    let guid_w = wpos("guid").unwrap();
    let grade_w = wpos("gradingStatus").ok_or_else(|| anyhow!("gradingStatus missing"))?;
    let reason_w = wpos("rejectreason");
    let proj_w = wpos("projectId");
    let tgt_w = wpos("targetId");
    let expo_w = wpos("exposureId");

    // Destination guid -> (dest_id, write values).
    let mut dest_map: HashMap<String, (i64, Vec<Value>)> = HashMap::new();
    {
        let sql = format!("SELECT Id,{} FROM {}", quoted(&write_cols), table);
        let mut stmt = tx.prepare(&sql)?;
        let n = write_cols.len();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let dest_id: i64 = row.get(0)?;
            let vals: Vec<Value> = (0..n)
                .map(|i| row.get::<_, Value>(i + 1))
                .collect::<rusqlite::Result<_>>()?;
            if let Value::Text(g) = &vals[guid_w] {
                if !g.is_empty() {
                    dest_map.insert(g.clone(), (dest_id, vals));
                }
            }
        }
    }

    let insert_sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        table,
        quoted(&write_cols),
        write_cols.iter().map(|_| "?").collect::<Vec<_>>().join(",")
    );
    let update_sql = format!(
        "UPDATE {} SET {} WHERE Id=?",
        table,
        write_cols
            .iter()
            .map(|c| format!("\"{}\"=?", c))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut ins_stmt = tx.prepare(&insert_sql)?;
    let mut upd_stmt = tx.prepare(&update_sql)?;

    let mut id_map: HashMap<i64, i64> = HashMap::new();
    let src_sql = format!("SELECT {} FROM {}", quoted(&cols_str(&cols)), table);
    let mut src_stmt = src.prepare(&src_sql)?;
    let ncols = cols.len();
    let mut rows = src_stmt.query([])?;
    while let Some(row) = rows.next()? {
        let all_vals: Vec<Value> = (0..ncols)
            .map(|i| row.get::<_, Value>(i))
            .collect::<rusqlite::Result<_>>()?;
        let src_id = match as_i64(&all_vals[id_pos]) {
            Some(n) => n,
            None => continue,
        };
        let src_proj = as_i64(&all_vals[proj_pos]);
        if let Some(allowed) = allowed_project_ids {
            match src_proj {
                Some(p) if allowed.contains(&p) => {}
                _ => continue,
            }
        }
        let guid = match &all_vals[guid_pos] {
            Value::Text(g) if !g.is_empty() => g.clone(),
            _ => continue,
        };

        let mut write_values: Vec<Value> = write_idx.iter().map(|&i| all_vals[i].clone()).collect();
        remap_fk(&mut write_values, proj_w, proj_map);
        remap_fk(&mut write_values, tgt_w, tgt_map);
        // exposureId references exposureplan.Id; if unmatched, write 0.
        if let Some(p) = expo_w {
            if let Value::Integer(src_plan) = write_values[p] {
                write_values[p] = Value::Integer(plan_map.get(&src_plan).copied().unwrap_or(0));
            }
        }

        match dest_map.get(&guid) {
            Some((dest_id, cur_vals)) => {
                id_map.insert(src_id, *dest_id);
                let dest_grade = as_i64(&cur_vals[grade_w]).unwrap_or(0);
                if dest_grade != 0 {
                    // Preserve local decision.
                    write_values[grade_w] = cur_vals[grade_w].clone();
                    if let Some(rp) = reason_w {
                        write_values[rp] = cur_vals[rp].clone();
                    }
                    summary.grade_preserved += 1;
                } else if as_i64(&write_values[grade_w]).unwrap_or(0) != 0 {
                    // Local Pending adopts the telescope's (non-Pending) grade.
                    summary.grade_filled += 1;
                }

                if values_equal(&write_values, cur_vals) {
                    summary.acquiredimage.unchanged += 1;
                } else {
                    let mut p: Vec<&dyn ToSql> =
                        write_values.iter().map(|v| v as &dyn ToSql).collect();
                    let did = *dest_id;
                    p.push(&did);
                    upd_stmt.execute(p.as_slice())?;
                    summary.acquiredimage.updated += 1;
                    summary
                        .changes
                        .push(format!("update acquiredimage {}", guid));
                }
            }
            None => {
                ins_stmt.execute(params_from_iter(write_values.iter()))?;
                let dest_id = tx.last_insert_rowid();
                id_map.insert(src_id, dest_id);
                summary.acquiredimage.inserted += 1;
                summary
                    .changes
                    .push(format!("insert acquiredimage {}", guid));
            }
        }
    }

    Ok(id_map)
}

fn remap_fk(values: &mut [Value], pos: Option<usize>, map: &HashMap<i64, i64>) {
    if let Some(p) = pos {
        if let Value::Integer(src_fk) = values[p] {
            if let Some(dest_fk) = map.get(&src_fk) {
                values[p] = Value::Integer(*dest_fk);
            }
        }
    }
}

/// Upsert per-project rule weights (no guid; matched by `(projectId, name)`).
fn upsert_ruleweights(
    src: &Connection,
    tx: &Transaction,
    proj_map: &HashMap<i64, i64>,
    counts: &mut TableCounts,
    changes: &mut Vec<String>,
) -> Result<()> {
    let mut src_stmt = src.prepare("SELECT name, weight FROM ruleweight WHERE projectid = ?1")?;
    for (src_proj, dest_proj) in proj_map {
        let mut dest_existing: HashMap<String, (i64, f64)> = HashMap::new();
        {
            let mut ds =
                tx.prepare("SELECT Id, name, weight FROM ruleweight WHERE projectid = ?1")?;
            let mut rows = ds.query(params![dest_proj])?;
            while let Some(r) = rows.next()? {
                dest_existing.insert(r.get::<_, String>(1)?, (r.get(0)?, r.get(2)?));
            }
        }
        let mut rows = src_stmt.query(params![src_proj])?;
        while let Some(r) = rows.next()? {
            let name: String = r.get(0)?;
            let weight: f64 = r.get(1)?;
            match dest_existing.get(&name) {
                Some((id, w)) => {
                    if (*w - weight).abs() < f64::EPSILON {
                        counts.unchanged += 1;
                    } else {
                        tx.execute(
                            "UPDATE ruleweight SET weight=?1 WHERE Id=?2",
                            params![weight, id],
                        )?;
                        counts.updated += 1;
                        changes.push(format!("update ruleweight {}", name));
                    }
                }
                None => {
                    tx.execute(
                        "INSERT INTO ruleweight (name, weight, projectid) VALUES (?1,?2,?3)",
                        params![name, weight, dest_proj],
                    )?;
                    counts.inserted += 1;
                    changes.push(format!("insert ruleweight {}", name));
                }
            }
        }
    }
    Ok(())
}

/// Copy imagedata BLOBs for pulled images (insert-only; blobs are immutable per
/// image). In dry-run mode the rows are counted but not written.
fn copy_imagedata(
    src: &Connection,
    tx: &Transaction,
    img_map: &HashMap<i64, i64>,
    counts: &mut TableCounts,
    dry_run: bool,
) -> Result<()> {
    let mut exists_stmt =
        tx.prepare("SELECT 1 FROM imagedata WHERE acquiredimageid = ?1 LIMIT 1")?;
    let mut count_stmt =
        src.prepare("SELECT COUNT(*) FROM imagedata WHERE acquiredimageid = ?1")?;
    let mut sel_stmt = src.prepare(
        "SELECT tag, imagedata, width, height FROM imagedata WHERE acquiredimageid = ?1",
    )?;
    let mut ins_stmt = tx.prepare(
        "INSERT INTO imagedata (tag, imagedata, acquiredimageid, width, height) VALUES (?1,?2,?3,?4,?5)",
    )?;
    for (src_img, dest_img) in img_map {
        let already = exists_stmt
            .query_row(params![dest_img], |_| Ok(()))
            .optional()?
            .is_some();
        if already {
            counts.unchanged += 1;
            continue;
        }
        if dry_run {
            let n: i64 = count_stmt.query_row(params![src_img], |r| r.get(0))?;
            counts.inserted += n as usize;
            continue;
        }
        #[allow(clippy::type_complexity)]
        let blobs: Vec<(Option<String>, Option<Vec<u8>>, i64, i64)> = sel_stmt
            .query_map(params![src_img], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, i64>(2).unwrap_or(0),
                    r.get::<_, i64>(3).unwrap_or(0),
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (tag, blob, w, h) in blobs {
            ins_stmt.execute(params![tag, blob, dest_img, w, h])?;
            counts.inserted += 1;
        }
    }
    Ok(())
}

/// Compute the set of source row Ids in `table` whose `fk_col` is in `parents`.
fn child_ids(
    src: &Connection,
    table: &str,
    fk_col: &str,
    parents: &HashSet<i64>,
) -> Result<HashSet<i64>> {
    let mut set = HashSet::new();
    let mut stmt = src.prepare(&format!("SELECT Id, \"{}\" FROM {}", fk_col, table))?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        let fk: Option<i64> = r.get(1)?;
        if let Some(fk) = fk {
            if parents.contains(&fk) {
                set.insert(id);
            }
        }
    }
    Ok(set)
}

/// Pull telescope structure + captures into the destination DB.
pub fn sync_pull(src: &Connection, dest: &Connection, opts: &PullOptions) -> Result<PullSummary> {
    require_pull_capable(src).context("source database")?;
    require_pull_capable(dest).context("destination database")?;

    let tx = dest.unchecked_transaction()?;
    let mut summary = PullSummary::default();

    // Resolve the project filter into cascading source-Id sets.
    let included_projects: Option<HashSet<i64>> = match &opts.project_filter {
        Some(sub) => {
            let mut stmt = src.prepare("SELECT Id FROM project WHERE name LIKE ?1")?;
            let ids: HashSet<i64> = stmt
                .query_map(params![format!("%{}%", sub)], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<_>>()?;
            Some(ids)
        }
        None => None,
    };
    let included_targets: Option<HashSet<i64>> = match &included_projects {
        Some(projs) => Some(child_ids(src, "target", "projectid", projs)?),
        None => None,
    };
    let included_plans: Option<HashSet<i64>> = match &included_targets {
        Some(tgts) => Some(child_ids(src, "exposureplan", "targetid", tgts)?),
        None => None,
    };

    // 1. exposuretemplate — profile-scoped, no FK; synced fully so plan FKs resolve.
    let tmpl = upsert_guid_table(
        src,
        &tx,
        "exposuretemplate",
        &[],
        None,
        &mut summary.changes,
    )?;
    summary.exposuretemplate = tmpl.counts;
    let tmpl_map = tmpl.id_map;

    // 2. project
    let proj = upsert_guid_table(
        src,
        &tx,
        "project",
        &[],
        included_projects.as_ref(),
        &mut summary.changes,
    )?;
    summary.project = proj.counts;
    let proj_map = proj.id_map;

    // 3. ruleweight (children of pulled projects)
    upsert_ruleweights(
        src,
        &tx,
        &proj_map,
        &mut summary.ruleweight,
        &mut summary.changes,
    )?;

    // 4. target (remap projectid)
    let tgt = upsert_guid_table(
        src,
        &tx,
        "target",
        &[("projectid", &proj_map)],
        included_targets.as_ref(),
        &mut summary.changes,
    )?;
    summary.target = tgt.counts;
    let tgt_map = tgt.id_map;

    // 5. exposureplan (remap targetid + exposureTemplateId)
    let plan = upsert_guid_table(
        src,
        &tx,
        "exposureplan",
        &[("targetid", &tgt_map), ("exposureTemplateId", &tmpl_map)],
        included_plans.as_ref(),
        &mut summary.changes,
    )?;
    summary.exposureplan = plan.counts;
    let plan_map = plan.id_map;

    // 6. acquiredimage (grade fill-if-pending; remap proj/tgt/exposureId)
    let img_map = upsert_acquired_images(
        src,
        &tx,
        &proj_map,
        &tgt_map,
        &plan_map,
        included_projects.as_ref(),
        &mut summary,
    )?;

    // 7. imagedata (optional, heavy)
    if opts.with_image_data {
        copy_imagedata(src, &tx, &img_map, &mut summary.imagedata, opts.dry_run)?;
        summary.imagedata_synced = true;
    }

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

    /// Build a representative v22 scheduler schema (optionally without guids).
    fn schema(conn: &Connection, with_guid: bool) {
        let g = if with_guid { ", guid TEXT" } else { "" };
        conn.execute_batch(&format!(
            "CREATE TABLE project (Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT, description TEXT, state INTEGER, priority INTEGER{g});
             CREATE TABLE target (Id INTEGER PRIMARY KEY, name TEXT, active INTEGER, ra REAL, dec REAL, epochcode INTEGER, projectid INTEGER{g});
             CREATE TABLE exposuretemplate (Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT, filtername TEXT, gain INTEGER{g});
             CREATE TABLE exposureplan (Id INTEGER PRIMARY KEY, profileId TEXT, exposure REAL, desired INTEGER, acquired INTEGER, accepted INTEGER, targetid INTEGER, exposureTemplateId INTEGER{g});
             CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, projectId INTEGER, targetId INTEGER, acquireddate INTEGER, filtername TEXT, gradingStatus INTEGER NOT NULL, metadata TEXT NOT NULL, rejectreason TEXT, profileId TEXT, exposureId INTEGER{g});
             CREATE TABLE ruleweight (Id INTEGER PRIMARY KEY, name TEXT, weight REAL, projectid INTEGER);
             CREATE TABLE imagedata (Id INTEGER PRIMARY KEY, tag TEXT, imagedata BLOB, acquiredimageid INTEGER, width INTEGER DEFAULT 0, height INTEGER DEFAULT 0);"
        ))
        .unwrap();
    }

    fn opts() -> PullOptions {
        PullOptions {
            dry_run: false,
            with_image_data: false,
            project_filter: None,
        }
    }

    /// Seed one project (guid pg) with one target (tg), one template (eg), one
    /// plan (lg, exposureplan.Id=1), and images. Returns the connection.
    fn telescope() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        schema(&c, true);
        c.execute_batch(
            "INSERT INTO project (Id,profileId,name,description,state,priority,guid) VALUES (1,'p','Caldwell','',1,5,'pg');
             INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES (1,'C33',1,10.6,41.2,0,1,'tg');
             INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,guid) VALUES (1,'p','Ha','Ha',100,'eg');
             INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,guid) VALUES (1,'p',300,50,10,8,1,1,'lg');
             INSERT INTO ruleweight (Id,name,weight,projectid) VALUES (1,'Priority',1.0,1);
             INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,exposureId,guid)
               VALUES (1,1,1,1000,'Ha',1,'{\"FileName\":\"a.fits\"}',NULL,'p',1,'img1');
             INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,exposureId,guid)
               VALUES (2,1,1,2000,'Ha',2,'{\"FileName\":\"b.fits\"}','clouds','p',1,'img2');
             INSERT INTO imagedata (Id,tag,imagedata,acquiredimageid,width,height) VALUES (1,'',X'01020304',1,100,100);
             INSERT INTO imagedata (Id,tag,imagedata,acquiredimageid,width,height) VALUES (2,'',X'05060708',2,100,100);",
        )
        .unwrap();
        c
    }

    fn empty_local() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        schema(&c, true);
        c
    }

    fn one<T: rusqlite::types::FromSql>(c: &Connection, sql: &str) -> T {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn inserts_full_tree_into_empty_local() {
        let src = telescope();
        let dest = empty_local();
        let s = sync_pull(&src, &dest, &opts()).unwrap();

        assert_eq!(s.project.inserted, 1);
        assert_eq!(s.target.inserted, 1);
        assert_eq!(s.exposuretemplate.inserted, 1);
        assert_eq!(s.exposureplan.inserted, 1);
        assert_eq!(s.acquiredimage.inserted, 2);
        assert_eq!(s.ruleweight.inserted, 1);

        // Referential integrity: target.projectid points at the new project Id,
        // and acquiredimage FKs resolve.
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM acquiredimage ai JOIN project p ON ai.projectId=p.Id JOIN target t ON ai.targetId=t.Id"), 2);
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM exposureplan ep JOIN target t ON ep.targetid=t.Id JOIN exposuretemplate et ON ep.exposureTemplateId=et.Id"), 1);
        // New image takes the telescope grade.
        assert_eq!(
            one::<i64>(
                &dest,
                "SELECT gradingStatus FROM acquiredimage WHERE guid='img1'"
            ),
            1
        );
    }

    #[test]
    fn remaps_fks_when_local_ids_differ() {
        let src = telescope();
        let dest = empty_local();
        // Pre-seed dest with unrelated rows so autoincrement assigns DIFFERENT
        // Ids to the pulled project/target than their source Ids.
        dest.execute_batch(
            "INSERT INTO project (Id,profileId,name,description,state,priority,guid) VALUES (50,'p','Other','',1,1,'other-pg');
             INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES (70,'OtherT',1,0,0,0,50,'other-tg');",
        ).unwrap();

        sync_pull(&src, &dest, &opts()).unwrap();
        // Pulled project got a fresh Id (not 1); its target points at it.
        let proj_id: i64 = one(&dest, "SELECT Id FROM project WHERE guid='pg'");
        assert_ne!(proj_id, 1);
        let tgt_proj: i64 = one(&dest, "SELECT projectid FROM target WHERE guid='tg'");
        assert_eq!(tgt_proj, proj_id);
        // acquiredimage.exposureId remapped to the dest plan Id.
        let plan_id: i64 = one(&dest, "SELECT Id FROM exposureplan WHERE guid='lg'");
        assert_eq!(
            one::<i64>(
                &dest,
                "SELECT exposureId FROM acquiredimage WHERE guid='img1'"
            ),
            plan_id
        );
    }

    #[test]
    fn preserves_local_grade_but_fills_pending() {
        let src = telescope();
        let dest = empty_local();
        // dest already has img1 (locally Accepted=1 -> but set to Rejected=2 to
        // prove preservation) and img2 still Pending(0).
        dest.execute_batch(
            "INSERT INTO project (Id,profileId,name,description,state,priority,guid) VALUES (1,'p','Caldwell','',1,5,'pg');
             INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES (1,'C33',1,10.6,41.2,0,1,'tg');
             INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,exposureId,guid)
               VALUES (1,1,1,1000,'Ha',2,'{}','local-reject','p',0,'img1');
             INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,exposureId,guid)
               VALUES (2,1,1,2000,'Ha',0,'{}',NULL,'p',0,'img2');",
        ).unwrap();

        let s = sync_pull(&src, &dest, &opts()).unwrap();
        // img1 local Rejected preserved despite telescope Accepted(1).
        assert_eq!(
            one::<i64>(
                &dest,
                "SELECT gradingStatus FROM acquiredimage WHERE guid='img1'"
            ),
            2
        );
        assert_eq!(
            one::<String>(
                &dest,
                "SELECT rejectreason FROM acquiredimage WHERE guid='img1'"
            ),
            "local-reject"
        );
        // img2 was Pending -> adopts telescope Rejected(2) + reason.
        assert_eq!(
            one::<i64>(
                &dest,
                "SELECT gradingStatus FROM acquiredimage WHERE guid='img2'"
            ),
            2
        );
        assert_eq!(
            one::<String>(
                &dest,
                "SELECT rejectreason FROM acquiredimage WHERE guid='img2'"
            ),
            "clouds"
        );
        assert_eq!(s.grade_preserved, 1);
        assert_eq!(s.grade_filled, 1);
    }

    #[test]
    fn is_idempotent() {
        let src = telescope();
        let dest = empty_local();
        sync_pull(&src, &dest, &opts()).unwrap();
        let s = sync_pull(&src, &dest, &opts()).unwrap();
        // Second run changes nothing.
        assert_eq!(s.project.inserted + s.project.updated, 0);
        assert_eq!(s.target.inserted + s.target.updated, 0);
        assert_eq!(s.exposureplan.inserted + s.exposureplan.updated, 0);
        assert_eq!(s.acquiredimage.inserted + s.acquiredimage.updated, 0);
        assert_eq!(s.acquiredimage.unchanged, 2);
    }

    #[test]
    fn updates_changed_structure_fields() {
        let src = telescope();
        let dest = empty_local();
        sync_pull(&src, &dest, &opts()).unwrap();
        // Telescope changes project state and plan acquired count.
        src.execute("UPDATE project SET state=3, priority=9 WHERE guid='pg'", [])
            .unwrap();
        src.execute("UPDATE exposureplan SET acquired=42 WHERE guid='lg'", [])
            .unwrap();
        let s = sync_pull(&src, &dest, &opts()).unwrap();
        assert_eq!(s.project.updated, 1);
        assert_eq!(s.exposureplan.updated, 1);
        assert_eq!(
            one::<i64>(&dest, "SELECT state FROM project WHERE guid='pg'"),
            3
        );
        assert_eq!(
            one::<i64>(&dest, "SELECT acquired FROM exposureplan WHERE guid='lg'"),
            42
        );
    }

    #[test]
    fn dry_run_writes_nothing() {
        let src = telescope();
        let dest = empty_local();
        let mut o = opts();
        o.dry_run = true;
        let s = sync_pull(&src, &dest, &o).unwrap();
        assert_eq!(s.acquiredimage.inserted, 2); // planned
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM acquiredimage"), 0); // but nothing written
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM project"), 0);
    }

    #[test]
    fn imagedata_gated_on_flag() {
        let src = telescope();
        let dest = empty_local();
        // Default: no blobs.
        sync_pull(&src, &dest, &opts()).unwrap();
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM imagedata"), 0);

        // With flag: blobs copied for the pulled images.
        let mut o = opts();
        o.with_image_data = true;
        let s = sync_pull(&src, &dest, &o).unwrap();
        assert!(s.imagedata_synced);
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM imagedata"), 2);
        // Blob bound to the dest image Id.
        let img1: i64 = one(&dest, "SELECT Id FROM acquiredimage WHERE guid='img1'");
        assert_eq!(
            one::<i64>(
                &dest,
                &format!(
                    "SELECT COUNT(*) FROM imagedata WHERE acquiredimageid={}",
                    img1
                )
            ),
            1
        );
    }

    #[test]
    fn project_filter_scopes_pull() {
        let src = telescope();
        // Add a second project + target + image that must NOT be pulled.
        src.execute_batch(
            "INSERT INTO project (Id,profileId,name,description,state,priority,guid) VALUES (2,'p','Messier','',1,5,'pg2');
             INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES (2,'M31',1,0,0,0,2,'tg2');
             INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,exposureId,guid)
               VALUES (3,2,2,3000,'L',1,'{}',NULL,'p',0,'img3');",
        ).unwrap();
        let dest = empty_local();
        let mut o = opts();
        o.project_filter = Some("Caldwell".to_string());
        sync_pull(&src, &dest, &o).unwrap();
        assert_eq!(one::<i64>(&dest, "SELECT COUNT(*) FROM project"), 1);
        assert_eq!(
            one::<i64>(
                &dest,
                "SELECT COUNT(*) FROM acquiredimage WHERE guid='img3'"
            ),
            0
        );
        assert_eq!(
            one::<i64>(&dest, "SELECT COUNT(*) FROM target WHERE guid='tg2'"),
            0
        );
    }

    #[test]
    fn refuses_pre_v22_database() {
        let src = telescope();
        let dest = Connection::open_in_memory().unwrap();
        schema(&dest, false); // no guid columns
        let err = sync_pull(&src, &dest, &opts()).unwrap_err();
        assert!(format!("{:#}", err).contains("destination"));
    }
}
