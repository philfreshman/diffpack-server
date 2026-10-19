# CLAUDE.md

An MCP server on Vercel (`mcp.diffpack.io`) that runs `diffpack-engine` natively
to search, read and diff packages from npm, crates.io, PyPI, the Go module
proxy and Packagist. Read
[`CONTEXT.md`](CONTEXT.md) for the vocabulary,
[`docs/architecture.md`](docs/architecture.md) for which module owns what,
[`docs/adr/`](docs/adr/) for why, and [`docs/operations.md`](docs/operations.md)
for deploys and dependency updates.

## Commands

```bash
cargo test                                 # offline, reads fixtures/
cargo test --test <name>                   # one file, tests/<name>.rs
cargo test --test networked -- --ignored   # against the real registries
cargo test --lib -- --ignored store::blob  # against the real blob store
./scripts/checks.sh seams                  # module boundaries, no compile
./scripts/checks.sh conformance            # MCP Inspector against a local fixture server
./scripts/checks.sh                        # all seven checks, as CI runs them
./scripts/install-hooks.sh                 # once per clone: the pre-commit hook
```

Run the ignored tests after changing how the server talks to a registry or the
blob store; the offline suite cannot check that. `deny` and `audit` need network
access, and `conformance` needs Node 22.19 or later. Never skip the pre-commit
hook with `--no-verify`.

## Rules that are easy to break

- Only `src/engine.rs` may import `diffpack_engine`. Never reimplement engine
  logic here: a change to how a diff is computed belongs in `diffpack-engine`.
- Code under `src/tools/`, `src/resources/` and `src/prompts/` reaches the
  network and the cache only through the seam modules. The allow-list is in
  `scripts/check-tool-seams.sh` and repeated in `docs/architecture.md`; change
  both together.
- An engine bump moves the `tag` in `Cargo.toml` and `VERSION` in
  `src/engine.rs` together, or `tests/engine.rs` fails. The version is part of
  the cache key, so every `diff_id` changes and the cache starts cold.
- `docs/cache-key.md` and `fixtures/cache-key-vectors.json` define the cache key
  and `src/cache_key.rs` follows them. A TypeScript reader in `diffpack` relies
  on the vectors, so never regenerate them from the Rust code.
- Keep `"**": false` in `vercel.json`. Without it every branch deploys.

## Dependencies

This is the one place in diffpack that unpacks untrusted archives on a server,
so its dependency policy is the strictest of the three repos.

- Code comes only from crates.io and the `diffpack-engine` git repo, pinned to a
  tag (`[sources]` in `deny.toml`).
- The license allow-list in `deny.toml` is exactly what the graph uses. Add a
  license only when a dependency brings it.
- An advisory ignore goes in both `deny.toml`, as `{ id, reason }` saying why it
  does not reach this crate, and `.cargo/audit.toml`. Remove both once the fix
  lands.
- Bump `rmcp` with `schemars`, `axum` with `vercel_runtime`, and `reqwest` with
  `rustls`.

## Code style

Write self-explaining code in Clean Code style: intention-revealing names, small
functions that do one thing, named constants and types instead of magic values,
and extracted, well-named helpers instead of explanatory blocks.

Do not write comments, doc comments included, to explain what code does or why.
This covers scripts and config files too. If code seems to need a comment,
rename or restructure it until it does not. The why behind a change belongs in
the commit message or PR description; a design decision that meets the bar in
`docs/adr/README.md` gets a new ADR.

There are no exceptions: the crate has no `unsafe` and no lint that demands
docs. Text a client shows the model is not a comment, so a tool's `DESCRIPTION`
and `#[schemars(description = "…")]` stay; use the attribute, never `///`.
Silence a lint with `#[expect(lint, reason = "…")]`.

## Git

Branch from `development` and open pull requests against it. Only `main`
deploys, so a merge into `development` is not live. Use Conventional Commits.
