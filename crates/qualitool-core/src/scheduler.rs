use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use qualitool_protocol::agent::AgentRequest;
use qualitool_protocol::check::CheckOutput;
use qualitool_protocol::finding::Finding;
use qualitool_protocol::manifest::{CheckManifest, ProbeManifest};

use crate::agent::{AgentError, AgentRouter};
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

/// Per-node error indicating why a single node in the DAG failed.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// A dependency of this node failed, so this node was skipped.
    #[error("dependency '{upstream}' failed; node was skipped")]
    DependencyFailed { upstream: NodeId },

    /// The probe's `run` method returned an error.
    #[error("probe execution failed")]
    ProbeFailed {
        #[source]
        source: ProbeError,
    },

    /// The check's `run` method returned an error.
    #[error("check execution failed")]
    CheckFailed {
        #[source]
        source: CheckError,
    },

    /// The check emitted `CallAgent` but the agent call failed.
    #[error("agent call failed for check '{check}'")]
    AgentCallFailed {
        check: String,
        #[source]
        source: AgentError,
    },
}

/// Errors that prevent the scheduler itself from completing.
///
/// Individual node failures are reported in [`RunResult::failures`], not here.
/// `RunError` is reserved for infrastructure-level failures (e.g. a spawned
/// task panicking).
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// A spawned task panicked or was cancelled.
    #[error("task for node '{node}' panicked: {message}")]
    TaskPanicked { node: String, message: String },
}

/// The output of a schedule execution.
///
/// The scheduler always runs to completion — independent DAG branches continue
/// even when other branches fail. Successful check outputs and per-node
/// failures are both collected here.
#[derive(Debug)]
pub struct RunResult {
    /// Successful check outputs, keyed by check name.
    pub check_outputs: Vec<(String, CheckOutput)>,

    /// Nodes that failed, either directly or because a dependency failed.
    pub failures: Vec<(NodeId, NodeError)>,
}

/// Configuration for the parallel scheduler.
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Maximum number of nodes to execute concurrently.
    /// Defaults to `num_cpus - 1` (minimum 1).
    pub max_parallelism: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        Self {
            max_parallelism: cpus.saturating_sub(1).max(1),
        }
    }
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

trait DynAgentRouter: Send + Sync {
    fn route_dyn<'a>(
        &'a self,
        request: &'a AgentRequest,
        probe_outputs: &'a HashMap<String, serde_json::Value>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Finding>, AgentError>> + Send + 'a>>;
}

impl<T: AgentRouter> DynAgentRouter for T {
    fn route_dyn<'a>(
        &'a self,
        request: &'a AgentRequest,
        probe_outputs: &'a HashMap<String, serde_json::Value>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Finding>, AgentError>> + Send + 'a>> {
        Box::pin(self.route(request, probe_outputs))
    }
}

// ---------------------------------------------------------------------------
// SchedulerBuilder
// ---------------------------------------------------------------------------

/// Fluent builder for constructing a validated [`Scheduler`].
pub struct SchedulerBuilder {
    probes: Vec<Box<dyn DynProbe>>,
    checks: Vec<Box<dyn DynCheck>>,
    agent_router: Option<Arc<dyn DynAgentRouter>>,
}

impl SchedulerBuilder {
    pub fn new() -> Self {
        Self {
            probes: Vec::new(),
            checks: Vec::new(),
            agent_router: None,
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

    /// Set the agent router for handling `CallAgent` effects from checks.
    ///
    /// If not set and a check emits `CallAgent`, the scheduler will record
    /// a [`NodeError::AgentCallFailed`] with [`AgentError::NoRouter`].
    pub fn set_agent_router(mut self, router: impl AgentRouter + 'static) -> Self {
        self.agent_router = Some(Arc::new(router));
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

        let probes: HashMap<String, Arc<dyn DynProbe>> = self
            .probes
            .into_iter()
            .map(|p| {
                let name = p.manifest().name.clone();
                (name, Arc::from(p) as Arc<dyn DynProbe>)
            })
            .collect();
        let checks: HashMap<String, Arc<dyn DynCheck>> = self
            .checks
            .into_iter()
            .map(|c| {
                let name = c.manifest().name.clone();
                (name, Arc::from(c) as Arc<dyn DynCheck>)
            })
            .collect();

        Ok(Scheduler {
            execution_order,
            probes,
            checks,
            deps,
            agent_router: self.agent_router,
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
/// execute all nodes sequentially in dependency order, or
/// [`run_parallel`](Scheduler::run_parallel) for bounded-concurrency execution.
pub struct Scheduler {
    execution_order: Vec<NodeId>,
    probes: HashMap<String, Arc<dyn DynProbe>>,
    checks: HashMap<String, Arc<dyn DynCheck>>,
    /// Forward dependencies: node → [nodes it depends on].
    deps: HashMap<NodeId, Vec<NodeId>>,
    /// Optional agent router for handling `CallAgent` effects.
    agent_router: Option<Arc<dyn DynAgentRouter>>,
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

    /// Execute nodes in parallel up to `config.max_parallelism`.
    ///
    /// Dispatches eligible nodes (all dependencies satisfied) concurrently,
    /// bounded by a semaphore. Probe outputs are shared safely across tasks.
    /// Independent branches continue executing even when other branches fail.
    /// Per-node failures are collected in [`RunResult::failures`].
    /// Dropping the returned future cancels all in-flight tasks.
    pub async fn run_parallel(
        &self,
        project_root: PathBuf,
        config: serde_json::Value,
        scheduler_config: &SchedulerConfig,
    ) -> Result<RunResult, RunError> {
        if self.execution_order.is_empty() {
            return Ok(RunResult {
                check_outputs: vec![],
                failures: vec![],
            });
        }

        let max_par = scheduler_config.max_parallelism.max(1);

        // Build in-degree map and reverse dependency map (owned NodeIds).
        let mut in_degree: HashMap<NodeId, usize> = HashMap::new();
        let mut reverse_deps: HashMap<NodeId, Vec<NodeId>> = HashMap::new();

        for node in &self.execution_order {
            in_degree.entry(node.clone()).or_insert(0);
            if let Some(node_deps) = self.deps.get(node) {
                *in_degree.entry(node.clone()).or_insert(0) = node_deps.len();
                for dep in node_deps {
                    reverse_deps
                        .entry(dep.clone())
                        .or_default()
                        .push(node.clone());
                }
            }
        }

        // Ready queue: nodes with in-degree 0.
        let mut ready: VecDeque<NodeId> = VecDeque::new();
        for node in &self.execution_order {
            if in_degree[node] == 0 {
                ready.push_back(node.clone());
            }
        }

        let probe_outputs: Arc<tokio::sync::Mutex<HashMap<ProbeId, ProbeOutput>>> =
            Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let check_outputs: Arc<tokio::sync::Mutex<Vec<(String, CheckOutput)>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let semaphore = Arc::new(Semaphore::new(max_par));

        let mut processed_count = 0;
        let total = self.execution_order.len();
        let mut failed_nodes: HashSet<NodeId> = HashSet::new();
        let mut failures: Vec<(NodeId, NodeError)> = Vec::new();

        /// Result of a single spawned node task.
        enum NodeOutcome {
            Ok(NodeId),
            Failed(NodeId, NodeError),
        }

        // JoinSet owns spawned tasks; dropping it cancels in-flight work.
        let mut in_flight: JoinSet<NodeOutcome> = JoinSet::new();

        loop {
            // Dispatch all currently-ready nodes.
            while let Some(node) = ready.pop_front() {
                // Check if any dependency has failed — skip immediately.
                if let Some(failed_dep) = self.first_failed_dependency(&node, &failed_nodes) {
                    failed_nodes.insert(node.clone());
                    failures.push((
                        node.clone(),
                        NodeError::DependencyFailed {
                            upstream: failed_dep,
                        },
                    ));
                    processed_count += 1;
                    // Promote dependents whose in-degree has reached 0.
                    if let Some(dependents) = reverse_deps.get(&node) {
                        for dependent in dependents {
                            let deg = in_degree.get_mut(dependent).expect("node in graph");
                            *deg -= 1;
                            if *deg == 0 {
                                ready.push_back(dependent.clone());
                            }
                        }
                    }
                    continue;
                }

                let sem = Arc::clone(&semaphore);
                let probe_out = Arc::clone(&probe_outputs);
                let check_out = Arc::clone(&check_outputs);
                let pr = project_root.clone();
                let cfg = config.clone();

                match node {
                    NodeId::Probe(ref name) => {
                        let probe = Arc::clone(
                            self.probes.get(name).expect("validated in build"),
                        );
                        let dep_names: Vec<String> =
                            probe.manifest().dependencies.clone();
                        let node_id = node.clone();
                        let name = name.clone();

                        in_flight.spawn(async move {
                            let _permit = sem
                                .acquire()
                                .await
                                .expect("semaphore should not be closed");

                            let dep_outputs = {
                                let outputs = probe_out.lock().await;
                                dep_names
                                    .iter()
                                    .filter_map(|dep_name| {
                                        let id = ProbeId(dep_name.clone());
                                        outputs.get(&id).map(|out| (id, out.clone()))
                                    })
                                    .collect()
                            };

                            let ctx = ProbeContext::new(pr, cfg, dep_outputs);
                            match probe.run_dyn(&ctx).await {
                                Ok(output) => {
                                    probe_out
                                        .lock()
                                        .await
                                        .insert(ProbeId(name), output);
                                    NodeOutcome::Ok(node_id)
                                }
                                Err(source) => {
                                    NodeOutcome::Failed(
                                        node_id,
                                        NodeError::ProbeFailed { source },
                                    )
                                }
                            }
                        });
                    }
                    NodeId::Check(ref name) => {
                        let check = Arc::clone(
                            self.checks.get(name).expect("validated in build"),
                        );
                        let dep_names: Vec<String> =
                            check.manifest().dependencies.clone();
                        let node_id = node.clone();
                        let name = name.clone();
                        let agent_router = self.agent_router.clone();

                        in_flight.spawn(async move {
                            let _permit = sem
                                .acquire()
                                .await
                                .expect("semaphore should not be closed");

                            let check_probe_outputs = {
                                let outputs = probe_out.lock().await;
                                dep_names
                                    .iter()
                                    .filter_map(|dep_name| {
                                        let id = ProbeId(dep_name.clone());
                                        outputs.get(&id).map(|out| (id, out.clone()))
                                    })
                                    .collect()
                            };

                            let ctx =
                                CheckContext::new(pr, cfg, check_probe_outputs);
                            match check.run_dyn(&ctx).await {
                                Ok(CheckOutput::Findings { findings }) => {
                                    check_out
                                        .lock()
                                        .await
                                        .push((name, CheckOutput::Findings { findings }));
                                    NodeOutcome::Ok(node_id)
                                }
                                Ok(CheckOutput::CallAgent { request }) => {
                                    let result = route_agent_call_parallel(
                                        &name,
                                        &request,
                                        &probe_out,
                                        agent_router.as_deref(),
                                    ).await;
                                    match result {
                                        Ok(findings) => {
                                            check_out
                                                .lock()
                                                .await
                                                .push((name, CheckOutput::Findings { findings }));
                                            NodeOutcome::Ok(node_id)
                                        }
                                        Err(error) => {
                                            NodeOutcome::Failed(node_id, error)
                                        }
                                    }
                                }
                                Err(source) => {
                                    NodeOutcome::Failed(
                                        node_id,
                                        NodeError::CheckFailed { source },
                                    )
                                }
                            }
                        });
                    }
                }
            }

            if processed_count == total {
                break;
            }

            // Wait for the next task to complete.
            let join_result = in_flight
                .join_next()
                .await
                .expect("in_flight should not be empty when processed < total");

            let outcome = join_result.map_err(|e| RunError::TaskPanicked {
                node: "unknown".into(),
                message: e.to_string(),
            })?;

            processed_count += 1;

            let finished_node = match outcome {
                NodeOutcome::Ok(node_id) => node_id,
                NodeOutcome::Failed(node_id, error) => {
                    failed_nodes.insert(node_id.clone());
                    failures.push((node_id.clone(), error));
                    node_id
                }
            };

            // Decrement in-degree for dependents of the finished node.
            if let Some(dependents) = reverse_deps.get(&finished_node) {
                for dependent in dependents {
                    let deg = in_degree.get_mut(dependent).expect("node in graph");
                    *deg -= 1;
                    if *deg == 0 {
                        ready.push_back(dependent.clone());
                    }
                }
            }
        }

        let check_outputs = Arc::try_unwrap(check_outputs)
            .expect("all tasks completed")
            .into_inner();

        Ok(RunResult {
            check_outputs,
            failures,
        })
    }

    /// Execute all nodes sequentially in topological order.
    ///
    /// Probe outputs are memoized and injected into downstream contexts
    /// automatically. Independent branches continue executing even when
    /// other branches fail. Per-node failures are collected in
    /// [`RunResult::failures`].
    pub async fn run(
        &self,
        project_root: PathBuf,
        config: serde_json::Value,
    ) -> Result<RunResult, RunError> {
        let mut probe_outputs: HashMap<ProbeId, ProbeOutput> = HashMap::new();
        let mut check_outputs: Vec<(String, CheckOutput)> = Vec::new();
        let mut failures: Vec<(NodeId, NodeError)> = Vec::new();
        let mut failed_nodes: HashSet<NodeId> = HashSet::new();

        for node in &self.execution_order {
            // Check if any dependency has failed — if so, skip this node.
            if let Some(failed_dep) = self.first_failed_dependency(node, &failed_nodes) {
                failed_nodes.insert(node.clone());
                failures.push((
                    node.clone(),
                    NodeError::DependencyFailed {
                        upstream: failed_dep,
                    },
                ));
                continue;
            }

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

                    match probe.run_dyn(&ctx).await {
                        Ok(output) => {
                            probe_outputs.insert(ProbeId(name.clone()), output);
                        }
                        Err(source) => {
                            failed_nodes.insert(node.clone());
                            failures.push((
                                node.clone(),
                                NodeError::ProbeFailed { source },
                            ));
                        }
                    }
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

                    match check.run_dyn(&ctx).await {
                        Ok(CheckOutput::Findings { findings }) => {
                            check_outputs.push((
                                name.clone(),
                                CheckOutput::Findings { findings },
                            ));
                        }
                        Ok(CheckOutput::CallAgent { request }) => {
                            match self.route_agent_call(
                                name,
                                &request,
                                &probe_outputs,
                            ).await {
                                Ok(findings) => {
                                    check_outputs.push((
                                        name.clone(),
                                        CheckOutput::Findings { findings },
                                    ));
                                }
                                Err(error) => {
                                    failures.push((node.clone(), error));
                                }
                            }
                        }
                        Err(source) => {
                            failed_nodes.insert(node.clone());
                            failures.push((
                                node.clone(),
                                NodeError::CheckFailed { source },
                            ));
                        }
                    }
                }
            }
        }

        Ok(RunResult {
            check_outputs,
            failures,
        })
    }

    /// Route a `CallAgent` effect through the configured agent router.
    ///
    /// Gathers probe outputs matching [`AgentRequest::include_probes`] and
    /// delegates to the router. Returns the resulting findings on success,
    /// or a [`NodeError::AgentCallFailed`] on failure.
    async fn route_agent_call(
        &self,
        check_name: &str,
        request: &AgentRequest,
        probe_outputs: &HashMap<ProbeId, ProbeOutput>,
    ) -> Result<Vec<Finding>, NodeError> {
        let router = self.agent_router.as_ref().ok_or_else(|| {
            NodeError::AgentCallFailed {
                check: check_name.to_string(),
                source: AgentError::NoRouter,
            }
        })?;

        let relevant_probes: HashMap<String, serde_json::Value> = request
            .include_probes
            .iter()
            .filter_map(|name| {
                let id = ProbeId(name.clone());
                probe_outputs
                    .get(&id)
                    .map(|out| (name.clone(), out.0.clone()))
            })
            .collect();

        router
            .route_dyn(request, &relevant_probes)
            .await
            .map_err(|source| NodeError::AgentCallFailed {
                check: check_name.to_string(),
                source,
            })
    }

    /// Returns the first failed dependency of a node, if any.
    fn first_failed_dependency(
        &self,
        node: &NodeId,
        failed_nodes: &HashSet<NodeId>,
    ) -> Option<NodeId> {
        self.deps
            .get(node)?
            .iter()
            .find(|dep| failed_nodes.contains(dep))
            .cloned()
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

/// Standalone agent routing for the parallel executor (runs inside spawned tasks).
async fn route_agent_call_parallel(
    check_name: &str,
    request: &AgentRequest,
    probe_outputs: &Arc<tokio::sync::Mutex<HashMap<ProbeId, ProbeOutput>>>,
    router: Option<&dyn DynAgentRouter>,
) -> Result<Vec<Finding>, NodeError> {
    let router = router.ok_or_else(|| NodeError::AgentCallFailed {
        check: check_name.to_string(),
        source: AgentError::NoRouter,
    })?;

    let relevant_probes: HashMap<String, serde_json::Value> = {
        let outputs = probe_outputs.lock().await;
        request
            .include_probes
            .iter()
            .filter_map(|name| {
                let id = ProbeId(name.clone());
                outputs.get(&id).map(|out| (name.clone(), out.0.clone()))
            })
            .collect()
    };

    router
        .route_dyn(request, &relevant_probes)
        .await
        .map_err(|source| NodeError::AgentCallFailed {
            check: check_name.to_string(),
            source,
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use qualitool_protocol::finding::{Finding, FindingId, Severity};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

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
        assert!(result.failures.is_empty());
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
    async fn probe_failure_reported_in_result() {
        let scheduler = Scheduler::builder()
            .add_probe(FailingProbe::new("bad"))
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();
        assert_eq!(result.failures.len(), 1);
        assert!(matches!(
            &result.failures[0],
            (NodeId::Probe(name), NodeError::ProbeFailed { .. }) if name == "bad"
        ));
    }

    #[tokio::test]
    async fn check_failure_reported_in_result() {
        let scheduler = Scheduler::builder()
            .add_check(FailingCheck::new("bad"))
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();
        assert_eq!(result.failures.len(), 1);
        assert!(matches!(
            &result.failures[0],
            (NodeId::Check(name), NodeError::CheckFailed { .. }) if name == "bad"
        ));
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

    // -- Parallel execution tests -------------------------------------------

    /// A probe that sleeps for a fixed duration and tracks concurrency.
    struct SlowProbe {
        manifest: ProbeManifest,
        output: serde_json::Value,
        delay: Duration,
        tracker: Arc<ConcurrencyTracker>,
    }

    /// Tracks the maximum number of concurrently-executing nodes.
    struct ConcurrencyTracker {
        current: AtomicUsize,
        peak: AtomicUsize,
    }

    impl ConcurrencyTracker {
        fn new() -> Self {
            Self {
                current: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
            }
        }

        fn enter(&self) {
            let prev = self.current.fetch_add(1, Ordering::SeqCst);
            let new = prev + 1;
            self.peak.fetch_max(new, Ordering::SeqCst);
        }

        fn exit(&self) {
            self.current.fetch_sub(1, Ordering::SeqCst);
        }

        fn peak(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }
    }

    impl SlowProbe {
        fn new(
            name: &str,
            deps: &[&str],
            output: serde_json::Value,
            delay: Duration,
            tracker: Arc<ConcurrencyTracker>,
        ) -> Self {
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
                delay,
                tracker,
            }
        }
    }

    impl Probe for SlowProbe {
        fn manifest(&self) -> &ProbeManifest {
            &self.manifest
        }

        async fn run(&self, _ctx: &ProbeContext) -> Result<ProbeOutput, ProbeError> {
            self.tracker.enter();
            tokio::time::sleep(self.delay).await;
            self.tracker.exit();
            Ok(ProbeOutput(self.output.clone()))
        }
    }

    /// A check that sleeps for a fixed duration and tracks concurrency.
    struct SlowCheck {
        manifest: CheckManifest,
        delay: Duration,
        tracker: Arc<ConcurrencyTracker>,
    }

    impl SlowCheck {
        fn new(
            name: &str,
            deps: &[&str],
            delay: Duration,
            tracker: Arc<ConcurrencyTracker>,
        ) -> Self {
            Self {
                manifest: CheckManifest {
                    name: name.into(),
                    version: "0.1.0".into(),
                    description: None,
                    input_schema: None,
                    output_schema: None,
                    dependencies: deps.iter().map(|&s| s.into()).collect(),
                },
                delay,
                tracker,
            }
        }
    }

    impl Check for SlowCheck {
        fn manifest(&self) -> &CheckManifest {
            &self.manifest
        }

        async fn run(&self, _ctx: &CheckContext) -> Result<CheckOutput, CheckError> {
            self.tracker.enter();
            tokio::time::sleep(self.delay).await;
            self.tracker.exit();
            Ok(CheckOutput::Findings { findings: vec![] })
        }
    }

    #[test]
    fn scheduler_config_default_max_parallelism() {
        let config = SchedulerConfig::default();
        // Default should be at least 1
        assert!(config.max_parallelism >= 1);
    }

    #[test]
    fn scheduler_config_custom_max_parallelism() {
        let config = SchedulerConfig { max_parallelism: 4 };
        assert_eq!(config.max_parallelism, 4);
    }

    #[tokio::test]
    async fn parallel_run_honors_max_parallelism() {
        // 4 independent probes each sleeping 50ms, max_parallelism=2.
        // Peak concurrency must not exceed 2.
        let tracker = Arc::new(ConcurrencyTracker::new());
        let delay = Duration::from_millis(50);

        let scheduler = Scheduler::builder()
            .add_probe(SlowProbe::new("p1", &[], serde_json::json!({}), delay, Arc::clone(&tracker)))
            .add_probe(SlowProbe::new("p2", &[], serde_json::json!({}), delay, Arc::clone(&tracker)))
            .add_probe(SlowProbe::new("p3", &[], serde_json::json!({}), delay, Arc::clone(&tracker)))
            .add_probe(SlowProbe::new("p4", &[], serde_json::json!({}), delay, Arc::clone(&tracker)))
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 2 };
        scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();

        assert!(tracker.peak() <= 2, "peak concurrency {} exceeded max_parallelism 2", tracker.peak());
        assert!(tracker.peak() >= 2, "expected peak concurrency of 2, got {}", tracker.peak());
    }

    #[tokio::test]
    async fn parallel_run_with_max_parallelism_1_is_sequential() {
        // With max_parallelism=1, peak concurrency must be exactly 1.
        let tracker = Arc::new(ConcurrencyTracker::new());
        let delay = Duration::from_millis(30);

        let scheduler = Scheduler::builder()
            .add_probe(SlowProbe::new("p1", &[], serde_json::json!({}), delay, Arc::clone(&tracker)))
            .add_probe(SlowProbe::new("p2", &[], serde_json::json!({}), delay, Arc::clone(&tracker)))
            .add_probe(SlowProbe::new("p3", &[], serde_json::json!({}), delay, Arc::clone(&tracker)))
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 1 };
        scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();

        assert_eq!(tracker.peak(), 1, "with max_parallelism=1, peak must be 1");
    }

    #[tokio::test]
    async fn parallel_run_respects_dependencies() {
        // p-root → p-child (dependency), both with delays.
        // p-child must not start until p-root completes.
        let tracker = Arc::new(ConcurrencyTracker::new());
        let delay = Duration::from_millis(50);

        let scheduler = Scheduler::builder()
            .add_probe(SlowProbe::new("p-root", &[], serde_json::json!({"value": 10}), delay, Arc::clone(&tracker)))
            .add_probe(DependentProbe::new("p-child", "p-root"))
            .add_check(VerifyingCheck::new("c1", "p-child"))
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 4 };
        let result = scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();

        // p-child should have doubled p-root's value
        match &result.check_outputs[0].1 {
            CheckOutput::Findings { findings } => {
                assert_eq!(findings[0].payload["value"], 20);
            }
            _ => panic!("expected Findings"),
        }
    }

    #[tokio::test]
    async fn parallel_run_shared_probe_cache_concurrent_reads() {
        // One probe, three checks all reading from it concurrently.
        let tracker = Arc::new(ConcurrencyTracker::new());
        let delay = Duration::from_millis(50);

        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("shared", &[], serde_json::json!({"value": 99})))
            .add_check(SlowCheck::new("c1", &["shared"], delay, Arc::clone(&tracker)))
            .add_check(SlowCheck::new("c2", &["shared"], delay, Arc::clone(&tracker)))
            .add_check(SlowCheck::new("c3", &["shared"], delay, Arc::clone(&tracker)))
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 3 };
        let result = scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();

        // All three checks should complete successfully
        assert_eq!(result.check_outputs.len(), 3);
        // Checks ran in parallel (peak should be >=2)
        assert!(tracker.peak() >= 2, "expected concurrent check execution, peak was {}", tracker.peak());
    }

    #[tokio::test]
    async fn parallel_run_speedup_over_sequential() {
        // 4 independent probes each sleeping 50ms.
        // Sequential: ~200ms. Parallel (max=4): ~50ms.
        let tracker = Arc::new(ConcurrencyTracker::new());
        let delay = Duration::from_millis(50);

        let scheduler = Scheduler::builder()
            .add_probe(SlowProbe::new("p1", &[], serde_json::json!({}), delay, Arc::clone(&tracker)))
            .add_probe(SlowProbe::new("p2", &[], serde_json::json!({}), delay, Arc::clone(&tracker)))
            .add_probe(SlowProbe::new("p3", &[], serde_json::json!({}), delay, Arc::clone(&tracker)))
            .add_probe(SlowProbe::new("p4", &[], serde_json::json!({}), delay, Arc::clone(&tracker)))
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 4 };
        let start = Instant::now();
        scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();
        let parallel_elapsed = start.elapsed();

        // Parallel should take significantly less than 4 * 50ms = 200ms.
        // Allow generous margin but ensure it's faster than sequential.
        assert!(
            parallel_elapsed < Duration::from_millis(150),
            "parallel execution took {:?}, expected under 150ms",
            parallel_elapsed,
        );
    }

    #[tokio::test]
    async fn parallel_run_probe_failure_reported_in_result() {
        let tracker = Arc::new(ConcurrencyTracker::new());
        let delay = Duration::from_millis(30);

        let scheduler = Scheduler::builder()
            .add_probe(FailingProbe::new("bad"))
            .add_probe(SlowProbe::new("good", &[], serde_json::json!({}), delay, Arc::clone(&tracker)))
            .add_check(TestCheck::new("c1", &["good"]))
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 4 };
        let result = scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();

        assert_eq!(result.failures.len(), 1);
        assert!(matches!(
            &result.failures[0],
            (NodeId::Probe(name), NodeError::ProbeFailed { .. }) if name == "bad"
        ));
        // The independent branch (good → c1) should still succeed.
        assert_eq!(result.check_outputs.len(), 1);
    }

    #[tokio::test]
    async fn parallel_run_check_failure_reported_in_result() {
        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({})))
            .add_check(FailingCheck::new("bad"))
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 4 };
        let result = scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();

        assert_eq!(result.failures.len(), 1);
        assert!(matches!(
            &result.failures[0],
            (NodeId::Check(name), NodeError::CheckFailed { .. }) if name == "bad"
        ));
    }

    #[tokio::test]
    async fn parallel_run_empty_scheduler() {
        let scheduler = Scheduler::builder().build().unwrap();
        let config = SchedulerConfig { max_parallelism: 4 };
        let result = scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();
        assert!(result.check_outputs.is_empty());
    }

    #[tokio::test]
    async fn parallel_run_diamond_dag() {
        // Diamond:  p-root → p-left, p-root → p-right, both → c-merge
        let tracker = Arc::new(ConcurrencyTracker::new());
        let delay = Duration::from_millis(50);

        let scheduler = Scheduler::builder()
            .add_probe(SlowProbe::new("p-root", &[], serde_json::json!({"value": 5}), delay, Arc::clone(&tracker)))
            .add_probe(SlowProbe::new("p-left", &["p-root"], serde_json::json!({"value": 10}), delay, Arc::clone(&tracker)))
            .add_probe(SlowProbe::new("p-right", &["p-root"], serde_json::json!({"value": 20}), delay, Arc::clone(&tracker)))
            .add_check(TestCheck::new("c-merge", &["p-left", "p-right"]))
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 4 };
        let start = Instant::now();
        let result = scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();
        let elapsed = start.elapsed();

        assert_eq!(result.check_outputs.len(), 1);
        assert!(result.failures.is_empty());
        // p-left and p-right should run in parallel after p-root.
        // Total: ~100ms (50ms for root + 50ms for parallel left/right), not 150ms.
        assert!(
            elapsed < Duration::from_millis(130),
            "diamond DAG took {:?}, expected under 130ms (parallel branches)",
            elapsed,
        );
    }

    // -- Failure propagation tests (QUA-29) -----------------------------------

    /// A probe that records whether it was actually executed.
    struct TrackedProbe {
        manifest: ProbeManifest,
        output: serde_json::Value,
        executed: Arc<std::sync::atomic::AtomicBool>,
    }

    impl TrackedProbe {
        fn new(name: &str, deps: &[&str], output: serde_json::Value) -> (Self, Arc<std::sync::atomic::AtomicBool>) {
            let executed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            (
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
                    executed: Arc::clone(&executed),
                },
                executed,
            )
        }
    }

    impl Probe for TrackedProbe {
        fn manifest(&self) -> &ProbeManifest {
            &self.manifest
        }

        async fn run(&self, _ctx: &ProbeContext) -> Result<ProbeOutput, ProbeError> {
            self.executed.store(true, Ordering::SeqCst);
            Ok(ProbeOutput(self.output.clone()))
        }
    }

    /// A check that records whether it was actually executed.
    struct TrackedCheck {
        manifest: CheckManifest,
        executed: Arc<std::sync::atomic::AtomicBool>,
    }

    impl TrackedCheck {
        fn new(name: &str, deps: &[&str]) -> (Self, Arc<std::sync::atomic::AtomicBool>) {
            let executed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            (
                Self {
                    manifest: CheckManifest {
                        name: name.into(),
                        version: "0.1.0".into(),
                        description: None,
                        input_schema: None,
                        output_schema: None,
                        dependencies: deps.iter().map(|&s| s.into()).collect(),
                    },
                    executed: Arc::clone(&executed),
                },
                executed,
            )
        }
    }

    impl Check for TrackedCheck {
        fn manifest(&self) -> &CheckManifest {
            &self.manifest
        }

        async fn run(&self, _ctx: &CheckContext) -> Result<CheckOutput, CheckError> {
            self.executed.store(true, Ordering::SeqCst);
            Ok(CheckOutput::Findings { findings: vec![] })
        }
    }

    #[tokio::test]
    async fn failure_propagation_one_hop_sequential() {
        // p-bad (fails) → p-child (depends on p-bad) → c1 (depends on p-child)
        // p-child and c1 should get DependencyFailed, not execute.
        let (p_child, child_ran) = TrackedProbe::new("p-child", &["p-bad"], serde_json::json!({}));
        let (c1, c1_ran) = TrackedCheck::new("c1", &["p-child"]);

        let scheduler = Scheduler::builder()
            .add_probe(FailingProbe::new("p-bad"))
            .add_probe(p_child)
            .add_check(c1)
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();

        assert!(!child_ran.load(Ordering::SeqCst), "p-child should not have executed");
        assert!(!c1_ran.load(Ordering::SeqCst), "c1 should not have executed");
        assert!(result.check_outputs.is_empty());

        // p-bad: ProbeFailed, p-child: DependencyFailed(p-bad), c1: DependencyFailed(p-child)
        assert_eq!(result.failures.len(), 3);

        let bad_failure = result.failures.iter().find(|(id, _)| *id == NodeId::Probe("p-bad".into()));
        assert!(matches!(bad_failure, Some((_, NodeError::ProbeFailed { .. }))));

        let child_failure = result.failures.iter().find(|(id, _)| *id == NodeId::Probe("p-child".into()));
        assert!(matches!(child_failure, Some((_, NodeError::DependencyFailed { upstream })) if *upstream == NodeId::Probe("p-bad".into())));

        let c1_failure = result.failures.iter().find(|(id, _)| *id == NodeId::Check("c1".into()));
        assert!(matches!(c1_failure, Some((_, NodeError::DependencyFailed { upstream })) if *upstream == NodeId::Probe("p-child".into())));
    }

    #[tokio::test]
    async fn failure_propagation_two_hops_sequential() {
        // p-root (fails) → p-mid → p-leaf
        // Both p-mid and p-leaf get DependencyFailed.
        let (p_mid, mid_ran) = TrackedProbe::new("p-mid", &["p-root"], serde_json::json!({}));
        let (p_leaf, leaf_ran) = TrackedProbe::new("p-leaf", &["p-mid"], serde_json::json!({}));

        let scheduler = Scheduler::builder()
            .add_probe(FailingProbe::new("p-root"))
            .add_probe(p_mid)
            .add_probe(p_leaf)
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();

        assert!(!mid_ran.load(Ordering::SeqCst));
        assert!(!leaf_ran.load(Ordering::SeqCst));
        assert_eq!(result.failures.len(), 3);

        // p-leaf should report p-mid as its failed upstream (not p-root).
        let leaf_failure = result.failures.iter().find(|(id, _)| *id == NodeId::Probe("p-leaf".into()));
        assert!(matches!(leaf_failure, Some((_, NodeError::DependencyFailed { upstream })) if *upstream == NodeId::Probe("p-mid".into())));
    }

    #[tokio::test]
    async fn sibling_branches_continue_after_failure_sequential() {
        // Two independent branches:
        //   Branch A: p-bad (fails) → c-a (skipped)
        //   Branch B: p-good (succeeds) → c-b (succeeds)
        // Branch B must complete despite Branch A failing.
        let (p_good, good_ran) = TrackedProbe::new("p-good", &[], serde_json::json!({"v": 1}));
        let (c_b, cb_ran) = TrackedCheck::new("c-b", &["p-good"]);
        let (c_a, ca_ran) = TrackedCheck::new("c-a", &["p-bad"]);

        let scheduler = Scheduler::builder()
            .add_probe(FailingProbe::new("p-bad"))
            .add_probe(p_good)
            .add_check(c_a)
            .add_check(c_b)
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();

        assert!(good_ran.load(Ordering::SeqCst), "p-good should have run");
        assert!(cb_ran.load(Ordering::SeqCst), "c-b should have run");
        assert!(!ca_ran.load(Ordering::SeqCst), "c-a should not have run");

        assert_eq!(result.check_outputs.len(), 1);
        assert_eq!(result.check_outputs[0].0, "c-b");

        // Failures: p-bad (ProbeFailed) + c-a (DependencyFailed)
        assert_eq!(result.failures.len(), 2);
    }

    #[tokio::test]
    async fn integration_failing_root_unrelated_branch_completes_sequential() {
        // Complex DAG:
        //   Branch A: p-fail (fails) → p-derived (skipped) → c-derived (skipped)
        //   Branch B: p-ok1 → c-ok1 (succeeds)
        //   Branch C: p-ok2 → p-ok3 (depends on p-ok2) → c-ok2 (depends on p-ok3)
        let (p_derived, derived_ran) = TrackedProbe::new("p-derived", &["p-fail"], serde_json::json!({}));
        let (c_derived, c_derived_ran) = TrackedCheck::new("c-derived", &["p-derived"]);

        let (p_ok1, ok1_ran) = TrackedProbe::new("p-ok1", &[], serde_json::json!({"v": 1}));
        let (c_ok1, c_ok1_ran) = TrackedCheck::new("c-ok1", &["p-ok1"]);

        let (p_ok2, ok2_ran) = TrackedProbe::new("p-ok2", &[], serde_json::json!({"value": 10}));
        let (p_ok3, ok3_ran) = TrackedProbe::new("p-ok3", &["p-ok2"], serde_json::json!({"value": 20}));
        let (c_ok2, c_ok2_ran) = TrackedCheck::new("c-ok2", &["p-ok3"]);

        let scheduler = Scheduler::builder()
            .add_probe(FailingProbe::new("p-fail"))
            .add_probe(p_derived)
            .add_check(c_derived)
            .add_probe(p_ok1)
            .add_check(c_ok1)
            .add_probe(p_ok2)
            .add_probe(p_ok3)
            .add_check(c_ok2)
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();

        // Failed branch
        assert!(!derived_ran.load(Ordering::SeqCst));
        assert!(!c_derived_ran.load(Ordering::SeqCst));

        // Successful branches
        assert!(ok1_ran.load(Ordering::SeqCst));
        assert!(c_ok1_ran.load(Ordering::SeqCst));
        assert!(ok2_ran.load(Ordering::SeqCst));
        assert!(ok3_ran.load(Ordering::SeqCst));
        assert!(c_ok2_ran.load(Ordering::SeqCst));

        assert_eq!(result.check_outputs.len(), 2);
        // Failures: p-fail + p-derived + c-derived = 3
        assert_eq!(result.failures.len(), 3);
    }

    #[tokio::test]
    async fn check_failure_does_not_cascade() {
        // A failing check should not affect other checks.
        // p1 → c-bad (fails), c-good (both depend on p1)
        let (c_good, c_good_ran) = TrackedCheck::new("c-good", &["p1"]);

        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({"v": 1})))
            .add_check(FailingCheck::new("c-bad"))
            .add_check(c_good)
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();

        assert!(c_good_ran.load(Ordering::SeqCst));
        assert_eq!(result.check_outputs.len(), 1);
        assert_eq!(result.check_outputs[0].0, "c-good");

        assert_eq!(result.failures.len(), 1);
        assert!(matches!(&result.failures[0], (NodeId::Check(n), NodeError::CheckFailed { .. }) if n == "c-bad"));
    }

    // -- Parallel failure propagation tests -----------------------------------

    #[tokio::test]
    async fn failure_propagation_one_hop_parallel() {
        let (p_child, child_ran) = TrackedProbe::new("p-child", &["p-bad"], serde_json::json!({}));
        let (c1, c1_ran) = TrackedCheck::new("c1", &["p-child"]);

        let scheduler = Scheduler::builder()
            .add_probe(FailingProbe::new("p-bad"))
            .add_probe(p_child)
            .add_check(c1)
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 4 };
        let result = scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();

        assert!(!child_ran.load(Ordering::SeqCst));
        assert!(!c1_ran.load(Ordering::SeqCst));
        assert_eq!(result.failures.len(), 3);
    }

    #[tokio::test]
    async fn sibling_branches_continue_after_failure_parallel() {
        let (p_good, good_ran) = TrackedProbe::new("p-good", &[], serde_json::json!({"v": 1}));
        let (c_b, cb_ran) = TrackedCheck::new("c-b", &["p-good"]);
        let (c_a, ca_ran) = TrackedCheck::new("c-a", &["p-bad"]);

        let scheduler = Scheduler::builder()
            .add_probe(FailingProbe::new("p-bad"))
            .add_probe(p_good)
            .add_check(c_a)
            .add_check(c_b)
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 4 };
        let result = scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();

        assert!(good_ran.load(Ordering::SeqCst));
        assert!(cb_ran.load(Ordering::SeqCst));
        assert!(!ca_ran.load(Ordering::SeqCst));

        assert_eq!(result.check_outputs.len(), 1);
        assert_eq!(result.failures.len(), 2);
    }

    // -- Agent effect loop helpers -------------------------------------------

    /// A check that emits `CallAgent` instead of `Findings`.
    struct AgentCallingCheck {
        manifest: CheckManifest,
        prompt: String,
        include_probes: Vec<String>,
    }

    impl AgentCallingCheck {
        fn new(name: &str, deps: &[&str], prompt: &str, include_probes: &[&str]) -> Self {
            Self {
                manifest: CheckManifest {
                    name: name.into(),
                    version: "0.1.0".into(),
                    description: None,
                    input_schema: None,
                    output_schema: None,
                    dependencies: deps.iter().map(|&s| s.into()).collect(),
                },
                prompt: prompt.into(),
                include_probes: include_probes.iter().map(|&s| s.into()).collect(),
            }
        }
    }

    impl Check for AgentCallingCheck {
        fn manifest(&self) -> &CheckManifest {
            &self.manifest
        }

        async fn run(&self, _ctx: &CheckContext) -> Result<CheckOutput, CheckError> {
            Ok(CheckOutput::CallAgent {
                request: AgentRequest {
                    agent_hint: None,
                    prompt: self.prompt.clone(),
                    include_probes: self.include_probes.clone(),
                    response_schema: serde_json::json!({"type": "object"}),
                    constraints: qualitool_protocol::agent::AgentConstraints {
                        max_tokens: Some(4000),
                        timeout_seconds: Some(60),
                        read_only: true,
                    },
                },
            })
        }
    }

    /// Test-double agent router that returns a fixed finding per call.
    struct TestAgentRouter {
        call_count: Arc<AtomicUsize>,
    }

    impl TestAgentRouter {
        fn new() -> (Self, Arc<AtomicUsize>) {
            let count = Arc::new(AtomicUsize::new(0));
            (Self { call_count: Arc::clone(&count) }, count)
        }
    }

    impl AgentRouter for TestAgentRouter {
        async fn route(
            &self,
            request: &AgentRequest,
            probe_outputs: &HashMap<String, serde_json::Value>,
        ) -> Result<Vec<Finding>, crate::agent::AgentError> {
            let call_num = self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(vec![Finding {
                id: FindingId(format!("agent-{call_num}")),
                check_id: format!("agent-check-{call_num}"),
                severity: Severity::Info,
                title: format!("Agent finding for: {}", request.prompt),
                summary: format!(
                    "Agent processed {} probes",
                    probe_outputs.len(),
                ),
                location: None,
                tags: vec![],
                payload: serde_json::json!({
                    "probe_keys": probe_outputs.keys().collect::<Vec<_>>(),
                }),
            }])
        }
    }

    /// Test-double agent router that always fails.
    struct FailingAgentRouter;

    impl AgentRouter for FailingAgentRouter {
        async fn route(
            &self,
            _request: &AgentRequest,
            _probe_outputs: &HashMap<String, serde_json::Value>,
        ) -> Result<Vec<Finding>, crate::agent::AgentError> {
            Err(crate::agent::AgentError::ExecutionFailed {
                message: "agent subprocess crashed".into(),
                source: None,
            })
        }
    }

    // -- Agent effect loop tests (sequential) ---------------------------------

    #[tokio::test]
    async fn call_agent_routed_through_agent_router_sequential() {
        let (router, call_count) = TestAgentRouter::new();

        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({"files": 42})))
            .add_check(AgentCallingCheck::new("ai-check", &["p1"], "analyze this", &["p1"]))
            .set_agent_router(router)
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert!(result.failures.is_empty());
        assert_eq!(result.check_outputs.len(), 1);

        let (name, output) = &result.check_outputs[0];
        assert_eq!(name, "ai-check");
        match output {
            CheckOutput::Findings { findings } => {
                assert_eq!(findings.len(), 1);
                assert!(findings[0].title.contains("analyze this"));
                // Verify probe data was passed through
                let keys = findings[0].payload["probe_keys"].as_array().unwrap();
                assert_eq!(keys.len(), 1);
                assert_eq!(keys[0], "p1");
            }
            CheckOutput::CallAgent { .. } => panic!("expected Findings after agent routing"),
        }
    }

    #[tokio::test]
    async fn multiple_agent_checks_sequential() {
        let (router, call_count) = TestAgentRouter::new();

        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({"a": 1})))
            .add_check(AgentCallingCheck::new("ai-1", &["p1"], "prompt-1", &["p1"]))
            .add_check(AgentCallingCheck::new("ai-2", &["p1"], "prompt-2", &["p1"]))
            .set_agent_router(router)
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();

        assert_eq!(call_count.load(Ordering::SeqCst), 2);
        assert!(result.failures.is_empty());
        assert_eq!(result.check_outputs.len(), 2);
    }

    #[tokio::test]
    async fn call_agent_without_router_fails_sequential() {
        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({})))
            .add_check(AgentCallingCheck::new("ai-check", &["p1"], "analyze", &["p1"]))
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();

        assert_eq!(result.failures.len(), 1);
        assert!(matches!(
            &result.failures[0],
            (NodeId::Check(n), NodeError::AgentCallFailed { source: AgentError::NoRouter, .. })
            if n == "ai-check"
        ));
        assert!(result.check_outputs.is_empty());
    }

    #[tokio::test]
    async fn agent_router_failure_reported_sequential() {
        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({})))
            .add_check(AgentCallingCheck::new("ai-check", &["p1"], "analyze", &["p1"]))
            .set_agent_router(FailingAgentRouter)
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();

        assert_eq!(result.failures.len(), 1);
        assert!(matches!(
            &result.failures[0],
            (NodeId::Check(n), NodeError::AgentCallFailed { check, .. })
            if n == "ai-check" && check == "ai-check"
        ));
    }

    #[tokio::test]
    async fn agent_check_receives_only_requested_probes() {
        let (router, _) = TestAgentRouter::new();

        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({"a": 1})))
            .add_probe(TestProbe::new("p2", &[], serde_json::json!({"b": 2})))
            // Check depends on both probes but only includes p1 in agent request
            .add_check(AgentCallingCheck::new("ai-check", &["p1", "p2"], "analyze", &["p1"]))
            .set_agent_router(router)
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();

        assert!(result.failures.is_empty());
        let (_, output) = &result.check_outputs[0];
        match output {
            CheckOutput::Findings { findings } => {
                let keys = findings[0].payload["probe_keys"].as_array().unwrap();
                assert_eq!(keys.len(), 1);
                assert_eq!(keys[0], "p1");
            }
            _ => panic!("expected Findings"),
        }
    }

    #[tokio::test]
    async fn mixed_findings_and_agent_checks_sequential() {
        let (router, call_count) = TestAgentRouter::new();

        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({"v": 1})))
            .add_check(TestCheck::new("plain-check", &["p1"]))
            .add_check(AgentCallingCheck::new("ai-check", &["p1"], "analyze", &["p1"]))
            .set_agent_router(router)
            .build()
            .unwrap();

        let result = scheduler.run(project_root(), empty_config()).await.unwrap();

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert!(result.failures.is_empty());
        assert_eq!(result.check_outputs.len(), 2);
    }

    // -- Agent effect loop tests (parallel) -----------------------------------

    #[tokio::test]
    async fn call_agent_routed_through_agent_router_parallel() {
        let (router, call_count) = TestAgentRouter::new();

        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({"files": 42})))
            .add_check(AgentCallingCheck::new("ai-check", &["p1"], "analyze this", &["p1"]))
            .set_agent_router(router)
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 4 };
        let result = scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert!(result.failures.is_empty());
        assert_eq!(result.check_outputs.len(), 1);

        let (name, output) = &result.check_outputs[0];
        assert_eq!(name, "ai-check");
        match output {
            CheckOutput::Findings { findings } => {
                assert_eq!(findings.len(), 1);
                assert!(findings[0].title.contains("analyze this"));
            }
            CheckOutput::CallAgent { .. } => panic!("expected Findings after agent routing"),
        }
    }

    #[tokio::test]
    async fn multiple_agent_checks_parallel() {
        let (router, call_count) = TestAgentRouter::new();

        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({"a": 1})))
            .add_check(AgentCallingCheck::new("ai-1", &["p1"], "prompt-1", &["p1"]))
            .add_check(AgentCallingCheck::new("ai-2", &["p1"], "prompt-2", &["p1"]))
            .set_agent_router(router)
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 4 };
        let result = scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();

        assert_eq!(call_count.load(Ordering::SeqCst), 2);
        assert!(result.failures.is_empty());
        assert_eq!(result.check_outputs.len(), 2);
    }

    #[tokio::test]
    async fn call_agent_without_router_fails_parallel() {
        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({})))
            .add_check(AgentCallingCheck::new("ai-check", &["p1"], "analyze", &["p1"]))
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 4 };
        let result = scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();

        assert_eq!(result.failures.len(), 1);
        assert!(matches!(
            &result.failures[0],
            (NodeId::Check(n), NodeError::AgentCallFailed { source: AgentError::NoRouter, .. })
            if n == "ai-check"
        ));
    }

    #[tokio::test]
    async fn agent_router_failure_reported_parallel() {
        let scheduler = Scheduler::builder()
            .add_probe(TestProbe::new("p1", &[], serde_json::json!({})))
            .add_check(AgentCallingCheck::new("ai-check", &["p1"], "analyze", &["p1"]))
            .set_agent_router(FailingAgentRouter)
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 4 };
        let result = scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();

        assert_eq!(result.failures.len(), 1);
        assert!(matches!(
            &result.failures[0],
            (NodeId::Check(n), NodeError::AgentCallFailed { check, .. })
            if n == "ai-check" && check == "ai-check"
        ));
    }

    #[tokio::test]
    async fn integration_failing_root_unrelated_branch_completes_parallel() {
        let (p_derived, derived_ran) = TrackedProbe::new("p-derived", &["p-fail"], serde_json::json!({}));
        let (c_derived, c_derived_ran) = TrackedCheck::new("c-derived", &["p-derived"]);

        let (p_ok1, ok1_ran) = TrackedProbe::new("p-ok1", &[], serde_json::json!({"v": 1}));
        let (c_ok1, c_ok1_ran) = TrackedCheck::new("c-ok1", &["p-ok1"]);

        let (p_ok2, ok2_ran) = TrackedProbe::new("p-ok2", &[], serde_json::json!({"value": 10}));
        let (p_ok3, ok3_ran) = TrackedProbe::new("p-ok3", &["p-ok2"], serde_json::json!({"value": 20}));
        let (c_ok2, c_ok2_ran) = TrackedCheck::new("c-ok2", &["p-ok3"]);

        let scheduler = Scheduler::builder()
            .add_probe(FailingProbe::new("p-fail"))
            .add_probe(p_derived)
            .add_check(c_derived)
            .add_probe(p_ok1)
            .add_check(c_ok1)
            .add_probe(p_ok2)
            .add_probe(p_ok3)
            .add_check(c_ok2)
            .build()
            .unwrap();

        let config = SchedulerConfig { max_parallelism: 4 };
        let result = scheduler.run_parallel(project_root(), empty_config(), &config).await.unwrap();

        assert!(!derived_ran.load(Ordering::SeqCst));
        assert!(!c_derived_ran.load(Ordering::SeqCst));

        assert!(ok1_ran.load(Ordering::SeqCst));
        assert!(c_ok1_ran.load(Ordering::SeqCst));
        assert!(ok2_ran.load(Ordering::SeqCst));
        assert!(ok3_ran.load(Ordering::SeqCst));
        assert!(c_ok2_ran.load(Ordering::SeqCst));

        assert_eq!(result.check_outputs.len(), 2);
        assert_eq!(result.failures.len(), 3);
    }
}
