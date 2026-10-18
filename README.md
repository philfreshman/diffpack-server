# diffpack-server

An MCP server that lets an agent search npm, crates.io and PyPI, read the files
of a published package version, and diff two versions. It runs
[`diffpack-engine`](https://github.com/philfreshman/diffpack-engine) natively,
the same code [diffpack.io](https://www.diffpack.io) runs in the browser.

```bash
claude mcp add --transport http diffpack https://mcp.diffpack.io/mcp
```

For development, see [`CLAUDE.md`](CLAUDE.md).
