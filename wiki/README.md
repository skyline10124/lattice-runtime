# LATTICE-Runtime Wiki

![LATTICE banner](../logo-banner.svg)

This wiki documents the runtime repository: the crates that execute model calls, run agents, expose plugin contracts, orchestrate pipelines and provide Python bindings.

## Quick Navigation

| Topic | Page |
| --- | --- |
| Runtime architecture | [architecture/overview](architecture/overview.md) |
| Crate responsibilities | [architecture/crate-map](architecture/crate-map.md) |
| Model resolution | [architecture/model-resolution](architecture/model-resolution.md) |
| Agent execution | [architecture/agent-runtime](architecture/agent-runtime.md) |
| Pipeline and bus | [architecture/pipeline-and-bus](architecture/pipeline-and-bus.md) |
| Security model | [architecture/security](architecture/security.md) |
| Plugin contract | [reference/plugin-contract](reference/plugin-contract.md) |
| Streaming events | [reference/streaming](reference/streaming.md) |
| Python binding | [api/python](api/python.md) |
| Testing | [development/testing](development/testing.md) |

## Status

Runtime is the main execution repository. It includes `lattice-core`, `lattice-agent`, `lattice-plugin`, `lattice-bus` and `lattice-python`.

Official plugins are not part of Runtime. They live in `LATTICE-Plugins` and depend on Runtime contracts.

## Invariants

- Runtime must not depend on Swarm or Plugins.
- `lattice-plugin` must define plugin contracts and loading mechanics, not official plugin implementations.
- Shared types used by multiple runtime crates belong in `lattice-core`.
- Python bindings call runtime crates; runtime crates do not call Python.
- Security checks for model endpoints and tools must live on the Rust execution path.
