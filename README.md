# LATTICE-Runtime

![LATTICE banner](logo-banner.svg)

Runtime is the execution core of LATTICE. It owns model resolution, provider transport, streaming, retry policy, agent execution, tool safety, pipeline orchestration, plugin contracts, bus events, WebSocket serving, and the Python binding.

Official plugin implementations are deliberately outside this repository. They live in [LATTICE-Plugins](https://github.com/Skyline10124/LATTICE-Plugins). The user-facing CLI/TUI entrypoint lives in [LATTICE-Swarm](https://github.com/Skyline10124/LATTICE-Swarm).

## Workspace

```text
LATTICE-Runtime/
├── lattice-core/      model catalog, router, transports, streaming, retry, shared contracts
├── lattice-agent/     async agent loop, tools, sandbox, memory, prompt compiler, hooks
├── lattice-plugin/    Plugin trait, registry, manifest loader, erased runner, DAG runner
├── lattice-bus/       pipeline profiles, event bus, micro-agent RPC, watcher, WebSocket
└── lattice-python/    PyO3 binding for model resolution, chat and streaming
```

Dependency direction is internal and one-way:

```text
lattice-core
  ↑
lattice-agent
  ↑
lattice-plugin
  ↑
lattice-bus

lattice-python → lattice-core
```

`lattice-plugin` contains runtime plugin contracts only. It does not contain official plugins.

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

## Documentation

Start with [wiki/README.md](wiki/README.md).

Key pages:

- [Architecture](wiki/architecture/overview.md)
- [Crate Map](wiki/architecture/crate-map.md)
- [Model Resolution](wiki/architecture/model-resolution.md)
- [Streaming](wiki/reference/streaming.md)
- [Plugin Runtime Contract](wiki/reference/plugin-contract.md)
- [Python Binding](wiki/api/python.md)
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
