# Operations

How this server is deployed and released, who can reach it, and how its
dependencies are updated. The settings live in `vercel.json`, `.releaserc.json`
and `renovate.json5`; this page says why they are set the way they are.

## Deployment

Only a push to `main` deploys, to production. Nothing else deploys: not
`development`, not a branch, not a pull request. Code reaches `main` only by
merging `development` into it, which is also what makes a release (see
[Releases](#releases)).

`vercel.json` holds only what Vercel cannot work out by itself:

- `rewrites` sends every path to `api/mcp.rs`. Without it, Vercel's Rust runtime
  serves only `/api/mcp`, and `/mcp` and `/health` are 404s that never reach the
  router.
- `functions.maxDuration` is the only place a Rust function's timeout can be
  set.
- `git.deploymentEnabled` is the branch policy. Keep `"**": false`. Vercel
  deploys any branch no rule turns off, so `{"main": true}` alone deploys every
  branch. The wildcard looks redundant and is not.

Every deploy builds the crate, `diffpack-engine` and `vercel_runtime` from
scratch, because there is no Cargo cache between deploys and the release profile
uses fat LTO. Expect slow deploys; the measured cold build time is in
[#8](https://github.com/philfreshman/diffpack-server/issues/8).

`tests/deploy.rs` checks `vercel.json` against the crate layout, since nothing
compiles against it. Renaming the `[[bin]]` or moving `api/mcp.rs` would
otherwise only show up as a 404 after a deploy.

## Releases

`main` only takes pull requests from `development`, merged with a merge commit.
The `main source is development` check
([`.github/workflows/main-source.yml`](../.github/workflows/main-source.yml))
fails a pull request to `main` from any other branch or from a fork, and it is
required on `main`.

Each merge into `main` runs
[`.github/workflows/release.yml`](../.github/workflows/release.yml), which uses
[semantic-release](https://semantic-release.gitbook.io/) to turn the commit
messages since the last `v*` tag into a version, a tag and a GitHub release with
notes grouped by type:

| Commits since the last release | Next version |
| :--- | :--- |
| a `feat!:` / `fix!:` or a `BREAKING CHANGE:` footer | major (`v2.0.0`) |
| at least one `feat` | minor (`v1.1.0`) |
| at least one `fix`, `perf` or `revert` | patch (`v1.0.1`) |
| only `chore`, `docs`, `style`, `refactor`, `test`, `build`, `ci` | no release |

This is why the commit types matter. A commit message is
`type(scope): description`, with `type` one of `build`, `chore`, `ci`, `docs`,
`feat`, `fix`, `perf`, `refactor`, `revert`, `style` or `test`, and the scope
optional. The `conventional commits` CI job fails a pull request with a commit
that does not follow it, since such a commit would be left out of the version
and the notes.

Nothing is committed back to `main`, so `version` in `Cargo.toml` stays as it
is and is not the release number. The release settings live in
[`.releaserc.json`](../.releaserc.json). The release is a tag and notes only:
the deploy is still Vercel's own build of the same push.

## The cache's credentials

The diff cache is a private Vercel Blob store, `store_MckmSOFEWm4x66fo`,
connected to the project. The project holds `BLOB_STORE_ID` and no
`BLOB_READ_WRITE_TOKEN`: the store is reached with the OIDC token Vercel issues
the deployment, so no long-lived secret is stored.

That token is in the environment as `VERCEL_OIDC_TOKEN` only at build time. A
running function is given it with every request, as the `x-vercel-oidc-token`
header, so `src/store/blob.rs` takes the header first and the environment
second, the way `@vercel/oidc` does. A locally run server has no such header,
and uses `VERCEL_OIDC_TOKEN` from `vercel env pull`, or a
`BLOB_READ_WRITE_TOKEN`.

The store is built for each tool call and resource read from that request's
headers, and uses the token for the write it makes before the answer. Vercel's
documentation does not say whether a header of that name sent by a client
survives to the function. Either way, a forged one can only fail: the Blob API
verifies the token and scopes it to this project, the store is built per
request so the token reaches no other caller's cache access, and a store
failure never fails a call. It is a miss.

Because the store is private, every write declares `private` access, and a read
presents the token on both of its requests, including the download.

A deployment with neither credential has no store, and every call recomputes.
The function's log line says so as `"cache":"no_store"`. Before
[#130](https://github.com/philfreshman/diffpack-server/issues/130), every
production request was that.

## Who can reach it

The Vercel project has Vercel Authentication on Standard Protection, which
answers an unauthenticated request with a login page instead of JSON-RPC.
Production domains are exempt, which is why the server is on the custom domain
`mcp.diffpack.io`.

That makes two hostnames public: `mcp.diffpack.io` and the production alias
`diffpack-server.vercel.app`. Per-deployment and branch URLs on `*.vercel.app`
redirect to the Vercel login. Two public hostnames are fine for an
unauthenticated server of public package content, but a rate limit added later
has to cover both ([ADR 0018](adr/0018-no-rate-limit-for-now.md)).

## Protocol conformance

The `MCP conformance` job runs
[`scripts/conformance.sh`](../scripts/conformance.sh): it starts
[`examples/serve.rs`](../examples/serve.rs), the real router over the fixture
archives on a loopback port, and drives it with the
[MCP Inspector](https://github.com/modelcontextprotocol/inspector) CLI as a
`2026-07-28` and as a `2025-11-25` client, with `curl` for what the Inspector
cannot send. The Inspector and ajv are pinned with a lockfile in
[`conformance/`](../conformance), and `npm ci` installs them without running
install scripts. Every tool the server lists needs an example call in
`conformance/calls.json`, or the job fails.

The `cargo test` suite asserts most of the same things in-process. This job
asks them again over a real socket, through the TypeScript SDK and the schema
validator a JavaScript client actually uses, rather than through this crate's
own test client.

## Dependency updates

[Renovate](https://docs.renovatebot.com), a GitHub App configured by
[`renovate.json5`](../renovate.json5), opens the pull requests against
`development`. It runs on the 1st and 15th of the month, the same days as the
sibling repos, and sweeps `Cargo.lock` for transitive updates on the 1st.
Security fixes open immediately.

Nothing younger than three days is proposed. This repo is stricter than the
other two because it is the one place in diffpack that downloads and unpacks
package archives on a server, so its dependency graph is attack surface, and a
malicious release is usually caught and yanked within hours to days.

Minor and patch updates and the `Cargo.lock` sweep merge on their own once the
required checks pass. These wait for a person:

- A major. Passing checks prove it compiles, not that it is the version we want.
- `rust-toolchain.toml`. A new toolchain brings new clippy lints, which
  `-D warnings` turns into failures, and it changes what Vercel builds with.
- The conformance tools in `conformance/package.json`. The MCP Inspector is the
  client the `MCP conformance` job judges this server by, so a new release can
  start failing a server that has not changed. Read what it now checks first.
- `diffpack-engine`. The pull request fails on purpose until `engine::VERSION`
  in `src/engine.rs` is moved to the new tag in the same branch. Every `diff_id`
  changes with it, so the cache starts cold. Read the engine's release notes for
  changes to rename detection, line counts or the unified-diff format before
  merging.

Three pairs are grouped so they always move together:

- `rmcp` and `schemars`, so that `JsonSchema` here is the trait rmcp asks for.
- `axum` and `vercel_runtime`, because vercel_runtime's axum integration decides
  which axum major this crate can use.
- `reqwest` and `rustls`. `src/fetch.rs` and `src/store/blob.rs` install the
  `ring` provider into rustls. If reqwest resolves a different rustls, every
  fetch fails at runtime while the offline suite stays green; only the networked
  tests notice.

To validate a change to the config before pushing it (needs Node):

```bash
npx --yes --package renovate@latest renovate-config-validator --strict
```

Three settings live on GitHub rather than in a file:

- The Renovate App is installed on the repository.
- **Allow auto-merge** is on. Without it, Renovate merges through the API on its
  next run instead of as soon as the checks pass.
- **Dependabot alerts** are on. Renovate reads them to open security fixes the
  day an advisory is published, which matters because
  `cargo audit --deny warnings` turns every other pull request red from that
  moment.
