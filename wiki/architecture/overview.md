# Runtime Architecture

Runtime is the execution layer of LATTICE. It turns a model name, prompt, agent profile or plugin slot into actual model calls, tool execution and events.

## Shape

```text
lattice-core
  ├── catalog and aliases
  ├── ModelRouter
  ├── provider requests/responses
  ├── protocol transports
  ├── streaming parsers
  ├── retry and error classification
  ├── shared message/tool types
  ├── shared memory and handoff contracts
  └── security helpers

lattice-agent
  ├── Agent
  ├── tool loop
  ├── prompt compiler
  ├── memory adapters
  ├── sandbox
  └── hooks and audit log

lattice-plugin
  ├── Plugin trait
  ├── ErasedPlugin
  ├── PluginRegistry
  ├── manifest loader
  ├── PluginDagRunner
  └── runtime behavior policies

lattice-bus
  ├── AgentProfile
  ├── Pipeline
  ├── AgentRunner
  ├── InMemoryBus
  ├── micro-agent RPC/pub-sub
  ├── watcher
  └── WebSocket serving

lattice-runtime
  ├── Runtime
  ├── RuntimeBuilder
  ├── shared ModelRouter
  ├── shared AgentRegistry
  ├── shared PluginRegistry
  ├── shared ToolRegistry
  ├── shared memory and events
  └── RuntimeHandle
```

## Dependency Rules

Runtime crates follow a one-way dependency direction:

```text
core ← agent ← plugin ← bus ← runtime
```

The arrows point toward the dependency. Lower crates do not know about higher crates.

## Call Flow

Direct model call:

```text
model name
  → ModelRouter::resolve
  → ResolvedModel
  → TransportDispatcher
  → provider request
  → StreamEvent / ChatResponse
```

Agent call:

```text
Agent::run
  → prompt registry collects sections
  → prompt compiler budgets and renders messages
  → lattice-core chat stream
  → optional tool calls
  → tool executor and sandbox
  → LoopEvent stream
```

Pipeline call:

```text
Runtime::run_pipeline
  → Runtime-owned AgentRegistry
  → Runtime-built Pipeline
  → AgentRunner or PluginDagRunner
  → handoff rules and fork targets
  → events published to bus
```

## Boundaries

Runtime exposes contracts that other repositories consume:

- `lattice::core::types`: messages, roles, tools and behavior modes.
- `lattice::core::handoff`: handoff condition/rule/target evaluation.
- `lattice::plugin::Plugin`: typed plugin interface.
- `lattice::plugin::registry::PluginRegistry`: runtime registry for official or local plugins.
- `lattice::runtime::Runtime`: preferred frontend seam for model, agent and pipeline execution.
- `lattice::bus::Pipeline`: lower-level profile-driven execution implementation.

Runtime does not expose official plugin implementations.
