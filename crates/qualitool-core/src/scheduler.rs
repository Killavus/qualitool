use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use qualitool_protocol::check::CheckOutput;
use qualitool_protocol::manifest::{CheckManifest, ProbeManifest};

use crate::check::{Check, CheckContext, CheckError};
use crate::probe::{Probe, ProbeContext, ProbeError, ProbeId, ProbeOutput};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Identifies a node in the execution DAG.
///
/// Probes and checks live in separate namespaces — a probe and a check may
/// share the same name without conflict. Derived `Ord` places probes before
/// checks, which gives a natural "gather-then-judge" ordering when there are
/// no other constraints.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeId {
    Probe(String),
    Check(String),
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeId::Probe(name) => write!(f, "probe:{name}"),
            NodeId::Check(name) => write!(f, "check:{name}"),
        }
    }
}

/// Errors that occur when building a schedule (before any execution).
#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    /// The dependency graph contains a cycle.
    #[error("dependency cycle detected: {}", cycle.join(" -> "))]
    Cycle { cycle: Vec<String> },

    /// A node declares a dependency on a node that was not registered.
    #[error("unknown dependency '{dependency}' declared by node '{node}'")]
    UnknownDependency { node: String, dependency: String },

    /// Two nodes of the same kind share a name.
    #[error("duplicate node name '{name}'")]
    DuplicateNode { name: String },
}

/// Errors that occur during schedule execution.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// A probe failed during execution.
    #[error("probe '{name}' failed")]
    ProbeFailed {
        name: String,
        #[source]
        source: ProbeError,
    },

    /// A check failed during execution.
    #[error("check '{name}' failed")]
    CheckFailed {
        name: String,
        #[source]
        source: CheckError,
    },
}

/// The output of a successful schedule execution.
#[derive(Debug)]
pub struct RunResult {
    /// Check outputs in execution order, keyed by check name.
    pub check_outputs: Vec<(String, CheckOutput)>,
}

// ---------------------------------------------------------------------------
// Type-erasure traits (internal)
// ---------------------------------------------------------------------------

trait DynProbe: Send + Sync {
    fn manifest(&self) -> &ProbeManifest;
    fn run_dyn<'a>(
        &'a self,
        ctx: &'a ProbeContext,
    ) -> Pin<Box<dyn Future<Output = Result<ProbeOutput, ProbeError>> + Send + 'a>>;
}

impl<T: Probe> DynProbe for T {
    fn manifest(&self) -> &ProbeManifest {
        Probe::manifest(self)
    }
    fn run_dyn<'a>(
        &'a self,
        ctx: &'a ProbeContext,
    ) -> Pin<Box<dyn Future<Output = Result<ProbeOutput, ProbeError>> + Send + 'a>> {
        Box::pin(Probe::run(self, ctx))
    }
}

trait DynCheck: Send + Sync {
    fn manifest(&self) -> &CheckManifest;
    fn run_dyn<'a>(
        &'a self,
        ctx: &'a CheckContext,
    ) -> Pin<Box<dyn Future<Output = Result<CheckOutput, CheckError>> + Send + 'a>>;
}

impl<T: Check> DynCheck for T {
    fn manifest(&self) -> &CheckManifest {
        Check::manifest(self)
    }
    fn run_dyn<'a>(
        &'a self,
        ctx: &'a CheckContext,
    ) -> Pin<Box<dyn Future<Output = Result<CheckOutput, CheckError>> + Send + 'a>> {
        Box::pin(Check::run(self, ctx))
    }
}

// ---------------------------------------------------------------------------
// SchedulerBuilder
// ---------------------------------------------------------------------------

/// Fluent builder for constructing a validated [`Scheduler`].
pub struct SchedulerBuilder {
    probes: Vec<Box<dyn DynProbe>>,
    checks: Vec<Box<dyn DynCheck>>,
}

impl SchedulerBuilder {
    pub fn new() -> Self {
        Self {
            probes: Vec::new(),
            checks: Vec::new(),
        }
    }

    /// Register a probe with the scheduler.
    pub fn add_probe(mut self, probe: impl Probe + 'static) -> Self {
        self.probes.push(Box::new(probe));
        self
    }

    /// Register a check with the scheduler.
    pub fn add_check(mut self, check: impl Check + 'static) -> Self {
        self.checks.push(Box::new(check));
        self
    }

    /// Validate dependencies, detect cycles, and produce a ready-to-run
    /// [`Scheduler`] with a deterministic execution order.
    pub fn build(self) -> Result<Scheduler, ScheduleError> {
        let mut all_nodes = HashSet::new();

        for p in &self.probes {
            let id = NodeId::Probe(p.manifest().name.clone());
            if !all_nodes.insert(id) {
                return Err(ScheduleError::DuplicateNode {
                    name: p.manifest().name.clone(),
                });
            }
        }

        for c in &self.checks {
            let id = NodeId::Check(c.manifest().name.clone());
            if !all_nodes.insert(id) {
                return Err(ScheduleError::DuplicateNode {
                    name: c.manifest().name.clone(),
                });
            }
        }

        // Build dependency map: node -> [nodes it depends on]
        let mut deps: HashMap<NodeId, Vec<NodeId>> = HashMap::new();

        for p in &self.probes {
            let manifest = p.manifest();
            let node = NodeId::Probe(manifest.name.clone());
            let mut node_deps = Vec::new();

            for dep_name in &manifest.dependencies {
                let dep = NodeId::Probe(dep_name.clone());
                if !all_nodes.contains(&dep) {
                    return Err(ScheduleError::UnknownDependency {
                        node: node.to_string(),
                        dependency: dep.to_string(),
                    });
                }
                node_deps.push(dep);
            }

            deps.insert(node, node_deps);
        }

        for c in &self.checks {
            let manifest = c.manifest();
            let node = NodeId::Check(manifest.name.clone());
            let mut node_deps = Vec::new();

            for dep_name in &manifest.dependencies {
                let dep = NodeId::Probe(dep_name.clone());
                if !all_nodes.contains(&dep) {
                    return Err(ScheduleError::UnknownDependency {
                        node: node.to_string(),
                        dependency: dep.to_string(),
                    });
                }
                node_deps.push(dep);
            }

            deps.insert(node, node_deps);
        }

        let nodes: Vec<NodeId> = all_nodes.into_iter().collect();
        let execution_order = topological_sort(&nodes, &deps)?;

        let probes: HashMap<String, Box<dyn DynProbe>> = self
            .probes
            .into_iter()
            .map(|p| (p.manifest().name.clone(), p))
            .collect();
        let checks: HashMap<String, Box<dyn DynCheck>> = self
            .checks
            .into_iter()
            .map(|c| (c.manifest().name.clone(), c))
            .collect();

        Ok(Scheduler {
            execution_order,
            probes,
            checks,
        })
    }
}

impl Default for SchedulerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/// A validated, topologically-sorted execution schedule.
///
/// Built via [`SchedulerBuilder`] which performs dependency validation and
/// cycle detection at construction time. Call [`run`](Scheduler::run) to
/// execute all nodes sequentially in dependency order.
pub struct Scheduler {
    execution_order: Vec<NodeId>,
    probes: HashMap<String, Box<dyn DynProbe>>,
    checks: HashMap<String, Box<dyn DynCheck>>,
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("execution_order", &self.execution_order)
            .field("probes", &self.probes.keys().collect::<Vec<_>>())
            .field("checks", &self.checks.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Scheduler {
    /// Start building a new scheduler.
    pub fn builder() -> SchedulerBuilder {
        SchedulerBuilder::new()
    }

    /// The deterministic execution order produced by topological sort.
    pub fn execution_order(&self) -> &[NodeId] {
        &self.execution_order
    }

    /// Execute all nodes sequentially in topological order.
    ///
    /// Probe outputs are memoized and injected into downstream contexts
    /// automatically. Returns the collected check outputs on success, or
    /// the first node error encountered.
    pub async fn run(
        &self,
        project_root: PathBuf,
        config: serde_json::Value,
    ) -> Result<RunResult, RunError> {
        let mut probe_outputs: HashMap<ProbeId, ProbeOutput> = HashMap::new();
        let mut check_outputs: Vec<(String, CheckOutput)> = Vec::new();

        for node in &self.execution_order {
            match node {
                NodeId::Probe(name) => {
                    let probe = self.probes.get(name).expect("validated in build");
                    let manifest = probe.manifest();

                    let dep_outputs: HashMap<ProbeId, ProbeOutput> = manifest
                        .dependencies
                        .iter()
                        .filter_map(|dep_name| {
                            let id = ProbeId(dep_name.clone());
                            probe_outputs.get(&id).map(|out| (id, out.clone()))
                        })
                        .collect();

                    let ctx =
                        ProbeContext::new(project_root.clone(), config.clone(), dep_outputs);

                    let output =
                        probe
                            .run_dyn(&ctx)
                            .await
                            .map_err(|source| RunError::ProbeFailed {
                                name: name.clone(),
                                source,
                            })?;

                    probe_outputs.insert(ProbeId(name.clone()), output);
                }
                NodeId::Check(name) => {
                    let check = self.checks.get(name).expect("validated in build");
                    let manifest = check.manifest();

                    let check_probe_outputs: HashMap<ProbeId, ProbeOutput> = manifest
                        .dependencies
                        .iter()
                        .filter_map(|dep_name| {
                            let id = ProbeId(dep_name.clone());
                            probe_outputs.get(&id).map(|out| (id, out.clone()))
                        })
                        .collect();

                    let ctx = CheckContext::new(
                        project_root.clone(),
                        config.clone(),
                        check_probe_outputs,
                    );

                    let output =
                        check
                            .run_dyn(&ctx)
                            .await
                            .map_err(|source| RunError::CheckFailed {
                                name: name.clone(),
                                source,
                            })?;

                    check_outputs.push((name.clone(), output));
                }
            }
        }

        Ok(RunResult { check_outputs })
    }
}

// ---------------------------------------------------------------------------
// Topological sort (DFS post-order on dependency edges)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum VisitState {
    Unvisited,
    InProgress,
    Done,
}

/// DFS-based topological sort following dependency edges.
///
/// Visiting a node means first recursively visiting all its dependencies,
/// then appending the node. This naturally yields an order where every
/// dependency precedes its dependents — no reversal needed.
///
/// Nodes and adjacency lists are iterated in sorted order so the output is
/// deterministic regardless of insertion order.
fn topological_sort(
    nodes: &[NodeId],
    deps: &HashMap<NodeId, Vec<NodeId>>,
) -> Result<Vec<NodeId>, ScheduleError> {
    let mut state: HashMap<NodeId, VisitState> =
        nodes.iter().map(|n| (n.clone(), VisitState::Unvisited)).collect();
    let mut result = Vec::with_capacity(nodes.len());
    let mut path: Vec<NodeId> = Vec::new();

    let mut sorted_nodes = nodes.to_vec();
    sorted_nodes.sort();

    for start in &sorted_nodes {
        if state.get(start) == Some(&VisitState::Unvisited) {
            visit(start, deps, &mut state, &mut result, &mut path)?;
        }
    }

    Ok(result)
}

fn visit(
    node: &NodeId,
    deps: &HashMap<NodeId, Vec<NodeId>>,
    state: &mut HashMap<NodeId, VisitState>,
    result: &mut Vec<NodeId>,
    path: &mut Vec<NodeId>,
) -> Result<(), ScheduleError> {
    state.insert(node.clone(), VisitState::InProgress);
    path.push(node.clone());

    if let Some(node_deps) = deps.get(node) {
        let mut sorted_deps = node_deps.clone();
        sorted_deps.sort();

        for dep in &sorted_deps {
            match state.get(dep) {
                Some(&VisitState::InProgress) => {
                    let start_idx = path.iter().position(|n| n == dep).unwrap();
                    let mut cycle: Vec<String> =
                        path[start_idx..].iter().map(|n| n.to_string()).collect();
                    cycle.push(dep.to_string());
                    return Err(ScheduleError::Cycle { cycle });
                }
                Some(&VisitState::Unvisited) => {
                    visit(dep, deps, state, result, path)?;
                }
                _ => {}
            }
        }
    }

    path.pop();
    state.insert(node.clone(), VisitState::Done);
    result.push(node.clone());
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use qualitool_protocol::finding::{Finding, FindingId, Severity};

    // -- Test helpers -------------------------------------------------------

    /// A probe that returns a fixed JSON value.
    struct TestProbe {
        manifest: ProbeManifest,
        output: serde_json::Value,
    }

    impl TestProbe {
        fn new(name: &str, deps: &[&str], output: serde_json::Value) -> Self {
            Self {
                manifest: ProbeManifest {
                    name: name.into(),
                    version: "0.1.0".into(),
                    description: None,
                    input_schema: None,
                    output_schema: None,
                    dependencies: deps.iter().map(|&s| s.into()).collect(),
                    contains_source_code: false,
                },
                output,
            }
        }
    }

    impl Probe for TestProbe {
        fn manifest(&self) -> &ProbeManifest {
            &self.manifest
        }

        async fn run(&self, _ctx: &ProbeContext) -> Result<ProbeOutput, ProbeError> {
            Ok(ProbeOutput(self.output.clone()))
        }
    }

    /// A probe that reads an upstream dependency and transforms it.
    struct DependentProbe {
        manifest: ProbeManifest,
        upstream: String,
    }

    impl DependentProbe {
        fn new(name: &str, upstream: &str) -> Self {
            Self {
                manifest: ProbeManifest {
                    name: name.into(),
                    version: "0.1.0".into(),
                    description: None,
                    input_schema: None,
                    output_schema: None,
                    dependencies: vec![upstream.into()],
                    contains_source_code: false,
                },
                upstream: upstream.into(),
            }
        }
    }

    impl Probe for DependentProbe {
        fn manifest(&self) -> &ProbeManifest {
            &self.manifest
        }

        async fn run(&self, ctx: &ProbeContext) -> Result<ProbeOutput, ProbeError> {
            let upstream = ctx
                .dependency_output(&ProbeId(self.upstream.clone()))
                .ok_or_else(|| ProbeError::MissingDependency {
                    probe_id: self.upstream.clone(),
                })?;
            let val = upstream.0["value"].as_u64().unwrap_or(0);
            Ok(ProbeOutput(serde_json::json!({"value": val * 2})))
        }
    }

    /// A check that returns empty findings.
    struct TestCheck {
        manifest: CheckManifest,
    }

    impl TestCheck {
        fn new(name: &str, deps: &[&str]) -> Self {
            Self {
                manifest: CheckManifest {
                    name: name.into(),
                    version: "0.1.0".into(),
                    description: None,
                    input_schema: None,
                    output_schema: None,
                    dependencies: deps.iter().map(|&s| s.into()).collect(),
                },
            }
        }
    }

    impl Check for TestCheck {
        fn manifest(&self) -> &CheckManifest {
            &self.manifest
        }

        async fn run(&self, _ctx: &CheckContext) -> Result<CheckOutput, CheckError> {
            Ok(CheckOutput::Findings { findings: vec![] })
        }
    }

    /// A check that produces a finding based on a probe output value.
    struct VerifyingCheck {
        manifest: CheckManifest,
        probe_name: String,
    }

    impl VerifyingCheck {
        fn new(name: &str, probe_dep: &str) -> Self {
            Self {
                manifest: CheckManifest {
                    name: name.into(),
                    version: "0.1.0".into(),
                    description: None,
                    input_schema: None,
                    output_schema: None,
                    dependencies: vec![probe_dep.into()],
                },
                probe_name: probe_dep.into(),
            }
        }
    }

    impl Check for VerifyingCheck {
        fn manifest(&self) -> &CheckManifest {
            &self.manifest
        }

        async fn run(&self, ctx: &CheckContext) -> Result<CheckOutput, CheckError> {
            let probe_out = ctx
                .probe_output(&ProbeId(self.probe_name.clone()))
                .ok_or_else(|| CheckError::MissingProbeOutput {
                    probe_id: self.probe_name.clone(),
                })?;
            let val = probe_out.0["value"].as_u64().unwrap_or(0);
            Ok(CheckOutput::Findings {
                findings: vec![Finding {
                    id: FindingId(format!("{}-1", self.manifest.name)),
                    check_id: self.manifest.name.clone(),
                    severity: Severity::Info,
                    title: format!("saw value {val}"),
                    summary: format!("probe {} reported value={val}", self.probe_name),
                    location: None,
                    tags: vec![],
                    payload: serde_json::json!({"value": val}),
                }],
            })
        }
    }

    /// A probe that always fails.
    struct FailingProbe {
        manifest: ProbeManifest,
    }

    impl FailingProbe {
        fn new(name: &str) -> Self {
            Self {
                manifest: ProbeManifest {
                    name: name.into(),
                    version: "0.1.0".into(),
                    description: None,
                    input_schema: None,
                    output_schema: None,
                    dependencies: vec![],
                    contains_source_code: false,
                },
            }
        }
    }

    impl Probe for FailingProbe {
        fn manifest(&self) -> &ProbeManifest {
            &self.manifest
        }

        async fn run(&self, _ctx: &ProbeContext) -> Result<ProbeOutput, ProbeError> {
            Err(ProbeError::ExecutionFailed {
                message: "intentional failure".into(),
                source: None,
            })
        }
    }

    /// A check that always fails.
    struct FailingCheck {
        manifest: CheckManifest,
    }

    impl FailingCheck {
        fn new(name: &str) -> Self {
            Self {
                manifest: CheckManifest {
                    name: name.into(),
                    version: "0.1.0".into(),
                    description: None,
                    input_schema: None,
                    output_schema: None,
                    dependencies: vec![],
                },
            }
        }
    }

    impl Check for FailingCheck {
        fn manifest(&self) -> &CheckManifest {
            &self.manifest
        }

        async fn run(&self, _ctx: &CheckContext) -> Result<CheckOutput, CheckError> {
            Err(CheckError::ExecutionFailed {
                message: "intentional failure".into(),
                source: None,
            })
        }
    }

    fn project_root() -> PathBuf {
        PathBuf::from("/tmp/test-project")
    }

    fn empty_config() -> serde_json::Value {
        serde_json::json!({})
    }

    // -- Schedule construction tests ----------------------------------------

    #[test]
    fn empty_scheduler_builds_successfully() {
        let scheduler = Scheduler::builder().build().unwrap();
        assert!(scheduler.execution_order().is_empty());
    }

    #[test]
    fn single_probe_execution_order() {
        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({})))
            .build()
            .unwrap();

        assert_eq!(scheduler.execution_order(), &[NodeId::Probe("p1".into())]);
    }

    #[test]
    fn single_check_no_deps_execution_order() {
        let scheduler = Scheduler::builder()
            .add_check(TestCheck::new("c1", &[]))
            .build()
            .unwrap();

        assert_eq!(scheduler.execution_order(), &[NodeId::Check("c1".into())]);
    }

    #[test]
    fn probe_chain_topological_order() {
        // p-a depends on p-b, p-b depends on p-c → order: p-c, p-b, p-a
        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p-a", &["p-b"], serde_json::json!({})))
            .add_probe(TestProbe::new("p-b", &["p-c"], serde_json::json!({})))
            .add_probe(TestProbe::new("p-c", &[], serde_json::json!({})))
            .build()
            .unwrap();

        assert_eq!(
            scheduler.execution_order(),
            &[
                NodeId::Probe("p-c".into()),
                NodeId::Probe("p-b".into()),
                NodeId::Probe("p-a".into()),
            ]
        );
    }

    #[test]
    fn checks_follow_probe_dependencies() {
        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({})))
            .add_check(TestCheck::new("c1", &["p1"]))
            .build()
            .unwrap();

        assert_eq!(
            scheduler.execution_order(),
            &[
                NodeId::Probe("p1".into()),
                NodeId::Check("c1".into()),
            ]
        );
    }

    #[test]
    fn deterministic_ordering_under_ties() {
        // Multiple independent nodes — order must be stable across runs.
        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("z-probe", &[], serde_json::json!({})))
            .add_probe(TestProbe::new("a-probe", &[], serde_json::json!({})))
            .add_probe(TestProbe::new("m-probe", &[], serde_json::json!({})))
            .add_check(TestCheck::new("z-check", &[]))
            .add_check(TestCheck::new("a-check", &[]))
            .build()
            .unwrap();

        let order = scheduler.execution_order().to_vec();

        // Probes before checks (NodeId::Probe < NodeId::Check), alpha within.
        assert_eq!(
            order,
            vec![
                NodeId::Probe("a-probe".into()),
                NodeId::Probe("m-probe".into()),
                NodeId::Probe("z-probe".into()),
                NodeId::Check("a-check".into()),
                NodeId::Check("z-check".into()),
            ]
        );

        // Run it again — must be identical.
        let scheduler2 = Scheduler::builder()
            .add_check(TestCheck::new("z-check", &[]))
            .add_probe(TestProbe::new("m-probe", &[], serde_json::json!({})))
            .add_check(TestCheck::new("a-check", &[]))
            .add_probe(TestProbe::new("z-probe", &[], serde_json::json!({})))
            .add_probe(TestProbe::new("a-probe", &[], serde_json::json!({})))
            .build()
            .unwrap();

        assert_eq!(scheduler2.execution_order(), &order);
    }

    // -- Cycle detection tests ----------------------------------------------

    #[test]
    fn direct_cycle_detected() {
        let err = Scheduler::builder()
            .add_probe(TestProbe::new("p-a", &["p-b"], serde_json::json!({})))
            .add_probe(TestProbe::new("p-b", &["p-a"], serde_json::json!({})))
            .build()
            .unwrap_err();

        match err {
            ScheduleError::Cycle { cycle } => {
                assert!(cycle.len() >= 3, "cycle path too short: {cycle:?}");
                assert_eq!(cycle.first(), cycle.last(), "cycle must close: {cycle:?}");
            }
            other => panic!("expected Cycle, got: {other}"),
        }
    }

    #[test]
    fn transitive_cycle_detected() {
        let err = Scheduler::builder()
            .add_probe(TestProbe::new("p-a", &["p-b"], serde_json::json!({})))
            .add_probe(TestProbe::new("p-b", &["p-c"], serde_json::json!({})))
            .add_probe(TestProbe::new("p-c", &["p-a"], serde_json::json!({})))
            .build()
            .unwrap_err();

        match err {
            ScheduleError::Cycle { cycle } => {
                assert!(cycle.len() >= 4, "cycle path too short: {cycle:?}");
                assert_eq!(cycle.first(), cycle.last(), "cycle must close: {cycle:?}");
            }
            other => panic!("expected Cycle, got: {other}"),
        }
    }

    #[test]
    fn self_cycle_detected() {
        let err = Scheduler::builder()
            .add_probe(TestProbe::new("p-a", &["p-a"], serde_json::json!({})))
            .build()
            .unwrap_err();

        match err {
            ScheduleError::Cycle { cycle } => {
                assert_eq!(cycle, vec!["probe:p-a", "probe:p-a"]);
            }
            other => panic!("expected Cycle, got: {other}"),
        }
    }

    // -- Dependency validation tests ----------------------------------------

    #[test]
    fn unknown_probe_dependency_error() {
        let err = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &["nonexistent"], serde_json::json!({})))
            .build()
            .unwrap_err();

        match err {
            ScheduleError::UnknownDependency { node, dependency } => {
                assert!(node.contains("p1"));
                assert!(dependency.contains("nonexistent"));
            }
            other => panic!("expected UnknownDependency, got: {other}"),
        }
    }

    #[test]
    fn unknown_check_dependency_error() {
        let err = Scheduler::builder()
            .add_check(TestCheck::new("c1", &["nonexistent"]))
            .build()
            .unwrap_err();

        match err {
            ScheduleError::UnknownDependency { node, dependency } => {
                assert!(node.contains("c1"));
                assert!(dependency.contains("nonexistent"));
            }
            other => panic!("expected UnknownDependency, got: {other}"),
        }
    }

    #[test]
    fn duplicate_probe_name_error() {
        let err = Scheduler::builder()
            .add_probe(TestProbe::new("dup", &[], serde_json::json!({})))
            .add_probe(TestProbe::new("dup", &[], serde_json::json!({})))
            .build()
            .unwrap_err();

        match err {
            ScheduleError::DuplicateNode { name } => assert_eq!(name, "dup"),
            other => panic!("expected DuplicateNode, got: {other}"),
        }
    }

    #[test]
    fn duplicate_check_name_error() {
        let err = Scheduler::builder()
            .add_check(TestCheck::new("dup", &[]))
            .add_check(TestCheck::new("dup", &[]))
            .build()
            .unwrap_err();

        match err {
            ScheduleError::DuplicateNode { name } => assert_eq!(name, "dup"),
            other => panic!("expected DuplicateNode, got: {other}"),
        }
    }

    #[test]
    fn probe_and_check_same_name_is_allowed() {
        // Different namespaces — no conflict.
        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("foo", &[], serde_json::json!({})))
            .add_check(TestCheck::new("foo", &["foo"]))
            .build()
            .unwrap();

        assert_eq!(
            scheduler.execution_order(),
            &[
                NodeId::Probe("foo".into()),
                NodeId::Check("foo".into()),
            ]
        );
    }

    // -- Execution tests ----------------------------------------------------

    #[tokio::test]
    async fn empty_scheduler_runs_to_empty_result() {
        let scheduler = Scheduler::builder().build().unwrap();
        let result = scheduler.run(project_root(), empty_config()).await.unwrap();
        assert!(result.check_outputs.is_empty());
    }

    #[tokio::test]
    async fn single_probe_executes() {
        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({"v": 1})))
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();
        // No checks → no check outputs, but execution should succeed.
        assert!(result.check_outputs.is_empty());
    }

    #[tokio::test]
    async fn check_receives_probe_output() {
        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({"value": 42})))
            .add_check(VerifyingCheck::new("c1", "p1"))
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();
        assert_eq!(result.check_outputs.len(), 1);
        let (name, output) = &result.check_outputs[0];
        assert_eq!(name, "c1");

        match output {
            CheckOutput::Findings { findings } => {
                assert_eq!(findings.len(), 1);
                assert!(findings[0].title.contains("42"));
            }
            _ => panic!("expected Findings"),
        }
    }

    #[tokio::test]
    async fn probe_output_memoized_for_downstream_probe() {
        // p-root produces {"value": 10}, p-child doubles it.
        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p-root", &[], serde_json::json!({"value": 10})))
            .add_probe(DependentProbe::new("p-child", "p-root"))
            .add_check(VerifyingCheck::new("c1", "p-child"))
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();
        assert_eq!(result.check_outputs.len(), 1);

        match &result.check_outputs[0].1 {
            CheckOutput::Findings { findings } => {
                assert_eq!(findings[0].payload["value"], 20);
            }
            _ => panic!("expected Findings"),
        }
    }

    #[tokio::test]
    async fn probe_failure_propagates_as_run_error() {
        let scheduler = Scheduler::builder()
            .add_probe(FailingProbe::new("bad"))
            .build()
            .unwrap();

        let err = scheduler.run(project_root(), empty_config()).await.unwrap_err();
        match err {
            RunError::ProbeFailed { name, .. } => assert_eq!(name, "bad"),
            other => panic!("expected ProbeFailed, got: {other}"),
        }
    }

    #[tokio::test]
    async fn check_failure_propagates_as_run_error() {
        let scheduler = Scheduler::builder()
            .add_check(FailingCheck::new("bad"))
            .build()
            .unwrap();

        let err = scheduler.run(project_root(), empty_config()).await.unwrap_err();
        match err {
            RunError::CheckFailed { name, .. } => assert_eq!(name, "bad"),
            other => panic!("expected CheckFailed, got: {other}"),
        }
    }

    // -- Integration: 3-probe, 5-check DAG ----------------------------------

    #[tokio::test]
    async fn three_probe_five_check_dag() {
        // Probes:
        //   file-tree (root)       → {"files": 100}
        //   git-history (root)     → {"commits": 50}
        //   dep-graph (→ file-tree) → doubles file-tree's "value"
        //
        // Checks:
        //   file-count   → depends on file-tree
        //   churn        → depends on file-tree, git-history
        //   complexity   → depends on file-tree
        //   dep-health   → depends on dep-graph
        //   overview     → depends on file-tree, git-history

        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new(
                "file-tree",
                &[],
                serde_json::json!({"value": 100}),
            ))
            .add_probe(TestProbe::new(
                "git-history",
                &[],
                serde_json::json!({"value": 50}),
            ))
            .add_probe(DependentProbe::new("dep-graph", "file-tree"))
            .add_check(VerifyingCheck::new("file-count", "file-tree"))
            .add_check(TestCheck::new("churn", &["file-tree", "git-history"]))
            .add_check(TestCheck::new("complexity", &["file-tree"]))
            .add_check(VerifyingCheck::new("dep-health", "dep-graph"))
            .add_check(TestCheck::new("overview", &["file-tree", "git-history"]))
            .build()
            .unwrap();

        // Verify execution order is deterministic and valid.
        let order = scheduler.execution_order();
        assert_eq!(order.len(), 8);

        // All probes must come before any check that depends on them.
        let pos = |id: &NodeId| order.iter().position(|n| n == id).unwrap();
        assert!(pos(&NodeId::Probe("file-tree".into())) < pos(&NodeId::Probe("dep-graph".into())));
        assert!(pos(&NodeId::Probe("file-tree".into())) < pos(&NodeId::Check("file-count".into())));
        assert!(pos(&NodeId::Probe("file-tree".into())) < pos(&NodeId::Check("churn".into())));
        assert!(pos(&NodeId::Probe("git-history".into())) < pos(&NodeId::Check("churn".into())));
        assert!(pos(&NodeId::Probe("dep-graph".into())) < pos(&NodeId::Check("dep-health".into())));

        // Execute and verify results.
        let result = scheduler.run(project_root(), empty_config()).await.unwrap();
        assert_eq!(result.check_outputs.len(), 5);

        // file-count should see file-tree's value=100
        let fc = result.check_outputs.iter().find(|(n, _)| n == "file-count").unwrap();
        match &fc.1 {
            CheckOutput::Findings { findings } => {
                assert_eq!(findings[0].payload["value"], 100);
            }
            _ => panic!("expected Findings"),
        }

        // dep-health should see dep-graph's value=200 (doubled from 100)
        let dh = result.check_outputs.iter().find(|(n, _)| n == "dep-health").unwrap();
        match &dh.1 {
            CheckOutput::Findings { findings } => {
                assert_eq!(findings[0].payload["value"], 200);
            }
            _ => panic!("expected Findings"),
        }
    }
}
