use crate::models::{AcquiredImage, GradingStatus, Project, Target};
use anyhow::{Context, Result};
use rusqlite::{params, Connection};

/// Database access layer for PSF Guard
pub struct Database<'a> {
    conn: &'a Connection,
}

impl<'a> Database<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Database { conn }
    }

    // Project queries
    pub fn get_all_projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare(
            "SELECT Id, profileId, name, description 
             FROM project 
             ORDER BY name",
        )?;

        let projects = stmt
            .query_map([], |row| {
                Ok(Project {
                    id: row.get(0)?,
                    profile_id: row.get(1)?,
                    name: row.get(2)?,
                    description: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(projects)
    }

    pub fn get_projects_with_images(&self) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT p.Id, p.profileId, p.name, p.description 
             FROM project p
             INNER JOIN acquiredimage ai ON p.Id = ai.projectId
             ORDER BY p.name",
        )?;

        let projects = stmt
            .query_map([], |row| {
                Ok(Project {
                    id: row.get(0)?,
                    profile_id: row.get(1)?,
                    name: row.get(2)?,
                    description: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(projects)
    }

    pub fn find_project_id_by_name(&self, name: &str) -> Result<i32> {
        let mut stmt = self.conn.prepare("SELECT Id FROM project WHERE name = ?")?;
        stmt.query_row([name], |row| row.get(0))
            .with_context(|| format!("Project '{}' not found", name))
    }

    // Target queries
    pub fn get_targets_with_stats(&self, project_id: i32) -> Result<Vec<(Target, i32, i32, i32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.Id, t.name, t.active, t.ra, t.dec,
                    COUNT(ai.Id) as image_count,
                    SUM(CASE WHEN ai.gradingStatus = 1 THEN 1 ELSE 0 END) as accepted_count,
                    SUM(CASE WHEN ai.gradingStatus = 2 THEN 1 ELSE 0 END) as rejected_count
             FROM target t
             LEFT JOIN acquiredimage ai ON t.Id = ai.targetId
             WHERE t.projectid = ?
             GROUP BY t.Id, t.name, t.active, t.ra, t.dec
             ORDER BY t.name",
        )?;

        let targets = stmt
            .query_map([project_id], |row| {
                Ok((
                    Target {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        active: row.get(2)?,
                        ra: row.get(3)?,
                        dec: row.get(4)?,
                        project_id,
                    },
                    row.get::<_, i32>(5)?, // image_count
                    row.get::<_, i32>(6)?, // accepted_count
                    row.get::<_, i32>(7)?, // rejected_count
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(targets)
    }

    pub fn get_targets_with_images(&self, project_id: i32) -> Result<Vec<(Target, i32, i32, i32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.Id, t.name, t.active, t.ra, t.dec,
                    COUNT(ai.Id) as image_count,
                    SUM(CASE WHEN ai.gradingStatus = 1 THEN 1 ELSE 0 END) as accepted_count,
                    SUM(CASE WHEN ai.gradingStatus = 2 THEN 1 ELSE 0 END) as rejected_count
             FROM target t
             INNER JOIN acquiredimage ai ON t.Id = ai.targetId
             WHERE t.projectid = ?
             GROUP BY t.Id, t.name, t.active, t.ra, t.dec
             HAVING COUNT(ai.Id) > 0
             ORDER BY t.name",
        )?;

        let targets = stmt
            .query_map([project_id], |row| {
                Ok((
                    Target {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        active: row.get(2)?,
                        ra: row.get(3)?,
                        dec: row.get(4)?,
                        project_id,
                    },
                    row.get::<_, i32>(5)?, // image_count
                    row.get::<_, i32>(6)?, // accepted_count
                    row.get::<_, i32>(7)?, // rejected_count
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(targets)
    }

    // Image queries
    pub fn get_images_by_project_id(
        &self,
        project_id: i32,
    ) -> Result<Vec<(AcquiredImage, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT ai.Id, ai.projectId, ai.targetId, ai.acquireddate, ai.filtername, 
                    ai.gradingStatus, ai.metadata, ai.rejectreason, ai.profileId,
                    p.name as project_name, t.name as target_name
             FROM acquiredimage ai
             JOIN project p ON ai.projectId = p.Id
             JOIN target t ON ai.targetId = t.Id
             WHERE ai.projectId = ?
             ORDER BY ai.acquireddate DESC",
        )?;

        let rows = stmt.query_map([project_id], |row| {
            let image = AcquiredImage {
                id: row.get(0)?,
                project_id: row.get(1)?,
                target_id: row.get(2)?,
                acquired_date: row.get(3)?,
                filter_name: row.get(4)?,
                grading_status: row.get(5)?,
                metadata: row.get(6)?,
                reject_reason: row.get(7)?,
                profile_id: row.get(8).unwrap_or_default(),
            };
            let project_name: String = row.get(9)?;
            let target_name: String = row.get(10)?;
            Ok((image, project_name, target_name))
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.into())
    }

    pub fn query_images(
        &self,
        status_filter: Option<GradingStatus>,
        project_filter: Option<&str>,
        target_filter: Option<&str>,
        date_cutoff: Option<i64>,
    ) -> Result<Vec<(AcquiredImage, String, String)>> {
        let mut query = String::from(
            "SELECT ai.Id, ai.projectId, ai.targetId, ai.acquireddate, ai.filtername, 
                    ai.gradingStatus, ai.metadata, ai.rejectreason, ai.profileId,
                    p.name as project_name, t.name as target_name
             FROM acquiredimage ai
             JOIN project p ON ai.projectId = p.Id
             JOIN target t ON ai.targetId = t.Id
             WHERE 1=1",
        );

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(status) = status_filter {
            query.push_str(" AND ai.gradingStatus = ?");
            params.push(Box::new(status as i32));
        }

        if let Some(project) = project_filter {
            query.push_str(" AND p.name LIKE ?");
            params.push(Box::new(format!("%{}%", project)));
        }

        if let Some(target) = target_filter {
            query.push_str(" AND t.name LIKE ?");
            params.push(Box::new(format!("%{}%", target)));
        }

        if let Some(cutoff) = date_cutoff {
            query.push_str(" AND ai.acquireddate >= ?");
            params.push(Box::new(cutoff));
        }

        query.push_str(" ORDER BY ai.acquireddate DESC");

        let mut stmt = self.conn.prepare(&query)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let images = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok((
                    AcquiredImage {
                        id: row.get(0)?,
                        project_id: row.get(1)?,
                        target_id: row.get(2)?,
                        acquired_date: row.get(3)?,
                        filter_name: row.get(4)?,
                        grading_status: row.get(5)?,
                        metadata: row.get(6)?,
                        reject_reason: row.get(7)?,
                        profile_id: row.get(8)?,
                    },
                    row.get::<_, String>(9)?,  // project_name
                    row.get::<_, String>(10)?, // target_name
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(images)
    }

    pub fn get_images_by_ids(&self, ids: &[i32]) -> Result<Vec<AcquiredImage>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }

        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT Id, projectId, targetId, acquireddate, filtername, 
                    gradingStatus, metadata, rejectreason, profileId
             FROM acquiredimage
             WHERE Id IN ({})",
            placeholders
        );

        let mut stmt = self.conn.prepare(&query)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();

        let images = stmt
            .query_map(params.as_slice(), |row| {
                Ok(AcquiredImage {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    target_id: row.get(2)?,
                    acquired_date: row.get(3)?,
                    filter_name: row.get(4)?,
                    grading_status: row.get(5)?,
                    metadata: row.get(6)?,
                    reject_reason: row.get(7)?,
                    profile_id: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(images)
    }

    pub fn get_targets_by_ids(&self, ids: &[i32]) -> Result<Vec<Target>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }

        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT Id, projectId, name, active, ra, dec
             FROM target
             WHERE Id IN ({})",
            placeholders
        );

        let mut stmt = self.conn.prepare(&query)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();

        let targets = stmt
            .query_map(params.as_slice(), |row| {
                Ok(Target {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    name: row.get(2)?,
                    active: row.get(3)?,
                    ra: row.get(4)?,
                    dec: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(targets)
    }

    // Update queries
    pub fn update_grading_status(
        &self,
        image_id: i32,
        status: GradingStatus,
        reject_reason: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE acquiredimage 
             SET gradingStatus = ?, rejectreason = ? 
             WHERE Id = ?",
            params![status as i32, reject_reason, image_id],
        )?;
        Ok(())
    }

    pub fn batch_update_grading_status(
        &self,
        updates: &[(i32, GradingStatus, Option<String>)],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        for (id, status, reason) in updates {
            tx.execute(
                "UPDATE acquiredimage 
                 SET gradingStatus = ?, rejectreason = ? 
                 WHERE Id = ?",
                params![*status as i32, reason.as_deref(), id],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn reset_grading_status(
        &self,
        mode: &str,
        date_cutoff: i64,
        project_filter: Option<&str>,
        target_filter: Option<&str>,
    ) -> Result<usize> {
        let mut query = String::from(
            "UPDATE acquiredimage 
             SET gradingStatus = 0, rejectreason = NULL 
             WHERE acquireddate >= ?",
        );

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(date_cutoff)];

        if let Some(project) = project_filter {
            query.push_str(" AND projectId IN (SELECT Id FROM project WHERE name LIKE ?)");
            params.push(Box::new(format!("%{}%", project)));
        }

        if let Some(target) = target_filter {
            query.push_str(" AND targetId IN (SELECT Id FROM target WHERE name LIKE ?)");
            params.push(Box::new(format!("%{}%", target)));
        }

        // For automatic mode, only reset non-manual rejections
        if mode == "automatic" {
            query.push_str(" AND (gradingStatus != 2 OR rejectreason NOT LIKE '%Manual%')");
        }

        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let count = self.conn.execute(&query, param_refs.as_slice())?;

        Ok(count)
    }

    pub fn count_images_to_reset(
        &self,
        mode: &str,
        date_cutoff: i64,
        project_filter: Option<&str>,
        target_filter: Option<&str>,
    ) -> Result<usize> {
        let mut query = String::from(
            "SELECT COUNT(*) 
             FROM acquiredimage 
             WHERE acquireddate >= ?",
        );

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(date_cutoff)];

        if let Some(project) = project_filter {
            query.push_str(" AND projectId IN (SELECT Id FROM project WHERE name LIKE ?)");
            params.push(Box::new(format!("%{}%", project)));
        }

        if let Some(target) = target_filter {
            query.push_str(" AND targetId IN (SELECT Id FROM target WHERE name LIKE ?)");
            params.push(Box::new(format!("%{}%", target)));
        }

        if mode == "automatic" {
            query.push_str(" AND (gradingStatus != 2 OR rejectreason NOT LIKE '%Manual%')");
        }

        query.push_str(" AND gradingStatus != 0");

        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let count: usize = self
            .conn
            .query_row(&query, param_refs.as_slice(), |row| row.get(0))?;

        Ok(count)
    }

    // Transaction helpers
    pub fn with_transaction<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&rusqlite::Transaction) -> Result<T>,
    {
        let tx = self.conn.unchecked_transaction()?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    // Overview and statistics methods
    pub fn get_project_overview_stats(
        &self,
        project_id: i32,
    ) -> Result<(i32, i32, i32, i32, Vec<String>, Option<i64>, Option<i64>)> {
        let mut stmt = self.conn.prepare(
            "SELECT 
                COUNT(*) as total_images,
                SUM(CASE WHEN gradingStatus = 1 THEN 1 ELSE 0 END) as accepted,
                SUM(CASE WHEN gradingStatus = 2 THEN 1 ELSE 0 END) as rejected,
                SUM(CASE WHEN gradingStatus = 0 THEN 1 ELSE 0 END) as pending,
                MIN(acquireddate) as earliest_date,
                MAX(acquireddate) as latest_date
             FROM acquiredimage 
             WHERE projectId = ?",
        )?;

        let (total, accepted, rejected, pending, earliest, latest) =
            stmt.query_row([project_id], |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, i32>(1)?,
                    row.get::<_, i32>(2)?,
                    row.get::<_, i32>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            })?;

        // Get unique filters for this project
        let mut filter_stmt = self.conn.prepare(
            "SELECT DISTINCT filtername FROM acquiredimage WHERE projectId = ? AND filtername IS NOT NULL ORDER BY filtername",
        )?;
        let filters: Vec<String> = filter_stmt
            .query_map([project_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((
            total, accepted, rejected, pending, filters, earliest, latest,
        ))
    }

    pub fn get_target_count_for_project(&self, project_id: i32) -> Result<i32> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM target WHERE projectId = ?")?;
        let count = stmt.query_row([project_id], |row| row.get::<_, i32>(0))?;
        Ok(count)
    }

    pub fn get_overall_statistics(
        &self,
    ) -> Result<(
        i32,
        i32,
        i32,
        i32,
        i32,
        i32,
        i32,
        i32,
        Vec<String>,
        Option<i64>,
        Option<i64>,
    )> {
        // Get overall image statistics
        let mut stmt = self.conn.prepare(
            "SELECT 
                COUNT(*) as total_images,
                SUM(CASE WHEN gradingStatus = 1 THEN 1 ELSE 0 END) as accepted,
                SUM(CASE WHEN gradingStatus = 2 THEN 1 ELSE 0 END) as rejected,
                SUM(CASE WHEN gradingStatus = 0 THEN 1 ELSE 0 END) as pending,
                MIN(acquireddate) as earliest_date,
                MAX(acquireddate) as latest_date
             FROM acquiredimage",
        )?;

        let (total_images, accepted, rejected, pending, earliest, latest) =
            stmt.query_row([], |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, i32>(1)?,
                    row.get::<_, i32>(2)?,
                    row.get::<_, i32>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            })?;

        // Get project counts
        let total_projects = self
            .conn
            .prepare("SELECT COUNT(*) FROM project")?
            .query_row([], |row| row.get::<_, i32>(0))?;
        let active_projects = self
            .conn
            .prepare("SELECT COUNT(DISTINCT projectId) FROM acquiredimage")?
            .query_row([], |row| row.get::<_, i32>(0))?;

        // Get target counts
        let total_targets = self
            .conn
            .prepare("SELECT COUNT(*) FROM target")?
            .query_row([], |row| row.get::<_, i32>(0))?;
        let active_targets = self
            .conn
            .prepare("SELECT COUNT(DISTINCT targetId) FROM acquiredimage")?
            .query_row([], |row| row.get::<_, i32>(0))?;

        // Get unique filters
        let mut filter_stmt = self.conn.prepare("SELECT DISTINCT filtername FROM acquiredimage WHERE filtername IS NOT NULL ORDER BY filtername")?;
        let filters: Vec<String> = filter_stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((
            total_projects,
            active_projects,
            total_targets,
            active_targets,
            total_images,
            accepted,
            rejected,
            pending,
            filters,
            earliest,
            latest,
        ))
    }

    pub fn get_all_targets_with_project_info(
        &self,
    ) -> Result<Vec<(Target, String, i32, i32, i32, i32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.Id, t.name, t.active, t.ra, t.dec, t.projectId, p.name,
                    COUNT(ai.Id) as image_count,
                    SUM(CASE WHEN ai.gradingStatus = 1 THEN 1 ELSE 0 END) as accepted_count,
                    SUM(CASE WHEN ai.gradingStatus = 2 THEN 1 ELSE 0 END) as rejected_count,
                    SUM(CASE WHEN ai.gradingStatus = 0 THEN 1 ELSE 0 END) as pending_count
             FROM target t
             INNER JOIN project p ON t.projectId = p.Id
             LEFT JOIN acquiredimage ai ON t.Id = ai.targetId
             GROUP BY t.Id, t.name, t.active, t.ra, t.dec, t.projectId, p.name
             HAVING COUNT(ai.Id) > 0
             ORDER BY p.name, t.name",
        )?;

        let targets = stmt
            .query_map([], |row| {
                let target = Target {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    active: row.get(2)?,
                    ra: row.get(3)?,
                    dec: row.get(4)?,
                    project_id: row.get(5)?,
                };
                Ok((
                    target,
                    row.get::<_, String>(6)?, // project_name
                    row.get::<_, i32>(7)?,    // image_count
                    row.get::<_, i32>(8)?,    // accepted_count
                    row.get::<_, i32>(9)?,    // rejected_count
                    row.get::<_, i32>(10)?,   // pending_count
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(targets)
    }

    // Requested values queries from exposureplan table
    pub fn get_project_requested_stats(
        &self,
        project_id: i32,
    ) -> Result<(i32, i32, i32, i32, Vec<String>)> {
        let mut stmt = self.conn.prepare(
            "SELECT 
                SUM(ep.desired) as total_requested,
                SUM(ep.acquired) as total_acquired,
                SUM(ep.accepted) as total_accepted,
                COUNT(DISTINCT et.filtername) as unique_filters,
                GROUP_CONCAT(DISTINCT et.filtername) as filter_list
             FROM target t
             JOIN exposureplan ep ON t.Id = ep.targetid
             JOIN exposuretemplate et ON ep.exposureTemplateId = et.Id
             WHERE t.projectid = ?",
        )?;

        let result = stmt.query_row([project_id], |row| {
            let total_requested: i32 = row.get::<_, Option<i32>>(0)?.unwrap_or(0);
            let total_acquired: i32 = row.get::<_, Option<i32>>(1)?.unwrap_or(0);
            let total_accepted: i32 = row.get::<_, Option<i32>>(2)?.unwrap_or(0);
            let unique_filters: i32 = row.get(3)?;
            let filter_list: Option<String> = row.get(4)?;

            let filters = filter_list
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();

            Ok((
                total_requested,
                total_acquired,
                total_accepted,
                unique_filters,
                filters,
            ))
        })?;

        Ok(result)
    }

    pub fn get_target_requested_stats(
        &self,
        target_id: i32,
    ) -> Result<Vec<(String, i32, i32, i32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT 
                et.filtername,
                ep.desired as requested,
                ep.acquired as acquired,
                ep.accepted as accepted
             FROM exposureplan ep
             JOIN exposuretemplate et ON ep.exposureTemplateId = et.Id
             WHERE ep.targetid = ?
             ORDER BY et.filtername",
        )?;

        let rows = stmt
            .query_map([target_id], |row| {
                Ok((
                    row.get::<_, String>(0)?, // filtername
                    row.get::<_, i32>(1)?,    // requested
                    row.get::<_, i32>(2)?,    // acquired
                    row.get::<_, i32>(3)?,    // accepted
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    pub fn get_all_targets_with_requested_stats(
        &self,
    ) -> Result<Vec<(Target, String, i32, i32, i32, i32, i32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.Id, t.name, t.active, t.ra, t.dec, t.projectid, p.name,
                    COUNT(ai.Id) as image_count,
                    SUM(CASE WHEN ai.gradingStatus = 1 THEN 1 ELSE 0 END) as accepted_count,
                    SUM(CASE WHEN ai.gradingStatus = 2 THEN 1 ELSE 0 END) as rejected_count,
                    SUM(CASE WHEN ai.gradingStatus = 0 THEN 1 ELSE 0 END) as pending_count,
                    COALESCE(SUM(ep.desired), 0) as total_requested
             FROM target t
             INNER JOIN project p ON t.projectId = p.Id
             LEFT JOIN acquiredimage ai ON t.Id = ai.targetId
             LEFT JOIN exposureplan ep ON t.Id = ep.targetid
             GROUP BY t.Id, t.name, t.active, t.ra, t.dec, t.projectId, p.name
             HAVING COUNT(ai.Id) > 0 OR SUM(ep.desired) > 0
             ORDER BY p.name, t.name",
        )?;

        let targets = stmt
            .query_map([], |row| {
                let target = Target {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    active: row.get(2)?,
                    ra: row.get(3)?,
                    dec: row.get(4)?,
                    project_id: row.get(5)?,
                };
                Ok((
                    target,
                    row.get::<_, String>(6)?, // project_name
                    row.get::<_, i32>(7)?,    // image_count
                    row.get::<_, i32>(8)?,    // accepted_count
                    row.get::<_, i32>(9)?,    // rejected_count
                    row.get::<_, i32>(10)?,   // pending_count
                    row.get::<_, i32>(11)?,   // total_requested
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(targets)
    }

    pub fn get_overall_requested_statistics(&self) -> Result<(i32, i32, i32)> {
        let mut stmt = self.conn.prepare(
            "SELECT 
                COALESCE(SUM(ep.desired), 0) as total_requested,
                COALESCE(SUM(ep.acquired), 0) as total_acquired,
                COALESCE(SUM(ep.accepted), 0) as total_accepted
             FROM exposureplan ep",
        )?;

        let result = stmt.query_row([], |row| {
            Ok((
                row.get::<_, i32>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, i32>(2)?,
            ))
        })?;

        Ok(result)
    }
}
