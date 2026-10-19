# diffpack-server

An MCP server that lets an agent search npm, crates.io and PyPI, read the files
of a published package version, and diff two versions. It runs
[`diffpack-engine`](https://github.com/philfreshman/diffpack-engine) natively,
the same code [diffpack.io](https://www.diffpack.io) runs in the browser.

The endpoint is `https://mcp.diffpack.io/mcp`, over Streamable HTTP, with no
sign-in and no API key.

## Connect a client

### Claude Code

Verified against production with Claude Code 2.1.299.

```bash
claude mcp add --transport http diffpack https://mcp.diffpack.io/mcp
```

To share it with a project instead, put this in the project's `.mcp.json`:

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

### Claude Desktop

Documented, not yet verified.

Add it as a custom connector: **Settings → Connectors → Add custom
connector**, with the name `diffpack` and the URL
`https://mcp.diffpack.io/mcp`. Leave the OAuth fields empty.

### Codex

Documented, not yet verified.

```bash
codex mcp add diffpack --url https://mcp.diffpack.io/mcp
```

Or add it to `~/.codex/config.toml` by hand:

```toml
[mcp_servers.diffpack]
url = "https://mcp.diffpack.io/mcp"
```

## Development

See [`CLAUDE.md`](CLAUDE.md).
