# diffpack-server

An MCP server in Rust, deployed to Vercel, exposing what
[`diffpack-engine`](https://github.com/philfreshman/diffpack-engine) can do as
tools an agent can call: resolve a package on npm, crates.io or PyPI, fetch and
extract its archives, and diff one version against another.

**Status: transport, the tools that read a package, the one that diffs two,
and the two that read a diff back.** The crate builds, tests and deploys,
and `/mcp` speaks Streamable HTTP: a client connects, negotiates a protocol
revision, lists tools and calls one. There are eight. `search_packages` finds
a package from a name half
remembered, which is where an agent with no exact name to start from starts;
`resolve_archive_url` answers from its arguments and fetches nothing;
`list_package_versions` says what a package has released, most recently
published first; `list_package_files` downloads a published version and lists
what is inside it, a page at a time; `get_file_content` returns one of those
files, cut short if it is longer than a response can carry; and
`diff_package_versions` compares two versions and answers with totals, a
sample of the files that moved most, and a handle; `get_diff_tree` takes that
handle and lists the comparison's files and directories a page at a time; and
`get_file_diff` takes the same handle and returns one of those files' diffs,
trimmed to the lines around each change unless you ask for all of them. The
resources that read a diff back arrive with
[#16](https://github.com/philfreshman/diffpack-server/issues/16).
`/health` is the other route and is what a monitor watches.

Production serves whatever was last merged to `main`, so a branch merged into
`development` is not live until it is promoted. See
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
api/mcp.rs       The deployed function: wraps the router, and nothing else.
src/lib.rs       Everything with a decision in it.
src/router.rs    Every route this function serves.
src/mcp.rs       The MCP handler: identity, capabilities, the tool list.
src/tools/       One module per tool: its definition and its handler.
src/registry.rs  What a registry is: npm, crates.io, PyPI, described once.
src/archive/     A version's files: fetch, size cap, extract, one interface.
src/catalogue/   What a package has released, most recently published first.
src/search/      A name half remembered, matched against what a registry has.
src/fetch.rs     Every request to a registry: user agent, timeout, redirects.
src/store/       Diff results kept between calls, over Vercel Blob.
src/error.rs     Which channel a failure reaches the client on.
src/log.rs       One structured line per tool call, which is what an incident
                 is read back from.
src/health.rs    The /health body.
src/page.rs      The 4.5 MB response ceiling: pages, and cut blobs.
src/handle.rs    The handle a diff is asked for again by.
src/cache_key.rs The deterministic diff cache key.
src/engine.rs    The one module allowed to import diffpack-engine.
tests/           The suite, driven at the seams: real requests through the
                 real router, and the vectors read from fixtures/.
docs/            The architecture, the decisions, and the specifications that
                 are normative rather than descriptive.
fixtures/        Golden vectors two languages are tested against, and the
                 archives the suite reads instead of a registry.
scripts/         The checks CI runs, and the hook installer that makes a
                 commit run them too.
deny.toml        The policy over the dependency graph.
renovate.json5   Which dependency updates land on their own, and which wait.
vercel.json      The deployment shape: the catch-all rewrite, the function
                 timeout, and which branches deploy.
.githooks/       The pre-commit hook. Not active until install-hooks.sh.
```

That is the file list. What each module is *for*, which modules phases 3 and 4
add, and what any of them may import is in
[`docs/architecture.md`](docs/architecture.md); the nouns they trade in are in
[`CONTEXT.md`](CONTEXT.md), and the decisions behind both are in
[`docs/adr/`](docs/adr/). Two copies of a module list is how the two start
disagreeing, so this one names the files and stops.

Three boundaries there are enforced rather than trusted: `src/engine.rs` is the
only importer of `diffpack_engine`, a module under `src/tools/` reaches the
network and the cache only through the seams, and `docs/cache-key.md` is the
source of truth for the cache key rather than a description of it. The first
two are `./scripts/checks.sh seams`; the third is `cargo test`.

## Commands

```bash
cargo test                              # the suite
cargo test --test networked -- --ignored # the same, against the real registries
cargo test --lib -- --ignored store::blob # the same, against the real blob store
cargo fmt --all --check                 # formatting, no compile needed
cargo clippy --all-targets -- -D warnings
cargo build --release                   # produces the `mcp` binary
./scripts/checks.sh seams               # the module boundaries
./scripts/checks.sh                     # fmt, seams, clippy, deny, audit, test
```

The toolchain is pinned in `rust-toolchain.toml` so CI and Vercel's build
container cannot drift apart silently. CI runs all of them on every pull
request into `development` and into `main`.

`tests/networked.rs` is the exception and is `#[ignore]`d for it: it fetches
real archives from npm, crates.io and PyPI. So is the one test in
`src/store/blob.rs` that writes to the blob store, which is in the module
rather than beside the others because the client it drives is private to
`src/store/`. The suite is otherwise offline — `Ctx::fixture` gives every
seam an adapter that reads the checked-in sets under `fixtures/`, and there is
no way to build a context that has one of them and not the others — so a
registry having a bad afternoon cannot fail a pull request. What the networked
tests are *for* is the half that cannot be
checked any other way: that the URLs this server builds are the URLs those
three registries actually serve, and that the requests `src/store/` writes are
the ones Vercel Blob answers. Worth running when `src/registry.rs`,
`src/archive/`, `src/catalogue/` or `src/store/` changes.

## The checks that block a commit

```bash
./scripts/install-hooks.sh
```

Run once per clone. It points `core.hooksPath` at `.githooks/` and installs
`cargo-deny` and `cargo-audit`, after which `git commit` runs the checks in
`scripts/checks.sh` that match what is staged:

| Check | Runs when the commit touches | What it is for |
| --- | --- | --- |
| `./scripts/checks.sh seams` | `*.rs`, `scripts/check-*seam*.sh` | The boundaries the compiler cannot see: who may import the engine, and what a tool module may reach. Two greps, no toolchain. |
| `cargo fmt --all --check` | `*.rs`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` | Formatting. It compiles nothing, so it costs a second and removes the one way a commit that passed here can still go red on the pull request. |
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
`DIFFPACK_HOOK_ALL=1 git commit` forces all six regardless of what is staged.

## Dependency updates

[Renovate](https://docs.renovatebot.com) opens the pull requests, configured by
[`renovate.json5`](renovate.json5). It is a GitHub App and not a workflow: it
runs on Mend's infrastructure, so nothing in `.github/workflows/` invokes it and
there is no token for it in this repository. It finds three sets of dependencies
without being told to — `Cargo.toml` and `Cargo.lock`, the `channel` in
`rust-toolchain.toml`, and the `uses:` lines in the CI workflow.

Two runs a month, on the 1st and the 15th, the same days as the sibling
repositories so that a fortnight's churn arrives together. The `Cargo.lock`
sweep that moves transitive dependencies nothing asked for runs on the 1st.
Security fixes ignore the schedule and open immediately.

Nothing younger than three days is proposed at all. `deny.toml` says why this
repository is stricter than the other two: it is the one place in diffpack
where package archives are fetched and extracted on a server rather than in a
reader's browser, so the dependency graph is attack surface, and a malicious
publish is usually caught and yanked within hours.

What lands on its own and what waits for a person:

| Update | Automerged | Why |
| --- | --- | --- |
| A minor or patch, any manager | Yes | The seven required checks are the gate: clippy with `-D warnings`, the suite, `cargo deny`, `cargo audit` and the seam rules all ran against it. A bump that breaks any of them is a red pull request instead. |
| `Cargo.lock` maintenance | Yes | Same gate, and it is the only thing that proposes transitive versions. |
| A major, any dependency | No | The checks prove a major compiles and passes, not that it is the major we want. `Cargo.toml` carries a paragraph on why each dependency is at the version it is; a major is when to check that paragraph is still true. |
| `rust-toolchain.toml` | No | A new release brings new clippy lints and `-D warnings` makes each one a failure, so this pull request usually arrives red with a list of things to fix. It also pins what Vercel's build container installs. |
| `diffpack-engine` | No | See below. |

Two pairs move together rather than separately, because `Cargo.toml` says they
have to: `rmcp` with `schemars`, so that `JsonSchema` here stays the trait rmcp
asks for, and `axum` with `vercel_runtime`, whose axum integration decides which
axum major this crate may be on.

**An engine bump is red on arrival, and that is the point.** `engine::VERSION`
in `src/engine.rs` is a field in the cache key, and `tests/engine.rs` fails
while it and the tag in `Cargo.toml` disagree — bumping the tag alone would
serve diffs the current engine would not produce. So the pull request Renovate
opens is a notification that a release exists; landing it means moving the
constant in the same branch, reading the engine's release notes for changes to
rename detection, line counts or the unified-diff format, and knowing that every
`diff_id` changes so the cache starts cold.

To check the config before pushing it:

```bash
npx --yes --package renovate@latest renovate-config-validator --strict
```

That is the only command here that needs Node, which is why it is not in
`scripts/checks.sh`. Renovate validates the file on every run anyway and opens
an issue against the repository when it cannot read it.

Three things live on the GitHub side, because no file in this repository can
set them:

- The Renovate App, installed on `philfreshman/diffpack-server`.
- **Allow auto-merge**, in the repository's settings. Without it,
  `platformAutomerge` falls back to Renovate merging through the API — which
  still works, but waits for its next run rather than landing the moment the
  checks go green.
- **Dependabot alerts**, under Code security. Renovate reads GitHub's
  vulnerability alerts rather than keeping its own feed, so this is what makes
  an advisory open a fix pull request the day it is published instead of on the
  15th. It matters here because `cargo audit --deny warnings` turns every
  *other* pull request red as soon as an advisory lands, and the fix should
  already be in flight by then.

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

### Who can reach it

`mcp.diffpack.io` exists because of a protection setting, not because a short
name is nicer. The project has Vercel Authentication on Standard Protection
(`ssoProtection: all_except_custom_domains`), which answers an unauthenticated
request with an HTML login page — where an MCP client expects JSON-RPC. The
setting exempts production domains, so attaching a custom domain fixes it
without weakening anything else. `diffpack.io` is on Vercel's own nameservers
in the same team, so the DNS record was configured automatically; there was no
registrar step.

Measured against the live project rather than inferred from the setting's
name:

| URL | Reachable without a Vercel session |
| --- | --- |
| `mcp.diffpack.io` | **yes** — this is the point |
| `diffpack-server.vercel.app` | **yes** — the production alias is a production domain, so Standard Protection exempts it too |
| `diffpack-server-<hash>-philfreshmans-projects.vercel.app` | no — redirects to `vercel.com/login` |
| `diffpack-server-git-<branch>-philfreshmans-projects.vercel.app` | no — redirects to `vercel.com/login` |

The second row is worth knowing: the server has two public front doors, not
one. That is not a hole — everything here is public package content and the
server is deliberately unauthenticated, the same posture `diffpack` itself
takes — but the rate limiting in
[#26](https://github.com/philfreshman/diffpack-server/issues/26) has to cover
both hostnames, and "it is only on a `.vercel.app` URL" is not a reason to
treat something as unreachable.

## Using the server

`/mcp` is a Streamable HTTP endpoint. It is stateless by design — revision
`2026-07-28` removed protocol-level sessions and the `initialize` handshake,
which suits a serverless function that has no warm process to hold one in —
and it answers clients back to `2025-11-25` as well. `POST` only: `GET` and
`DELETE` are `405`, and no answer ever carries an `Mcp-Session-Id`.

A client can connect, list tools and call any of the eight there are:
`search_packages`, which answers a query with the packages a registry has
that match it — npm and crates.io hits carry a version and a description, and
PyPI hits carry a name alone, because the index PyPI publishes has nothing
else in it; `resolve_archive_url`, which returns the URL a package version's
archive is served from without fetching anything; `list_package_versions`,
which lists what a package has published with the date of each and whether it
is a preview, most recently published first rather than by version number;
`list_package_files`, which fetches that archive and lists the paths inside
it with the top-level directory stripped; `get_file_content`, which returns
one of those files, saying when it had to cut one short and when the bytes
were not valid UTF-8; `diff_package_versions`, which compares two versions
and answers with how much changed, the files that changed most, and a handle
the tools that read the diff back take; `get_diff_tree`, the first of
those, which takes that handle and lists the comparison's files and
directories a page at a time — one directory's subtree, one depth, one set
of statuses, since most of a package is unchanged between two versions and
paging through that is a call spent on what did not happen; and
`get_file_diff`, which takes the same handle and a path and returns that
file's diff, with three lines of unchanged context around each change rather
than the whole file, and `isDiff: false` where the answer is a file rather
than a patch. The resources that read a diff back arrive with
[#16](https://github.com/philfreshman/diffpack-server/issues/16).

Claude Code:

```bash
claude mcp add --transport http diffpack https://mcp.diffpack.io/mcp
```

Codex, in `config.toml`:

```toml
[mcp_servers.diffpack]
url = "https://mcp.diffpack.io/mcp"
```

Both point at production, which serves `main` — so they work once the branch
carrying `/mcp` has been promoted, and answer `404` before that. `/health` is
the route that tells you which build is serving:

```bash
curl https://mcp.diffpack.io/health
```

### Browser clients

Every client above is a local process and sends no `Origin`, which is what
this server is configured for: origin validation is on, the allowed list is
empty by default, and a request carrying any `Origin` is refused with `403`. A
request with none is served.

`DIFFPACK_ALLOWED_ORIGINS` opens it, as a comma-separated list, so letting a
browser-based client in is a deployment decision rather than a release:

```
DIFFPACK_ALLOWED_ORIGINS=https://app.example,https://staging.app.example
```

`Host` validation is deliberately off. It is rmcp's DNS-rebinding defence and
it is aimed at a server on a developer's own machine; on a public deployment
it defends nothing — an attacker's page sends the correct `Host` by fetching
the real URL — while its default loopback list would reject every deployment
we have.

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
