# 0002. One module per tool

**Status:** accepted, 2026-09-21. Implemented by #41.

Each MCP tool is one module under `src/tools/`, holding its `Tool` definition —
name, description, input schema — and its handler together. `src/mcp.rs`
collects them and sorts them for `tools/list`; it does not describe them.

Phases 3 and 4 add eight tools. With this shape, adding one is adding a file
and a line to the collector, reviewing one is reading a file, and the per-tool
checklists that #23 (an agent can use it without docs) and #24 (conformance)
ask for have something to be per-tool *about*. A schema and the code that
validates against it sit within a screen of each other, which is the distance
at which they stay in agreement.

## Rejected: a definition list beside a dispatch match

The shape `rmcp` makes easiest: one `tools()` function returning every
definition, and one `call_tool` with a `match` over the names, both in
`src/mcp.rs`.

That is two lists that must agree, in two places, with nothing checking that
they do. The drift is silent in the worst direction: a tool described in
`tools/list` with no arm in the match answers "unknown tool" to a client that
was told it exists, and an arm with no definition is dead code no agent can
reach. A schema changed in one list and not in the other is worse again —
the call is accepted and the handler reads a field that is not there.

The cost is one indirection: the tool list is assembled rather than written
out, by a macro over one line per tool. That line declares the module as well
as registering it, so the sort is a property of the collection and there is no
order to add a tool in wrongly.
