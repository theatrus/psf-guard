use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: i32,
    pub profile_id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Target {
    pub id: i32,
    pub name: String,
    pub active: bool,
    pub ra: Option<f64>,
    pub dec: Option<f64>,
    pub project_id: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcquiredImage {
    pub id: i32,
    pub project_id: i32,
    pub target_id: i32,
    pub acquired_date: Option<i64>,
    pub filter_name: String,
    pub grading_status: i32,
    pub metadata: String,
    pub reject_reason: Option<String>,
    pub profile_id: Option<String>,
}

#[derive(Debug, Copy, Clone)]
pub enum GradingStatus {
    Pending = 0,
    Accepted = 1,
    Rejected = 2,
}

/// Project overview statistics
#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectOverviewStats {
    pub total_images: i32,
    pub accepted_images: i32,
    pub rejected_images: i32,
    pub pending_images: i32,
    pub filters_used: Vec<String>,
    pub earliest_date: Option<i64>,
    pub latest_date: Option<i64>,
}

/// Overall system statistics
#[derive(Debug, Serialize, Deserialize)]
pub struct OverallStats {
    pub total_images: i32,
    pub accepted_images: i32,
    pub rejected_images: i32,
    pub pending_images: i32,
    pub active_projects: i32,
    pub total_projects: i32,
    pub active_targets: i32,
    pub total_targets: i32,
    pub unique_filters: Vec<String>,
    pub earliest_date: Option<i64>,
    pub latest_date: Option<i64>,
    pub files_found: i32,
    pub files_missing: i32,
}

/// Project desired statistics
#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectDesiredStats {
    pub total_desired: i32,
    pub total_acquired: i32,
    pub total_accepted: i32,
    pub rejected_count: i32,
    pub filters_used: Vec<String>,
}

/// Overall desired statistics
#[derive(Debug, Serialize, Deserialize)]
pub struct OverallDesiredStats {
    pub total_desired: i32,
    pub total_acquired: i32,
    pub total_accepted: i32,
}

/// Target with statistics
#[derive(Debug, Serialize, Deserialize)]
pub struct TargetWithStats {
    pub target: Target,
    pub project_name: String,
    pub total_images: i32,
    pub accepted_images: i32,
    pub rejected_images: i32,
    pub pending_images: i32,
}

/// Target with statistics and desired counts
#[derive(Debug, Serialize, Deserialize)]
pub struct TargetWithDesiredStats {
    pub target: Target,
    pub project_name: String,
    pub total_images: i32,
    pub accepted_images: i32,
    pub rejected_images: i32,
    pub pending_images: i32,
    pub total_desired: i32,
}

impl GradingStatus {
    pub fn from_i32(value: i32) -> &'static str {
        match value {
            0 => "Pending",
            1 => "Accepted",
            2 => "Rejected",
            _ => "Unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grading_status_from_i32() {
        assert_eq!(GradingStatus::from_i32(0), "Pending");
        assert_eq!(GradingStatus::from_i32(1), "Accepted");
        assert_eq!(GradingStatus::from_i32(2), "Rejected");
        assert_eq!(GradingStatus::from_i32(3), "Unknown");
        assert_eq!(GradingStatus::from_i32(-1), "Unknown");
        assert_eq!(GradingStatus::from_i32(999), "Unknown");
    }

    #[test]
    fn test_grading_status_enum_values() {
        assert_eq!(GradingStatus::Pending as i32, 0);
        assert_eq!(GradingStatus::Accepted as i32, 1);
        assert_eq!(GradingStatus::Rejected as i32, 2);
    }

    #[test]
    fn test_project_serialization() {
        let project = Project {
            id: 1,
            profile_id: "test-profile".to_string(),
            name: "Test Project".to_string(),
            description: Some("A test project".to_string()),
        };

        let json = serde_json::to_string(&project).unwrap();
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"profile_id\":\"test-profile\""));
        assert!(json.contains("\"name\":\"Test Project\""));
        assert!(json.contains("\"description\":\"A test project\""));

        let deserialized: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, project.id);
        assert_eq!(deserialized.name, project.name);
    }

    #[test]
    fn test_project_with_null_description() {
        let json = r#"{"id":1,"profile_id":"test","name":"Test","description":null}"#;
        let project: Project = serde_json::from_str(json).unwrap();
        assert_eq!(project.description, None);
    }

    #[test]
    fn test_acquired_image_complete() {
        let image = AcquiredImage {
            id: 123,
            project_id: 1,
            target_id: 2,
            acquired_date: Some(1693526400),
            filter_name: "Ha".to_string(),
            grading_status: 1,
            metadata: r#"{"test": "data"}"#.to_string(),
            reject_reason: Some("Too cloudy".to_string()),
            profile_id: Some("profile-123".to_string()),
        };

        let json = serde_json::to_string(&image).unwrap();
        let deserialized: AcquiredImage = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, image.id);
        assert_eq!(deserialized.project_id, image.project_id);
        assert_eq!(deserialized.target_id, image.target_id);
        assert_eq!(deserialized.acquired_date, image.acquired_date);
        assert_eq!(deserialized.filter_name, image.filter_name);
        assert_eq!(deserialized.grading_status, image.grading_status);
        assert_eq!(deserialized.metadata, image.metadata);
        assert_eq!(deserialized.reject_reason, image.reject_reason);
        assert_eq!(deserialized.profile_id, image.profile_id);
    }
}
