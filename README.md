# diffpack-server

An MCP server that lets an agent search npm, crates.io, PyPI, Go modules and
Packagist (Composer, for PHP), read the files of a published package version, and diff two versions. It runs
[`diffpack-engine`](https://github.com/philfreshman/diffpack-engine) natively,
the same code [diffpack.io](https://www.diffpack.io) runs in the browser.

The endpoint is `https://mcp.diffpack.io/mcp`, over Streamable HTTP, with no
sign-in and no API key.

## Connect a client

### Claude Code

```bash
claude mcp add --transport http diffpack https://mcp.diffpack.io/mcp
```

To share it with a project instead, add `--scope project`, which writes this
entry to the project's `.mcp.json`:

```json
{
  "mcpServers": {
    "diffpack": {
      "type": "http",
      "url": "https://mcp.diffpack.io/mcp"
    }
  }
}
```

Verified against production with Claude Code 2.1.299, using this entry as an
`--mcp-config` file.

### Claude Desktop

Documented, not yet verified.

Add it as a custom connector: **Customize → Connectors → + Add → Add custom
connector**. Use the name `diffpack` and the URL
`https://mcp.diffpack.io/mcp`, then choose **No sign in**. On a Team or
Enterprise plan, an Owner adds it under **Organization settings →
Connectors**, and members connect from **Customize → Connectors**.

### Codex

Documented, not yet verified.

```bash
codex mcp add diffpack --url https://mcp.diffpack.io/mcp
```

No login is needed, so skip `codex mcp login`. Or add it to
`~/.codex/config.toml` by hand:

```toml
[mcp_servers.diffpack]
url = "https://mcp.diffpack.io/mcp"
```

## Development

See [`CLAUDE.md`](CLAUDE.md).
