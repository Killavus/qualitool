use std::collections::HashMap;
use std::path::{Path, PathBuf};

use qualitool_protocol::check::CheckOutput;
use qualitool_protocol::manifest::CheckManifest;

use crate::probe::{ProbeId, ProbeOutput};

/// Context provided to a [`Check`] during execution.
///
/// Guarantees:
/// - `project_root` is an absolute path to the project being analysed.
/// - `config` holds the resolved (merged) configuration for this check invocation.
/// - Probe outputs are available read-only for probes listed in the check's
///   manifest `dependencies` field, after those probes have succeeded.
pub struct CheckContext {
    project_root: PathBuf,
    config: serde_json::Value,
    probe_outputs: HashMap<ProbeId, ProbeOutput>,
}

impl CheckContext {
    /// Construct a new `CheckContext`.
    pub fn new(
        project_root: PathBuf,
        config: serde_json::Value,
        probe_outputs: HashMap<ProbeId, ProbeOutput>,
    ) -> Self {
        Self {
            project_root,
            config,
            probe_outputs,
        }
    }

    /// Absolute path to the project being analysed.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Resolved configuration for this check invocation.
    pub fn config(&self) -> &serde_json::Value {
        &self.config
    }

    /// Read-only access to a probe's output.
    ///
    /// Returns `None` if the probe is not a declared dependency or has not
    /// been executed yet.
    pub fn probe_output(&self, probe_id: &ProbeId) -> Option<&ProbeOutput> {
        self.probe_outputs.get(probe_id)
    }
}

/// Errors that can occur during check execution.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    /// The check's own logic failed.
    #[error("check execution failed: {message}")]
    ExecutionFailed {
        message: String,
        #[source]
        source: Option<anyhow::Error>,
    },

    /// A required probe output was not available in the context.
    #[error("missing required probe output: {probe_id}")]
    MissingProbeOutput { probe_id: String },
}

/// A judgment/heuristic/scoring primitive that consumes probe outputs
/// and produces [`Finding`](qualitool_protocol::Finding)s or a
/// [`CallAgent`](CheckOutput::CallAgent) effect.
///
/// Checks never gather raw data directly — that is the responsibility of
/// [`Probe`](super::probe::Probe). A check emits exactly one action:
/// either terminal `Findings` or a single-turn `CallAgent` request that
/// the core scheduler fulfils on its behalf.
pub trait Check: Send + Sync {
    /// The check's manifest describing its identity, schemas, and dependencies.
    fn manifest(&self) -> &CheckManifest;

    /// Execute the check against the given context.
    fn run(
        &self,
        ctx: &CheckContext,
    ) -> impl std::future::Future<Output = Result<CheckOutput, CheckError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use qualitool_protocol::agent::{AgentConstraints, AgentRequest};
    use qualitool_protocol::finding::{Finding, FindingId, Severity};

    struct FileCountCheck {
        manifest: CheckManifest,
    }

    impl FileCountCheck {
        fn new() -> Self {
            Self {
                manifest: CheckManifest {
                    name: "file-count".into(),
                    version: "0.1.0".into(),
                    description: Some("Checks if file count exceeds threshold".into()),
                    input_schema: None,
                    output_schema: None,
                    dependencies: vec!["file-tree".into()],
                },
            }
        }
    }

    impl Check for FileCountCheck {
        fn manifest(&self) -> &CheckManifest {
            &self.manifest
        }

        async fn run(&self, ctx: &CheckContext) -> Result<CheckOutput, CheckError> {
            let file_tree = ctx
                .probe_output(&ProbeId("file-tree".into()))
                .ok_or_else(|| CheckError::MissingProbeOutput {
                    probe_id: "file-tree".into(),
                })?;

            let count = file_tree.0["count"].as_u64().unwrap_or(0);
            let threshold = ctx.config()["threshold"].as_u64().unwrap_or(1000);

            if count > threshold {
                Ok(CheckOutput::Findings {
                    findings: vec![Finding {
                        id: FindingId("fc-1".into()),
                        check_id: "file-count".into(),
                        severity: Severity::Medium,
                        title: "High file count".into(),
                        summary: format!("{count} files exceed threshold of {threshold}"),
                        location: None,
                        tags: vec!["scale".into()],
                        payload: serde_json::json!({"count": count, "threshold": threshold}),
                    }],
                })
            } else {
                Ok(CheckOutput::Findings {
                    findings: vec![],
                })
            }
        }
    }

    struct AgentCheck {
        manifest: CheckManifest,
    }

    impl AgentCheck {
        fn new() -> Self {
            Self {
                manifest: CheckManifest {
                    name: "architecture-smell".into(),
                    version: "0.1.0".into(),
                    description: Some("Delegates analysis to an AI agent".into()),
                    input_schema: None,
                    output_schema: None,
                    dependencies: vec!["file-tree".into()],
                },
            }
        }
    }

    impl Check for AgentCheck {
        fn manifest(&self) -> &CheckManifest {
            &self.manifest
        }

        async fn run(&self, _ctx: &CheckContext) -> Result<CheckOutput, CheckError> {
            Ok(CheckOutput::CallAgent {
                request: AgentRequest {
                    agent_hint: None,
                    prompt: "Analyse architecture".into(),
                    include_probes: vec!["file-tree".into()],
                    response_schema: serde_json::json!({"type": "object"}),
                    constraints: AgentConstraints {
                        max_tokens: Some(4000),
                        timeout_seconds: Some(60),
                        read_only: true,
                    },
                },
            })
        }
    }

    #[tokio::test]
    async fn check_returns_findings_above_threshold() {
        let check = FileCountCheck::new();
        let mut probes = HashMap::new();
        probes.insert(
            ProbeId("file-tree".into()),
            ProbeOutput(serde_json::json!({"count": 5000})),
        );

        let ctx = CheckContext::new(
            PathBuf::from("/tmp/project"),
            serde_json::json!({"threshold": 1000}),
            probes,
        );

        let output = check.run(&ctx).await.unwrap();
        match &output {
            CheckOutput::Findings { findings } => {
                assert_eq!(findings.len(), 1);
                assert_eq!(findings[0].severity, Severity::Medium);
                assert!(findings[0].summary.contains("5000"));
            }
            CheckOutput::CallAgent { .. } => panic!("expected Findings"),
        }
    }

    #[tokio::test]
    async fn check_returns_empty_findings_below_threshold() {
        let check = FileCountCheck::new();
        let mut probes = HashMap::new();
        probes.insert(
            ProbeId("file-tree".into()),
            ProbeOutput(serde_json::json!({"count": 50})),
        );

        let ctx = CheckContext::new(
            PathBuf::from("/tmp/project"),
            serde_json::json!({"threshold": 1000}),
            probes,
        );

        let output = check.run(&ctx).await.unwrap();
        match output {
            CheckOutput::Findings { findings } => assert!(findings.is_empty()),
            CheckOutput::CallAgent { .. } => panic!("expected Findings"),
        }
    }

    #[tokio::test]
    async fn check_returns_call_agent() {
        let check = AgentCheck::new();
        let ctx = CheckContext::new(
            PathBuf::from("/tmp/project"),
            serde_json::json!({}),
            HashMap::new(),
        );

        let output = check.run(&ctx).await.unwrap();
        match &output {
            CheckOutput::CallAgent { request } => {
                assert_eq!(request.prompt, "Analyse architecture");
                assert!(request.constraints.read_only);
            }
            CheckOutput::Findings { .. } => panic!("expected CallAgent"),
        }
    }

    #[tokio::test]
    async fn check_fails_on_missing_probe_output() {
        let check = FileCountCheck::new();
        let ctx = CheckContext::new(
            PathBuf::from("/tmp/project"),
            serde_json::json!({}),
            HashMap::new(),
        );

        let err = check.run(&ctx).await.unwrap_err();
        assert!(matches!(err, CheckError::MissingProbeOutput { .. }));
        assert!(err.to_string().contains("file-tree"));
    }

    #[tokio::test]
    async fn check_context_exposes_project_root() {
        let ctx = CheckContext::new(
            PathBuf::from("/home/user/project"),
            serde_json::json!({}),
            HashMap::new(),
        );
        assert_eq!(ctx.project_root(), Path::new("/home/user/project"));
    }

    #[tokio::test]
    async fn check_context_exposes_config() {
        let config = serde_json::json!({"severity_threshold": "high"});
        let ctx = CheckContext::new(PathBuf::from("/tmp"), config.clone(), HashMap::new());
        assert_eq!(ctx.config(), &config);
    }

    #[test]
    fn check_error_execution_failed_displays_message() {
        let err = CheckError::ExecutionFailed {
            message: "analysis failed".into(),
            source: None,
        };
        assert_eq!(
            err.to_string(),
            "check execution failed: analysis failed"
        );
    }

    #[test]
    fn check_manifest_accessible() {
        let check = FileCountCheck::new();
        assert_eq!(check.manifest().name, "file-count");
        assert_eq!(check.manifest().dependencies, vec!["file-tree"]);
    }
}
