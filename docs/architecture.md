# The shape of this crate

Where code goes, and what it may import. This is a map, not a tour: if you are
deciding which module a new function belongs in, the answer is here, and the
reasoning behind each seam is in [`adr/`](adr/).

[`CONTEXT.md`](../CONTEXT.md) defines the nouns. This document places them.

## The module map

```
api/mcp.rs          entry point: wraps the router in VercelLayer
src/router.rs       routes, panic guard over the transport, origin config
src/mcp.rs          the ServerHandler: identity, capabilities, dispatch
src/tools/          one module per tool: definition() and call() together   #41
src/registry.rs     what a registry is: npm, crates, pypi (go later)        #42
src/archive/        fetch(registry, package, version) -> FileMap            #10
src/store/          DiffStore: get(&DiffKey) / put(entry)                   #20 #21 #22
src/page.rs         pagination and the 4.5 MB response ceiling              #43
src/cache_key.rs    DiffKey, diff_id, blob paths — docs/cache-key.md
src/error.rs        Failure, the two channels, redaction
src/engine.rs       the only importer of diffpack_engine
src/health.rs       the /health body
```

An issue number means the module arrives with that issue. Everything without
one is in the tree today.

## The import rules

Three, and all three are enforced rather than trusted. `./scripts/checks.sh
seams` runs the first two; `cargo test` runs the third.

**`src/engine.rs` is the only importer of `diffpack_engine`.** Everything the
server needs from the engine is re-exported there, so a bump to a new engine
release is one file to change and one file to read.
[`scripts/check-engine-seam.sh`](../scripts/check-engine-seam.sh) fails the
build otherwise. See [ADR 0007](adr/0007-one-importer-of-the-engine.md).

**A tool module goes through the seams, not around them.** A module under
`src/tools/` may import the standard library, the MCP and serialisation
crates, and `crate::{archive, cache_key, engine, error, page, registry,
store}`. It may not name an HTTP client or the blob store: those are
`archive`'s and `store`'s business, and eight tools that each know how to
fetch is eight places to fix a timeout.
[`scripts/check-tool-seams.sh`](../scripts/check-tool-seams.sh) fails the build
otherwise, and its allow-list is the list above.

**`docs/cache-key.md` is normative, not descriptive.** The cache key is a
contract with a TypeScript implementation (#27) that will never share a line of
code with this one, so the document and `fixtures/cache-key-vectors.json` are
the source of truth and `src/cache_key.rs` is held to them. When the code and
the document disagree, the document wins.

## What sits behind each interface

### `api/mcp.rs` — the deployed function

Vercel's Rust runtime wants a `[[bin]]` per handler under `api/`, and
`vercel.json` rewrites every path to this one. All it does is wrap
`router::router()` in `VercelLayer` and hand it to the runtime. It stays this
thin because a test can reach a `Router` and cannot reach a process: anything
with a decision in it belongs on the other side of that layer.

### `src/router.rs` — every route this function serves

Routing is this crate's job rather than the platform's, because `vercel.json`
sends `/(.*)` here. The module owns the route table, the panic guard over the
transport (the half of a request that runs before `rmcp` hands off to a
handler), the `X-Accel-Buffering` rule for event streams, and where the allowed
browser origins come from. `router_with` takes the handler factory and the
origin list as parameters, which is the seam the transport tests drive.

### `src/mcp.rs` — what this server says it is

The `ServerHandler`: identity, capabilities, protocol revisions, and the sorted
tool list. It collects tools; it does not describe them — a tool's definition
lives with its handler under `src/tools/` ([ADR
0002](adr/0002-one-module-per-tool.md)). `Guarded` is also here: the wrapper
that turns a panic inside a handler into a JSON-RPC error rather than a hung
request, which has to be inside the handler because `rmcp` runs handlers on a
task of their own.

### `src/tools/` — one module per tool

One file per MCP tool, holding its `Tool` definition and its handler together,
so that adding a tool is adding a file and reviewing a tool is reading one.
What a tool module does is: validate its parameters, ask `registry` or
`archive` or `store` for what it needs, shape an answer through `page`, and
fail through `error`. What it does not do is fetch, cache or paginate by hand.

### `src/registry.rs` — what a registry is

npm, crates.io and PyPI, described once: the identifier used in a cache key
(`npm`, `crates`, `pypi`), the name the registry calls itself in a message to a
model (`npm`, `crates.io`, `PyPI`), and how to reach it. The
`diffpack://registries` resource is a projection of this module rather than a
hand-written copy of it, and Go support (#28) is a value added here rather than
an edit in five places. See [ADR 0004](adr/0004-one-registry-module.md).

### `src/archive/` — a version's files

`fetch(registry, package, version) -> FileMap`. Everything on the other side of
that signature — resolving the download URL, the HTTP client, the timeout,
decompressing, untarring, stripping the top-level directory — is this module's
and nobody else's. Two adapters sit behind the same interface: the live one
over `reqwest`, and a fixture one reading local tarballs, which is what lets
the conformance suite (#24) run offline. See [ADR
0001](adr/0001-the-archive-seam-is-a-filemap.md).

### `src/store/` — cached diff results

`DiffStore`: `get(&DiffKey)` and `put(entry)`, a whole entry at a time. The
Vercel Blob client is the implementation behind it and is private to this
module, along with the 256 MB budget and the eviction that keeps it (#22). A
tool asks for a result and gets one or does not; how many HTTP calls that took
is not a tool's business. See [ADR 0003](adr/0003-the-cache-seam-is-a-store.md).

### `src/page.rs` — the response ceiling

Vercel's function response limit is 4.5 MB, and every tool that returns a list,
a tree or a patch set can exceed it. One module owns the ceiling, the cursor
format and the "this is a page of N" shape, so that there is one implementation
of staying under it rather than one per tool. See [ADR
0005](adr/0005-one-module-owns-the-response-ceiling.md).

### `src/cache_key.rs` — `DiffKey`, `diff_id`, blob paths

The implementation of `docs/cache-key.md`, which is normative. The canonical
string is built by concatenation rather than by serialising a struct: the field
set is closed and the order is fixed, so a `serde` upgrade cannot quietly
reorder it. Read the document before changing anything here.

### `src/error.rs` — which channel a failure reaches the client on

MCP has two, and conflating them is the usual mistake: a protocol error the
model never sees, and a tool error that is a *successful* response carrying
`isError: true`, which the model reads and can act on. `Failure::respond`
returns exactly a tool handler's type, so a handler that ends in
`failure.respond()` cannot put a failure on the wrong channel by accident.
Redaction over anything that leaves the process lives here too.

### `src/engine.rs` — the one importer of `diffpack-engine`

Re-exports what the server uses and names the pinned version, which is a field
in the cache key rather than a label. The server computes diffs with the same
code the browser runs; re-implementing any of it would mean two copies of an
output format that has to stay byte-identical.

### `src/health.rs` — the `/health` body

The one route that needs no MCP client. It names the build that answered, so a
deploy that silently served the previous binary is visible rather than
inferred.

## The decisions behind this

[`adr/`](adr/) records them, each with the alternative that was rejected. The
rejected alternative is the part that stops a future review re-suggesting it.
