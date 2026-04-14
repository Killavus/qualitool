use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct FindingId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CodeLocation {
    pub file: PathBuf,
    pub line_start: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col_start: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col_end: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Finding {
    pub id: FindingId,
    pub check_id: String,
    pub severity: Severity,
    pub title: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<CodeLocation>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub payload: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finding_round_trip() {
        let finding = Finding {
            id: FindingId("abc123".into()),
            check_id: "file-count".into(),
            severity: Severity::Medium,
            title: "Too many files".into(),
            summary: "Project has 10,000+ files".into(),
            location: Some(CodeLocation {
                file: PathBuf::from("src/main.rs"),
                line_start: 1,
                line_end: Some(50),
                col_start: None,
                col_end: None,
            }),
            tags: vec!["performance".into(), "scale".into()],
            payload: serde_json::json!({"file_count": 10000}),
        };

        let json = serde_json::to_string(&finding).unwrap();
        let deserialized: Finding = serde_json::from_str(&json).unwrap();
        assert_eq!(finding, deserialized);
    }

    #[test]
    fn finding_without_location_round_trip() {
        let finding = Finding {
            id: FindingId("def456".into()),
            check_id: "language-mix".into(),
            severity: Severity::Info,
            title: "Language distribution".into(),
            summary: "Project uses 3 languages".into(),
            location: None,
            tags: vec![],
            payload: serde_json::json!({"languages": ["rust", "typescript", "python"]}),
        };

        let json = serde_json::to_string(&finding).unwrap();
        let deserialized: Finding = serde_json::from_str(&json).unwrap();
        assert_eq!(finding, deserialized);

        // location should be absent from JSON
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("location").is_none());
    }

    #[test]
    fn severity_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Severity::Info).unwrap(), "\"info\"");
        assert_eq!(
            serde_json::to_string(&Severity::Critical).unwrap(),
            "\"critical\""
        );
    }

    #[test]
    fn code_location_minimal() {
        let loc = CodeLocation {
            file: PathBuf::from("lib.rs"),
            line_start: 42,
            line_end: None,
            col_start: None,
            col_end: None,
        };

        let json = serde_json::to_string(&loc).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("line_end").is_none());
        assert!(value.get("col_start").is_none());

        let deserialized: CodeLocation = serde_json::from_str(&json).unwrap();
        assert_eq!(loc, deserialized);
    }
}
