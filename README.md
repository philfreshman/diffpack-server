# diffpack-server

An MCP server that lets an agent search npm, crates.io and PyPI, read the files
of a published package version, and diff two versions. It runs
[`diffpack-engine`](https://github.com/philfreshman/diffpack-engine) natively,
the same code [diffpack.io](https://www.diffpack.io) runs in the browser.

## Connect

The endpoint is `https://mcp.diffpack.io/mcp`: Streamable HTTP, stateless, no
sign-in. In Claude Code:

```bash
claude mcp add --transport http diffpack https://mcp.diffpack.io/mcp
```

Other clients take the same URL. `https://mcp.diffpack.io/health` names the
build that is serving. A browser-based client is refused until its origin is in
`DIFFPACK_ALLOWED_ORIGINS`
([ADR 0009](docs/adr/0009-origin-validation-on-host-validation-off.md)).

## Tools and resources

- `search_packages`: find packages from a partial name.
- `resolve_archive_url`: where a version's archive is served from, without
  fetching it.
- `list_package_versions`: a package's releases, newest first, and which one the
  registry installs by default.
- `list_package_files`: the files in a published version, a page at a time.
- `get_file_content`: one file from a published version.
- `diff_package_versions`: compare two versions; answers with totals, the files
  that changed most, and a handle.
- `get_diff_tree`: a comparison's files and directories, a page at a time.
- `get_file_diff`: one file's diff from a comparison.
- `diffpack://registries`: how each registry names packages and where its data
  comes from.
- `diffpack://diff/{handle}` and `diffpack://diff/{handle}/file/{path}`: a
  comparison, or one file of it, to read rather than call.

## Development

```bash
./scripts/install-hooks.sh   # once per clone
./scripts/checks.sh          # fmt, seams, clippy, deny, audit, test
```

[`CLAUDE.md`](CLAUDE.md) has the commands and the rules that are easy to break.
[`CONTEXT.md`](CONTEXT.md) defines the vocabulary,
[`docs/architecture.md`](docs/architecture.md) maps the modules,
[`docs/adr/`](docs/adr/) records the decisions, and
[`docs/operations.md`](docs/operations.md) covers deployment and dependency
updates. Branch from `development`; only `main` deploys.
