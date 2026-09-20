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
scripts/         The checks CI runs, and the hook installer that makes a
                 commit run them too.
deny.toml        The policy over the dependency graph.
vercel.json      The deployment shape: the catch-all rewrite, the function
                 timeout, and which branches deploy.
.githooks/       The pre-commit hook. Not active until install-hooks.sh.
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
./scripts/checks.sh                     # clippy, deny, audit, test
```

The toolchain is pinned in `rust-toolchain.toml` so CI and Vercel's build
container cannot drift apart silently. CI runs all of them on every pull
request into `development` and into `main`.

## The checks that block a commit

```bash
./scripts/install-hooks.sh
```

Run once per clone. It points `core.hooksPath` at `.githooks/` and installs
`cargo-deny` and `cargo-audit`, after which `git commit` runs the checks in
`scripts/checks.sh` that match what is staged:

| Check | Runs when the commit touches | What it is for |
| --- | --- | --- |
| `cargo clippy --all-targets -- -D warnings` | `*.rs`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` | Lints, as errors. |
| `cargo deny --all-features check` | `Cargo.toml`, `Cargo.lock`, `deny.toml` | Advisories, licenses, banned crates, and where the code came from. See `deny.toml`. |
| `cargo audit --deny warnings` | `Cargo.toml`, `Cargo.lock`, `deny.toml` | The same advisory database, read without `deny.toml` — the second opinion that notices an ignore that has expired. |
| `cargo test` | `*.rs`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `fixtures/`, `vercel.json` | The suite. |

`scripts/checks.sh` is the single definition of each one: the hook and the CI
jobs both call it, so what fails locally is what fails on the pull request.
Every requested check runs before the hook gives up, so one commit attempt
tells you everything that is wrong.

Two things to know. The hook checks the working tree rather than the staged
snapshot — stashing the unstaged remainder to isolate the index is a good way
to lose work — so a partial commit is checked by CI, not here. And `deny` and
`audit` need the network for the RustSec database.

`git commit --no-verify` skips the hook, which is a reasonable thing to do for
a work-in-progress commit on a branch. CI is the gate that cannot be skipped.
`DIFFPACK_HOOK_ALL=1 git commit` forces all four regardless of what is staged.

## Deployment

Pushes to `main` deploy to production. Nothing else deploys at all — not
`development`, not a branch, not a pull request — so the only way code reaches
production is a merge into `main`.

`vercel.json` is small on purpose. Vercel's Rust runtime finds `api/*.rs` and
`Cargo.toml` by itself, so there is no build command to set, and the file says
only the three things the platform cannot work out:

| Key | Why it is there |
| --- | --- |
| `rewrites` | The runtime serves `api/mcp.rs` at `/api/mcp` and nothing else. Without a catch-all, `/health` and `/mcp` are 404s from the platform and the handler never sees them. Routing belongs inside the function, where a test can reach it. |
| `functions.maxDuration` | A Rust handler cannot declare its timeout in code the way a Node one can. This is the only place 300 seconds can be said. |
| `git.deploymentEnabled` | The branch policy, and the same pattern [`diffpack`](https://github.com/philfreshman/diffpack/blob/main/vercel.json) uses. |

Two things here are easy to get wrong.

**The wildcard has to be there.** Vercel matches branch names with minimatch
and deploys when *any* matching rule is true, so `{"main": true}` on its own
deploys every branch — `main` matches its rule, and every other branch matches
no rule and falls through to the default, which is on. `"**": false` is what
turns the default off, and the specific entry then beats it. Deleting the
wildcard as redundant is the mistake it looks like it invites.

**Every deploy compiles the world.** The Rust runtime is in Beta and there is
no Cargo cache between deployments, so each one builds the crate,
`diffpack-engine` and the whole of `vercel_runtime` from scratch — under
`lto = "fat"` and `codegen-units = 1`, which `Cargo.toml` sets because a
handler is built once and invoked many times. Cold build time is measured and
recorded in [#8](https://github.com/philfreshman/diffpack-server/issues/8);
that number is the answer to every later "why is the deploy slow".

`tests/deploy.rs` holds `vercel.json` against the crate layout, because
nothing compiles against it: renaming the `[[bin]]` or moving `api/mcp.rs`
would otherwise be invisible until a deploy served a 404. It deliberately does
not assert the timeout or the branch policy back at the file — those are
verified against the deployed result, not against the file that was written.

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
