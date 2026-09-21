# 0009. `Origin` validation on, `Host` validation off

**Status:** accepted, 2026-09-17 (#6). Recorded here in 2026-09-21.

Two of `rmcp`'s defences point at the same attack and only one of them applies
to a public deployment, so the pair is configured deliberately rather than
left at its defaults.

`Origin` validation is **on**, and enforced even though the allowed list is
empty by default. With no entries, a request carrying any `Origin` is refused
with `403` and a request carrying none is served — which is exactly the shape
of this server's clients, since Claude Code, Claude Desktop and Codex are local
processes that send no `Origin`. Letting a browser-based client in is a
deployment decision: `DIFFPACK_ALLOWED_ORIGINS`, comma separated, no release
needed.

`Host` validation is **off**. It is rmcp's DNS-rebinding defence and it is
aimed at a server on a developer's own machine, where rejecting a `Host` that
is not loopback is what stops a rebound name reaching it.

## Rejected: leaving `Host` validation at its default

It would have to be configured either way, because the default loopback list
rejects every deployment this project has — `mcp.diffpack.io`,
`diffpack-server.vercel.app` and every preview URL. So the choice was a
hostname allow-list or nothing.

Nothing, because on a public deployment the check defends against nothing. An
attacker's page sends the correct `Host` by fetching the real URL; the header
is not a claim anyone has to forge. What it would add is a second list of
hostnames to keep in step with the domains the project actually serves, whose
failure mode is a `403` on a URL that was just attached — a defence that costs
an outage and buys nothing.

`Origin` is the one that applies, because the thing being kept out is a browser
page driving this server with a user's network position, and `Origin` is the
header a browser sets and a page cannot.
