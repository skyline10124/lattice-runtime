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

lattice-python
  └── PyO3 adapter over lattice-core
```

## Dependency Rules

Runtime crates follow a one-way dependency direction:

```text
core ← agent ← plugin ← bus
core ← python
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
Pipeline::run
  → AgentRegistry loads TOML profiles
  → AgentRunner or PluginDagRunner
  → handoff rules and fork targets
  → events published to bus
```

## Boundaries

Runtime exposes contracts that other repositories consume:

- `lattice_core::types`: messages, roles, tools and behavior modes.
- `lattice_core::handoff`: handoff condition/rule/target evaluation.
- `lattice_plugin::Plugin`: typed plugin interface.
- `lattice_plugin::registry::PluginRegistry`: runtime registry for official or local plugins.
- `lattice_bus::Pipeline`: profile-driven execution API for CLI/TUI callers.

Runtime does not expose official plugin implementations.
