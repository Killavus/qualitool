use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProbeManifest {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub contains_source_code: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CheckManifest {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_manifest_round_trip() {
        let manifest = ProbeManifest {
            name: "file-tree".into(),
            version: "0.1.0".into(),
            description: Some("Recursive file listing".into()),
            input_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "includes": {"type": "array", "items": {"type": "string"}},
                    "excludes": {"type": "array", "items": {"type": "string"}}
                }
            })),
            output_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "files": {"type": "array", "items": {"type": "string"}}
                }
            })),
            dependencies: vec![],
            contains_source_code: false,
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let deserialized: ProbeManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(manifest, deserialized);
    }

    #[test]
    fn check_manifest_round_trip() {
        let manifest = CheckManifest {
            name: "file-count".into(),
            version: "0.1.0".into(),
            description: Some("Counts files and reports thresholds".into()),
            input_schema: None,
            output_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "count": {"type": "integer"}
                }
            })),
            dependencies: vec!["file-tree".into()],
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let deserialized: CheckManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(manifest, deserialized);
    }

    #[test]
    fn probe_manifest_minimal() {
        let json = r#"{"name":"test","version":"0.1.0"}"#;
        let manifest: ProbeManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "test");
        assert!(manifest.description.is_none());
        assert!(manifest.input_schema.is_none());
        assert!(manifest.output_schema.is_none());
        assert!(manifest.dependencies.is_empty());
        assert!(!manifest.contains_source_code);
    }
}
