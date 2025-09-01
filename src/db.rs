use crate::models::{
    AcquiredImage, GradingStatus, OverallDesiredStats, OverallStats, Project, ProjectDesiredStats,
    ProjectOverviewStats, Target, TargetWithDesiredStats, TargetWithStats,
};
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
    pub fn get_project_overview_stats(&self, project_id: i32) -> Result<ProjectOverviewStats> {
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

        Ok(ProjectOverviewStats {
            total_images: total,
            accepted_images: accepted,
            rejected_images: rejected,
            pending_images: pending,
            filters_used: filters,
            earliest_date: earliest,
            latest_date: latest,
        })
    }

    pub fn get_target_count_for_project(&self, project_id: i32) -> Result<i32> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM target WHERE projectId = ?")?;
        let count = stmt.query_row([project_id], |row| row.get::<_, i32>(0))?;
        Ok(count)
    }

    pub fn get_overall_statistics(&self) -> Result<OverallStats> {
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

        Ok(OverallStats {
            total_images,
            accepted_images: accepted,
            rejected_images: rejected,
            pending_images: pending,
            active_projects,
            total_projects,
            active_targets,
            total_targets,
            unique_filters: filters,
            earliest_date: earliest,
            latest_date: latest,
            files_found: 0,   // Will be set by caller if needed
            files_missing: 0, // Will be set by caller if needed
        })
    }

    pub fn get_all_targets_with_project_info(&self) -> Result<Vec<TargetWithStats>> {
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
                Ok(TargetWithStats {
                    target,
                    project_name: row.get::<_, String>(6)?,
                    total_images: row.get::<_, i32>(7)?,
                    accepted_images: row.get::<_, i32>(8)?,
                    rejected_images: row.get::<_, i32>(9)?,
                    pending_images: row.get::<_, i32>(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(targets)
    }

    // Desired values queries from exposureplan table
    pub fn get_project_desired_stats(&self, project_id: i32) -> Result<ProjectDesiredStats> {
        let mut stmt = self.conn.prepare(
            "SELECT 
                SUM(ep.desired) as total_desired,
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
            let total_desired: i32 = row.get::<_, Option<i32>>(0)?.unwrap_or(0);
            let total_acquired: i32 = row.get::<_, Option<i32>>(1)?.unwrap_or(0);
            let total_accepted: i32 = row.get::<_, Option<i32>>(2)?.unwrap_or(0);
            let _unique_filters: i32 = row.get(3)?;
            let filter_list: Option<String> = row.get(4)?;

            let filters = filter_list
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();

            Ok(ProjectDesiredStats {
                total_desired,
                total_acquired,
                total_accepted,
                rejected_count: 0, // Not available in this query
                filters_used: filters,
            })
        })?;

        Ok(result)
    }

    pub fn get_target_desired_stats(&self, target_id: i32) -> Result<Vec<(String, i32, i32, i32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT 
                et.filtername,
                ep.desired as desired,
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
                    row.get::<_, i32>(1)?,    // desired
                    row.get::<_, i32>(2)?,    // acquired
                    row.get::<_, i32>(3)?,    // accepted
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    pub fn get_all_targets_with_desired_stats(&self) -> Result<Vec<TargetWithDesiredStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.Id, t.name, t.active, t.ra, t.dec, t.projectid, p.name,
                    COUNT(DISTINCT ai.Id) as image_count,
                    SUM(CASE WHEN ai.gradingStatus = 1 THEN 1 ELSE 0 END) as accepted_count,
                    SUM(CASE WHEN ai.gradingStatus = 2 THEN 1 ELSE 0 END) as rejected_count,
                    SUM(CASE WHEN ai.gradingStatus = 0 THEN 1 ELSE 0 END) as pending_count,
                    COALESCE((SELECT SUM(ep2.desired) FROM exposureplan ep2 WHERE ep2.targetid = t.Id), 0) as total_desired
             FROM target t
             INNER JOIN project p ON t.projectId = p.Id
             LEFT JOIN acquiredimage ai ON t.Id = ai.targetId
             GROUP BY t.Id, t.name, t.active, t.ra, t.dec, t.projectId, p.name
             HAVING COUNT(DISTINCT ai.Id) > 0 OR (SELECT SUM(ep2.desired) FROM exposureplan ep2 WHERE ep2.targetid = t.Id) > 0
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
                Ok(TargetWithDesiredStats {
                    target,
                    project_name: row.get::<_, String>(6)?,
                    total_images: row.get::<_, i32>(7)?,
                    accepted_images: row.get::<_, i32>(8)?,
                    rejected_images: row.get::<_, i32>(9)?,
                    pending_images: row.get::<_, i32>(10)?,
                    total_desired: row.get::<_, i32>(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(targets)
    }

    pub fn get_overall_desired_statistics(&self) -> Result<OverallDesiredStats> {
        let mut stmt = self.conn.prepare(
            "SELECT 
                COALESCE(SUM(ep.desired), 0) as total_desired,
                COALESCE(SUM(ep.acquired), 0) as total_acquired,
                COALESCE(SUM(ep.accepted), 0) as total_accepted
             FROM exposureplan ep",
        )?;

        let result = stmt.query_row([], |row| {
            Ok(OverallDesiredStats {
                total_desired: row.get::<_, i32>(0)?,
                total_acquired: row.get::<_, i32>(1)?,
                total_accepted: row.get::<_, i32>(2)?,
            })
        })?;

        Ok(result)
    }
}
