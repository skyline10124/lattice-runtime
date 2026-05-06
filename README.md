# LATTICE-Runtime

Runtime workspace for LATTICE.

This repository owns the execution-time crates:

- `lattice-core`: model catalog, routing, transport, streaming, retry, shared types, security helpers, memory and handoff contracts.
- `lattice-agent`: async agent loop, tool execution, prompt assembly, sandboxing, memory adapters, hooks and audit support.
- `lattice-plugin`: plugin trait, erased runner, registry, manifest loader, DAG runner and watcher.
- `lattice-bus`: pipelines, agent profiles, event bus, micro-agent RPC/pub-sub and WebSocket serving.
- `lattice-python`: PyO3 bindings for Python callers.

Official plugin implementations are intentionally not in this repository; they live in `LATTICE-Plugins`.

License: Apache-2.0.
