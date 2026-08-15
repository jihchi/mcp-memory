# MCP-Memory

A standalone Rust MCP server for persistent, project-scoped agent memory. Records are stored as [Open Knowledge Format (OKF v0.2)](SPEC.md) Markdown and indexed by bundled SQLite with FTS5.

## Build

```bash
cargo build --release
```

The executable is `target/release/mcp-memory`. It does not require Python, SQLite, or another runtime to be installed.

## Run

### stdio (default)

```bash
mcp-memory
# equivalent: mcp-memory --transport stdio
```

### Streamable HTTP

```bash
mcp-memory --transport http --bind 127.0.0.1:8080 --path /mcp
```

Endpoint: `http://127.0.0.1:8080/mcp`

### Legacy HTTP+SSE

```bash
mcp-memory --transport sse --bind 127.0.0.1:8080 --path /sse
```

SSE endpoint: `http://127.0.0.1:8080/sse`

Message endpoint: `http://127.0.0.1:8080/message`

The HTTP transports bind to localhost by default. Exposing them on a network makes project filesystem access available to connected MCP clients and should only be done behind appropriate access controls.

## MCP tools

- `memory_store` — store or update an OKF memory
- `memory_retrieve` — retrieve by key and namespace
- `memory_search` — full-text, tag, and namespace search
- `memory_delete` — delete a memory
- `memory_get_last` — retrieve `system/last_memory`
- `memory_update_last` — update the session checkpoint

Every tool requires `project_root`. Tool names, parameters, defaults, output text, SQLite schema, and OKF bundle layout are compatible with the original Python implementation.

## Storage

For a project rooted at `/path/to/project`:

```text
/path/to/project/
  .mcp_memory/memories.db   # SQLite + FTS5 index
  memory/                   # human-readable OKF bundle
    index.md
    log.md
    ...
```

Existing databases and OKF files created by the Python implementation can be used directly.

Environment variables:

- `MCP_MEMORY_PROJECT_ROOT` — fallback project root
- `MCP_MEMORY_DB_PATH` — database path; default `.mcp_memory/memories.db`
- `MCP_MEMORY_DIR` — OKF bundle path; default `memory`

Relative database and memory paths are resolved beneath each tool call's `project_root`. Absolute paths provide a shared global store.

## Test

```bash
cargo test
```

See [`OKF_RULES.md`](OKF_RULES.md) for the concise OKF rules and [`SPEC.md`](SPEC.md) for the complete specification.
