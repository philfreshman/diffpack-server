# Operations

How this server is deployed, who can reach it, and how its dependencies are
updated. The settings live in `vercel.json` and `renovate.json5`; this page says
why they are set the way they are.

## Deployment

Only a push to `main` deploys, to production. Nothing else deploys: not
`development`, not a branch, not a pull request.

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
