use std::collections::HashMap;
use std::path::{Path, PathBuf};

use qualitool_protocol::manifest::ProbeManifest;

/// Unique identifier for a probe invocation within a run.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProbeId(pub String);

/// The structured output of a successful probe execution.
///
/// Probes produce typed structured data as JSON values — never Findings.
/// The output is cached by the scheduler and shared across all dependent nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeOutput(pub serde_json::Value);

/// Context provided to a [`Probe`] during execution.
///
/// Guarantees:
/// - `project_root` is an absolute path to the project being analysed.
/// - `config` holds the resolved (merged) configuration for this probe invocation.
/// - Upstream dependency outputs are available only for probes listed in the
///   manifest's `dependencies` field and only after those probes have succeeded.
pub struct ProbeContext {
    project_root: PathBuf,
    config: serde_json::Value,
    dependency_outputs: HashMap<ProbeId, ProbeOutput>,
}

impl ProbeContext {
    /// Construct a new `ProbeContext`.
    pub fn new(
        project_root: PathBuf,
        config: serde_json::Value,
        dependency_outputs: HashMap<ProbeId, ProbeOutput>,
    ) -> Self {
        Self {
            project_root,
            config,
            dependency_outputs,
        }
    }

    /// Absolute path to the project being analysed.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Resolved configuration for this probe invocation.
    pub fn config(&self) -> &serde_json::Value {
        &self.config
    }

    /// Access the output of an upstream dependency probe.
    ///
    /// Returns `None` if the probe is not a declared dependency or has not
    /// been executed yet.
    pub fn dependency_output(&self, probe_id: &ProbeId) -> Option<&ProbeOutput> {
        self.dependency_outputs.get(probe_id)
    }
}

/// Errors that can occur during probe execution.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    /// The probe's own logic failed.
    #[error("probe execution failed: {message}")]
    ExecutionFailed {
        message: String,
        #[source]
        source: Option<anyhow::Error>,
    },

    /// A declared dependency's output was not available in the context.
    #[error("missing required dependency output: {probe_id}")]
    MissingDependency { probe_id: String },
}

/// A read-only, side-effect-free, cacheable data-gathering primitive.
///
/// Probes collect structured data from a project (file trees, git history,
/// dependency graphs, etc.) and produce a [`ProbeOutput`]. They never produce
/// Findings — that is the responsibility of [`Check`](super::check::Check).
pub trait Probe: Send + Sync {
    /// The probe's manifest describing its identity, schemas, and dependencies.
    fn manifest(&self) -> &ProbeManifest;

    /// Execute the probe against the given context.
    fn run(
        &self,
        ctx: &ProbeContext,
    ) -> impl std::future::Future<Output = Result<ProbeOutput, ProbeError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FileCountProbe {
        manifest: ProbeManifest,
    }

    impl FileCountProbe {
        fn new() -> Self {
            Self {
                manifest: ProbeManifest {
                    name: "file-count".into(),
                    version: "0.1.0".into(),
                    description: Some("Counts files in the project".into()),
                    input_schema: None,
                    output_schema: None,
                    dependencies: vec![],
                    contains_source_code: false,
                },
            }
        }
    }

    impl Probe for FileCountProbe {
        fn manifest(&self) -> &ProbeManifest {
            &self.manifest
        }

        async fn run(&self, ctx: &ProbeContext) -> Result<ProbeOutput, ProbeError> {
            let _root = ctx.project_root();
            Ok(ProbeOutput(serde_json::json!({"count": 42})))
        }
    }

    struct DependentProbe {
        manifest: ProbeManifest,
    }

    impl DependentProbe {
        fn new() -> Self {
            Self {
                manifest: ProbeManifest {
                    name: "dependent".into(),
                    version: "0.1.0".into(),
                    description: None,
                    input_schema: None,
                    output_schema: None,
                    dependencies: vec!["file-count".into()],
                    contains_source_code: false,
                },
            }
        }
    }

    impl Probe for DependentProbe {
        fn manifest(&self) -> &ProbeManifest {
            &self.manifest
        }

        async fn run(&self, ctx: &ProbeContext) -> Result<ProbeOutput, ProbeError> {
            let upstream = ctx
                .dependency_output(&ProbeId("file-count".into()))
                .ok_or_else(|| ProbeError::MissingDependency {
                    probe_id: "file-count".into(),
                })?;

            let count = upstream.0["count"].as_u64().unwrap_or(0);
            Ok(ProbeOutput(serde_json::json!({"doubled": count * 2})))
        }
    }

    #[tokio::test]
    async fn probe_runs_and_returns_output() {
        let probe = FileCountProbe::new();
        let ctx = ProbeContext::new(
            PathBuf::from("/tmp/project"),
            serde_json::json!({}),
            HashMap::new(),
        );

        let output = probe.run(&ctx).await.unwrap();
        assert_eq!(output.0["count"], 42);
    }

    #[tokio::test]
    async fn probe_context_exposes_project_root() {
        let ctx = ProbeContext::new(
            PathBuf::from("/home/user/project"),
            serde_json::json!({}),
            HashMap::new(),
        );
        assert_eq!(ctx.project_root(), Path::new("/home/user/project"));
    }

    #[tokio::test]
    async fn probe_context_exposes_config() {
        let config = serde_json::json!({"threshold": 100});
        let ctx = ProbeContext::new(PathBuf::from("/tmp"), config.clone(), HashMap::new());
        assert_eq!(ctx.config(), &config);
    }

    #[tokio::test]
    async fn probe_context_provides_dependency_outputs() {
        let mut deps = HashMap::new();
        deps.insert(
            ProbeId("file-count".into()),
            ProbeOutput(serde_json::json!({"count": 10})),
        );

        let ctx = ProbeContext::new(PathBuf::from("/tmp"), serde_json::json!({}), deps);

        let output = ctx.dependency_output(&ProbeId("file-count".into()));
        assert!(output.is_some());
        assert_eq!(output.unwrap().0["count"], 10);

        let missing = ctx.dependency_output(&ProbeId("nonexistent".into()));
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn dependent_probe_reads_upstream_output() {
        let probe = DependentProbe::new();
        let mut deps = HashMap::new();
        deps.insert(
            ProbeId("file-count".into()),
            ProbeOutput(serde_json::json!({"count": 21})),
        );

        let ctx = ProbeContext::new(PathBuf::from("/tmp"), serde_json::json!({}), deps);
        let output = probe.run(&ctx).await.unwrap();
        assert_eq!(output.0["doubled"], 42);
    }

    #[tokio::test]
    async fn dependent_probe_fails_on_missing_dependency() {
        let probe = DependentProbe::new();
        let ctx = ProbeContext::new(
            PathBuf::from("/tmp"),
            serde_json::json!({}),
            HashMap::new(),
        );

        let err = probe.run(&ctx).await.unwrap_err();
        assert!(matches!(err, ProbeError::MissingDependency { .. }));

        let msg = err.to_string();
        assert!(msg.contains("file-count"));
    }

    #[test]
    fn probe_error_execution_failed_displays_message() {
        let err = ProbeError::ExecutionFailed {
            message: "disk read failed".into(),
            source: None,
        };
        assert_eq!(err.to_string(), "probe execution failed: disk read failed");
    }

    #[test]
    fn probe_manifest_accessible() {
        let probe = FileCountProbe::new();
        assert_eq!(probe.manifest().name, "file-count");
        assert_eq!(probe.manifest().version, "0.1.0");
    }
}
