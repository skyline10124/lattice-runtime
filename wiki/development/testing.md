# Testing

Run the full Runtime suite:

```sh
cargo test
cargo clippy -- -D warnings
cargo fmt --all --check
```

Useful focused checks:

```sh
cargo test -p lattice-core
cargo test -p lattice-agent
cargo test -p lattice-plugin
cargo test -p lattice-bus
cargo check -p lattice-python
```

## Expectations

- Runtime must compile without Swarm or Plugins.
- `lattice-plugin` tests use local test plugins, not official plugin implementations.
- Security behavior should be tested on Rust paths, not only Python paths.
- Profile loading should report failed loads instead of silently skipping them.
