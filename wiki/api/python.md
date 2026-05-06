# Python Binding

`lattice-python` exposes a PyO3 module named `lattice_core`.

## Build

```sh
cd lattice-python
pip install maturin
maturin develop
```

## Usage

```python
from lattice_core import LatticeEngine

engine = LatticeEngine()
resolved = engine.resolve_model("sonnet")
response = engine.chat([
    {"role": "user", "content": "Hello"}
], model="sonnet")
```

`stream_chat` is channel-backed and yields events incrementally instead of collecting the full response first.

## Message Support

Python dictionaries support:

- `role`
- `content`
- `name`
- `tool_call_id`
- `tool_calls`

This lets Python callers send tool results and preserve assistant tool calls.
