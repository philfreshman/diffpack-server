# diffpack-server

An MCP server in Rust, deployed to Vercel, exposing what
[`diffpack-engine`](https://github.com/philfreshman/diffpack-engine) can do as
tools an agent can call: resolve a package on npm, crates.io or PyPI, fetch and
extract its archives, and diff one version against another.

**Status: foundations.** The crate builds, tests and deploys as a single
`/health` function. There is no MCP endpoint and there are no tools yet — see
[#2](https://github.com/philfreshman/diffpack-server/issues/2) for the plan and
what is done.

This repository changes a property the other two rely on. Today no package
content passes through a diffpack server: the app is client-side by design and
the engine runs in the browser. An MCP server fetches and diffs archives
server-side, so whatever lands here belongs in the sibling repositories' docs
as well.

## Layout

Vercel's Rust runtime fixes it. Every handler is a `[[bin]]` whose path is a
file under `api/`, and `vercel.json` rewrites all traffic to it. So this repo
is one binary in front of a library:

```
api/mcp.rs       The deployed function. Thin: it matches a path, asks the
                 library what to say, and writes a response.
src/lib.rs       Everything with a decision in it, where a test can reach it
                 without a runtime.
src/health.rs    The /health body.
src/cache_key.rs The deterministic diff cache key.
src/engine.rs    The one module allowed to import diffpack-engine.
docs/            Normative specifications. docs/cache-key.md is one.
fixtures/        Golden vectors two languages are tested against.
scripts/         Checks that run in CI and need no toolchain.
```

Two boundaries are enforced rather than documented:

- **`src/engine.rs` is the only importer of `diffpack_engine`.** One seam means
  one file to change when the engine moves. `scripts/check-engine-seam.sh`
  fails the build otherwise.
- **`docs/cache-key.md` is normative, not descriptive.** The cache key is a
  contract with a TypeScript implementation that will never share a line of
  code with this one, so the document and
  `fixtures/cache-key-vectors.json` are the source of truth and both
  implementations are held to them.

## Commands

```bash
cargo test                              # the suite
cargo fmt --all --check                 # formatting, no compile needed
cargo clippy --all-targets -- -D warnings
cargo build --release                   # produces the `mcp` binary
./scripts/check-engine-seam.sh          # the engine import boundary
```

The toolchain is pinned in `rust-toolchain.toml` so CI and Vercel's build
container cannot drift apart silently. CI runs all five on every pull request
into `main`.

## The sibling repositories

diffpack is three repositories meant to be checked out side by side under one
parent directory:

```
<parent>/
├── diffpack/          github.com/philfreshman/diffpack
├── diffpack-engine/   github.com/philfreshman/diffpack-engine
└── diffpack-server/   github.com/philfreshman/diffpack-server   ← this one
```

| Repo | Sibling path | What lives there |
| --- | --- | --- |
| **diffpack** | `../diffpack` | The web app at [diffpack.io](https://www.diffpack.io) — TanStack Start on Vite, the UI, the registry adapters and the worker that drives the diff. Deployed to Vercel, where the server function renders the page shell and nothing more: package archives go from the registry straight to the browser. Read its `README.md`, `CONTRIBUTING.md` and `AGENTS.md`. |
| **diffpack-engine** | `../diffpack-engine` | The Rust crate compiled to WebAssembly that fetches, extracts and diffs package archives. Published to npm as `@philfreshman/diffpack-engine`; the app consumes a pinned version rather than building it. This repo depends on the same crate natively, pinned to a git tag. |
| **diffpack-server** | *this checkout* | The MCP server. |
