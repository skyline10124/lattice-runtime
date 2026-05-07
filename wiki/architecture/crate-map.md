# Crate Map

| Crate | Responsibility | Depends On |
| --- | --- | --- |
| `lattice-core` | Model catalog, routing, transports, streaming, retry, token estimation, shared contracts | external crates only |
| `lattice-agent` | Agent state, prompt assembly, tool execution, sandbox, memory, hooks, audit | `lattice-core` |
| `lattice-plugin` | Plugin trait, erased runner, registry, manifest loader, DAG runner, behavior policies | `lattice-core`, `lattice-agent` |
| `lattice-bus` | Pipeline, profile loading, agent runner, event bus, watcher, micro-agent RPC, WebSocket serving | `lattice-core`, `lattice-agent`, `lattice-plugin` |
| `lattice-runtime` | Deep runtime interface that owns router, registries, memory, event bus and pipeline construction | `lattice-core`, `lattice-agent`, `lattice-plugin`, `lattice-bus` |

## `lattice-core`

Important modules:

- `catalog/`: embedded model catalog, aliases, provider defaults and API protocols.
- `router.rs`: model normalization, alias resolution, provider selection and credential resolution.
- `transport/`: protocol adapters for OpenAI-compatible, Anthropic and Gemini APIs.
- `streaming/`: SSE parsing and normalized streaming events.
- `retry.rs`: jittered exponential backoff and error classification.
- `errors.rs`: `LatticeError` and provider error mapping.
- `memory.rs`: shared memory contracts used by agent and bus layers.
- `handoff.rs`: handoff rules shared by plugin and bus orchestration.
- `security.rs`: URL and private/reserved IP validation.

## `lattice-agent`

Important modules:

- `lib.rs`: `Agent`, `ToolExecutor`, `PluginAgent` and loop events.
- `state.rs`: conversation state and context trimming.
- `tools.rs`: default tool executor.
- `sandbox.rs`: command and URL safety checks.
- `prompt/`: collect, sort, budget, trim and render prompt sections.
- `memory/`: in-memory and SQLite memory adapters.
- `hook.rs`: hook chain for tool validation.
- `audit.rs`: append-only audit log.

## `lattice-plugin`

Important modules:

- `lib.rs`: `Plugin`, `Behavior`, `PluginRunner` and runtime errors.
- `erased.rs`: type-erased plugin adapter.
- `erased_runner.rs`: shared plugin run loop.
- `registry.rs`: runtime plugin registry.
- `manifest.rs` and `loader.rs`: declarative local plugin loading.
- `dag_runner.rs`: plugin DAG execution.
- `orchestration.rs`: DAG configuration and handoff re-exports.

## `lattice-bus`

Important modules:

- `profile.rs`: TOML-backed agent profile.
- `pipeline.rs`: pipeline execution and fork control flow.
- `runner.rs`: agent profile execution through `Agent`.
- `assembly.rs`: construction of agents from profiles.
- `lib.rs`: bus trait, in-memory bus, RPC and pub/sub.
- `micro_agent.rs`: micro-agent protocol.
- `ws.rs`: WebSocket bridge.
- `watcher.rs`: profile hot reload.

## `lattice-runtime`

Important modules:

- `lib.rs`: `Runtime`, `RuntimeBuilder`, `RuntimeConfig`, `RuntimeParts` and `RuntimeHandle`.
- `Runtime::resolve()`: shared model router access.
- `Runtime::chat_complete()` and `Runtime::stream_chat()`: direct model execution through the shared router.
- `Runtime::assemble_agent()` and `Runtime::run_agent()`: profile-backed agent execution.
- `Runtime::pipeline()` and `Runtime::run_pipeline()`: pipeline construction with shared model router, plugin registry, tool registry, memory and event bus.
