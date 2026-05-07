//! Deep runtime interface for LATTICE.
//!
//! This crate concentrates the construction knowledge for model routing,
//! profile registries, tool/plugin registries, memory, events, and pipeline
//! execution. Lower crates remain available for extension, but application
//! entrypoints should prefer [`Runtime`] so wiring policy has one locality.

use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::agent::memory::Memory;
use crate::agent::tool_registry::ToolRegistry;
use crate::agent::LoopEvent;
use crate::bus::events::EventBus;
use crate::bus::{AgentRegistry, Bus, LatticeDir, MicroAgent, Pipeline, PipelineRun};
use crate::core::catalog::{ModelCatalogEntry, ResolvedModel};
use crate::core::router::ModelRouter;
use crate::core::streaming::StreamEvent;
use crate::core::types::{Message, ToolDefinition};
use crate::plugin::registry::PluginRegistry;
use futures::Stream;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Core(#[from] crate::core::LatticeError),
    #[error("runtime configuration error: {0}")]
    Config(String),
    #[error("agent assembly failed: {0}")]
    Assembly(String),
}

#[derive(Clone)]
pub struct RuntimeConfig {
    pub name: String,
    pub event_capacity: usize,
    pub max_pipeline_iterations: usize,
    pub credentials: HashMap<String, String>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            name: "lattice".into(),
            event_capacity: 256,
            max_pipeline_iterations: 20,
            credentials: HashMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct RuntimeParts {
    pub agent_registry: AgentRegistry,
    pub plugin_registry: Arc<PluginRegistry>,
    pub tool_registry: Arc<ToolRegistry>,
    pub memory: Option<Arc<dyn Memory>>,
}

impl Default for RuntimeParts {
    fn default() -> Self {
        Self {
            agent_registry: AgentRegistry::default(),
            plugin_registry: Arc::new(PluginRegistry::default()),
            tool_registry: Arc::new(ToolRegistry::default()),
            memory: None,
        }
    }
}

#[derive(Clone)]
pub struct Runtime {
    config: RuntimeConfig,
    router: Arc<ModelRouter>,
    agent_registry: Arc<AgentRegistry>,
    plugin_registry: Arc<PluginRegistry>,
    tool_registry: Arc<ToolRegistry>,
    memory: Option<Arc<dyn Memory>>,
    event_bus: Arc<EventBus>,
}

impl Runtime {
    pub fn new(config: RuntimeConfig, parts: RuntimeParts) -> Self {
        let router = if config.credentials.is_empty() {
            ModelRouter::new()
        } else {
            ModelRouter::with_credentials(config.credentials.clone())
        };
        Self {
            event_bus: Arc::new(EventBus::new(config.event_capacity)),
            config,
            router: Arc::new(router),
            agent_registry: Arc::new(parts.agent_registry),
            plugin_registry: parts.plugin_registry,
            tool_registry: parts.tool_registry,
            memory: parts.memory,
        }
    }

    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::default()
    }

    pub fn from_lattice_dir(
        root: &Path,
        global_agents_dir: Option<&Path>,
    ) -> Result<Self, RuntimeError> {
        let dir = match global_agents_dir {
            Some(global) => LatticeDir::discover_with_global(root, global),
            None => LatticeDir::discover(root),
        }
        .map_err(|e| RuntimeError::Config(e.to_string()))?;

        Ok(Self::builder()
            .agent_registry(dir.registry)
            .bus_config(&dir.bus_config)
            .build())
    }

    pub fn resolve(&self, model: &str) -> Result<ResolvedModel, RuntimeError> {
        self.resolve_with_provider(model, None)
    }

    pub fn resolve_with_provider(
        &self,
        model: &str,
        provider_override: Option<&str>,
    ) -> Result<ResolvedModel, RuntimeError> {
        Ok(self.router.resolve(model, provider_override)?)
    }

    pub fn inspect_model(&self, model: &str) -> Result<ResolvedModel, RuntimeError> {
        Ok(self.router.inspect_model(model)?)
    }

    pub fn list_models(&self) -> Vec<String> {
        self.router.list_models()
    }

    pub fn list_authenticated_models(&self) -> Vec<String> {
        self.router.list_authenticated_models()
    }

    pub fn register_model(&mut self, entry: ModelCatalogEntry) -> Result<(), RuntimeError> {
        match Arc::get_mut(&mut self.router) {
            Some(router) => {
                router.register_model(entry);
                Ok(())
            }
            None => Err(RuntimeError::Config(
                "custom models must be registered before cloning runtime handles".into(),
            )),
        }
    }

    pub async fn chat_complete(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<crate::core::provider::ChatResponse, RuntimeError> {
        let resolved = self.resolve(model)?;
        Ok(crate::core::chat_complete(&resolved, messages, tools).await?)
    }

    pub async fn stream_chat(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>, RuntimeError> {
        let resolved = self.resolve(model)?;
        Ok(crate::core::chat(&resolved, messages, tools).await?)
    }

    pub fn assemble_agent(
        &self,
        profile_name: &str,
    ) -> Result<crate::bus::AgentRunner, RuntimeError> {
        let profile = self
            .agent_registry
            .get(profile_name)
            .ok_or_else(|| RuntimeError::Config(format!("agent '{profile_name}' not found")))?
            .clone();
        let resolved = self.resolve(&profile.agent.model)?;
        crate::bus::assembly::assemble_agent(
            &profile,
            resolved,
            self.memory.clone(),
            Some(Arc::clone(&self.tool_registry)),
        )
        .map_err(|e| RuntimeError::Assembly(e.to_string()))
    }

    pub async fn run_agent(
        &self,
        profile_name: &str,
        input: &str,
        max_turns: Option<u32>,
    ) -> Result<serde_json::Value, RuntimeError> {
        let mut runner = self.assemble_agent(profile_name)?;
        let turns = max_turns.unwrap_or_else(|| runner.profile.handoff.max_turns.unwrap_or(10));
        runner
            .run(input, turns)
            .await
            .map_err(|e| RuntimeError::Assembly(e.to_string()))
    }

    pub async fn run_agent_events(
        &self,
        profile_name: &str,
        input: &str,
        max_turns: Option<u32>,
    ) -> Result<Vec<LoopEvent>, RuntimeError> {
        let mut runner = self.assemble_agent(profile_name)?;
        let turns = max_turns.unwrap_or_else(|| runner.profile.handoff.max_turns.unwrap_or(10));
        Ok(runner.agent_mut().run(input, turns).await)
    }

    pub async fn run_pipeline(&self, start_agent: &str, input: &str) -> PipelineRun {
        let mut pipeline = self.pipeline(&self.config.name);
        pipeline.run(start_agent, input).await
    }

    pub fn micro_agent(
        &self,
        profile_name: &str,
        bus: Arc<dyn Bus>,
    ) -> Result<MicroAgent, RuntimeError> {
        let profile = self
            .agent_registry
            .get(profile_name)
            .ok_or_else(|| RuntimeError::Config(format!("agent '{profile_name}' not found")))?
            .clone();

        let mut agent = MicroAgent::new(profile, bus, self.memory.clone(), None, None)
            .with_model_router(Arc::clone(&self.router))
            .with_tool_registry(Arc::clone(&self.tool_registry));

        if !self.config.credentials.is_empty() {
            agent = agent.with_credentials(self.config.credentials.clone());
        }

        Ok(agent)
    }

    pub fn dry_run_pipeline(&self, start_agent: &str) -> crate::bus::DryRunReport {
        self.pipeline(&self.config.name).dry_run(start_agent)
    }

    pub fn pipeline(&self, name: &str) -> Pipeline {
        let mut pipeline = Pipeline::new(
            name,
            Arc::clone(&self.agent_registry),
            self.memory.clone(),
            Some(Arc::clone(&self.event_bus)),
        )
        .with_model_router(Arc::clone(&self.router))
        .with_plugin_registry(Arc::clone(&self.plugin_registry))
        .with_tool_registry(Arc::clone(&self.tool_registry))
        .with_max_pipeline_iterations(self.config.max_pipeline_iterations);

        if !self.config.credentials.is_empty() {
            pipeline = pipeline.with_credentials(self.config.credentials.clone());
        }

        pipeline
    }

    pub fn router(&self) -> Arc<ModelRouter> {
        Arc::clone(&self.router)
    }

    pub fn agent_registry(&self) -> Arc<AgentRegistry> {
        Arc::clone(&self.agent_registry)
    }

    pub fn plugin_registry(&self) -> Arc<PluginRegistry> {
        Arc::clone(&self.plugin_registry)
    }

    pub fn tool_registry(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.tool_registry)
    }

    pub fn event_bus(&self) -> Arc<EventBus> {
        Arc::clone(&self.event_bus)
    }
}

#[derive(Default)]
pub struct RuntimeBuilder {
    config: RuntimeConfig,
    parts: RuntimeParts,
}

impl RuntimeBuilder {
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.config.name = name.into();
        self
    }

    pub fn event_capacity(mut self, capacity: usize) -> Self {
        self.config.event_capacity = capacity;
        self
    }

    pub fn max_pipeline_iterations(mut self, max: usize) -> Self {
        self.config.max_pipeline_iterations = max;
        self
    }

    pub fn credentials(mut self, credentials: HashMap<String, String>) -> Self {
        self.config.credentials = credentials;
        self
    }

    pub fn bus_config(mut self, config: &crate::bus::BusToml) -> Self {
        self.config.event_capacity = config.subscriber_buffer;
        self
    }

    pub fn agent_registry(mut self, registry: AgentRegistry) -> Self {
        self.parts.agent_registry = registry;
        self
    }

    pub fn plugin_registry(mut self, registry: Arc<PluginRegistry>) -> Self {
        self.parts.plugin_registry = registry;
        self
    }

    pub fn tool_registry(mut self, registry: Arc<ToolRegistry>) -> Self {
        self.parts.tool_registry = registry;
        self
    }

    pub fn memory(mut self, memory: Arc<dyn Memory>) -> Self {
        self.parts.memory = Some(memory);
        self
    }

    pub fn build(self) -> Runtime {
        Runtime::new(self.config, self.parts)
    }
}

pub struct RuntimeHandle {
    runtime: Arc<Runtime>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl RuntimeHandle {
    pub fn new(runtime: Runtime) -> Self {
        Self {
            runtime: Arc::new(runtime),
            tasks: Mutex::new(Vec::new()),
        }
    }

    pub fn runtime(&self) -> Arc<Runtime> {
        Arc::clone(&self.runtime)
    }

    pub fn track(&self, task: tokio::task::JoinHandle<()>) {
        self.tasks
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push(task);
    }

    pub fn shutdown(&self) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|err| err.into_inner());
        for task in tasks.drain(..) {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::memory::InMemoryMemory;
    use crate::bus::InMemoryBus;

    #[test]
    fn runtime_owns_shared_registries_and_events() {
        let runtime = Runtime::builder()
            .memory(Arc::new(InMemoryMemory::new()))
            .build();

        assert!(runtime.agent_registry().list().is_empty());
        assert!(runtime.plugin_registry().is_empty());
        assert!(runtime.tool_registry().is_empty());
        let _rx = runtime.event_bus().subscribe();
    }

    #[test]
    fn pipeline_uses_runtime_owned_parts() {
        let plugin_registry = Arc::new(PluginRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let runtime = Runtime::builder()
            .name("test-runtime")
            .plugin_registry(Arc::clone(&plugin_registry))
            .tool_registry(Arc::clone(&tool_registry))
            .build();

        let pipeline = runtime.pipeline("test-pipeline");

        assert_eq!(pipeline.name, "test-pipeline");
        assert!(pipeline.model_router.is_some());
        assert!(Arc::ptr_eq(
            pipeline.plugin_registry.as_ref().unwrap(),
            &plugin_registry
        ));
        assert!(Arc::ptr_eq(
            pipeline.tool_registry.as_ref().unwrap(),
            &tool_registry
        ));
    }

    #[test]
    fn micro_agent_uses_runtime_owned_parts() {
        let profile = crate::bus::AgentProfile {
            agent: crate::bus::AgentConfig {
                name: "agent".into(),
                model: "sonnet".into(),
                description: String::new(),
                skippable: false,
                tags: vec![],
            },
            system: crate::bus::SystemConfig {
                prompt: "test".into(),
                file: None,
            },
            tools: crate::bus::ToolsConfig::default(),
            behavior: crate::bus::BehaviorConfig::default(),
            handoff: crate::bus::HandoffConfig::default(),
            bus: crate::bus::BusConfigProfile::default(),
            memory: crate::bus::MemoryConfigProfile::default(),
            plugins: None,
        };
        let registry = crate::bus::AgentRegistry::from_profiles([profile]);
        let runtime = Runtime::builder().agent_registry(registry).build();
        let bus = Arc::new(InMemoryBus::with_defaults());

        let agent = runtime.micro_agent("agent", bus).unwrap();

        assert!(agent.model_router.is_some());
        assert!(agent.tool_registry.is_some());
    }
}
