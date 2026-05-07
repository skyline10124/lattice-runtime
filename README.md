# LATTICE-Runtime

![LATTICE banner](logo-banner.svg)

Runtime is the Rust execution core of LATTICE. It owns model resolution, provider transport, streaming, retry policy, agent execution, tool safety, pipeline orchestration, plugin contracts, bus events, and WebSocket serving.

The preferred Rust interface is `lattice::runtime::Runtime`. It owns the shared model router, profile registry, plugin registry, tool registry, memory, event bus, and pipeline construction policy. Lower modules remain public for extension, but application entrypoints should not manually wire agents and pipelines unless they are building a new runtime adapter.

Official plugin implementations are deliberately outside this repository. They live in [LATTICE-Plugins](https://github.com/Skyline10124/LATTICE-Plugins). The user-facing CLI/TUI entrypoint lives in [LATTICE-Swarm](https://github.com/Skyline10124/LATTICE-Swarm).

## Workspace

```text
LATTICE-Runtime/
└── lattice/           Rust runtime crate: core, agent, plugin, bus and runtime modules
```

The Rust crate keeps a one-way internal module direction:

```text
core
  ↑
agent
  ↑
plugin
  ↑
bus
  ↑
runtime

```

`lattice::plugin` contains runtime plugin contracts only. It does not contain official plugins.

## Build

```sh
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt --all --check
```

The workspace is a normal Cargo workspace. No external service is required for unit tests.

## Core Concepts

- `ResolvedModel`: canonical model plus provider, protocol, base URL and credential status.
- `Transport`: protocol adapter for OpenAI Chat Completions, Anthropic Messages, Gemini GenerateContent and compatible endpoints.
- `StreamEvent`: normalized streaming event surface for tokens, reasoning, tool calls, usage and terminal events.
- `Agent`: multi-turn async conversation loop with tool execution and prompt assembly.
- `Plugin`: typed LLM function contract; runtime runs it but does not ship official implementations.
- `Pipeline`: profile-driven agent orchestration with handoff and fork control flow.
- `Bus`: in-memory RPC/pub-sub fabric for runtime events and micro-agents.
- `Runtime`: deep module that owns router/registries/memory/events and exposes resolve, chat, agent, and pipeline execution.

## Runtime Interface

```rust
use lattice::runtime::Runtime;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let runtime = Runtime::builder().build();
let model = runtime.resolve("sonnet")?;
let run = runtime.run_pipeline("reviewer", "review this change").await;
# Ok(())
# }
```

`Runtime` is the seam Swarm and other Rust frontends should target. It gives callers leverage by hiding construction order, registry sharing, model router reuse, event bus wiring, micro-agent construction, and plugin/tool registry attachment behind one interface.

## Documentation

Start with [wiki/README.md](wiki/README.md).

Key pages:

- [Architecture](wiki/architecture/overview.md)
- [Crate Map](wiki/architecture/crate-map.md)
- [Model Resolution](wiki/architecture/model-resolution.md)
- [Streaming](wiki/reference/streaming.md)
- [Plugin Runtime Contract](wiki/reference/plugin-contract.md)
- [Testing](wiki/development/testing.md)

## Repository Relationships

```text
LATTICE-Swarm
  ├── submodule: LATTICE-Runtime
  └── submodule: LATTICE-Plugins

LATTICE-Plugins → LATTICE-Runtime
```

Primary maintenance happens in `LATTICE-Runtime` and `LATTICE-Swarm`. The legacy mono-repo at `~/lattice` is no longer maintained.

## License

Apache-2.0. See [LICENSE](LICENSE).
