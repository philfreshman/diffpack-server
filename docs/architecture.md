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
src/tools/          one module per tool: definition and handler together
src/registry.rs     what a registry is: npm, crates, pypi (go later)
src/archive/        fetch(registry, package, version) -> FileMap
src/catalogue/      versions(registry, package) -> Vec<Version>, newest first
src/fetch.rs        the one HTTP client: user agent, timeout, redirects, cap
src/store/          DiffStore: get(&DiffKey) / put(entry)                   #20 #21 #22
src/page.rs         the 4.5 MB response ceiling: pages, and cut blobs
src/handle.rs       the diff handle: mint, encode, decode, verify
src/cache_key.rs    DiffKey, diff_id, blob paths — docs/cache-key.md
src/error.rs        Failure, the two channels, redaction
src/log.rs          one line per tool call: what, how long, how it ended
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
crates, `futures` for the one case where a tool waits on two fetches at once,
and `crate::{archive, cache_key, catalogue, engine, error, handle, page,
registry, store}`. It may not name an HTTP client or the blob store: those
are `fetch`'s and `store`'s business, and eight tools that each know how to
fetch is eight places to fix a timeout.
[`scripts/check-tool-seams.sh`](../scripts/check-tool-seams.sh) fails the build
otherwise, and its allow-list is the list above.

`src/tools/mod.rs` is exempt from the import half of that rule, because it is
the one file there that is not a tool: it is the collection, the `Ctx`, and
the dispatch, and those need what a tool must not have — `crate::log`, so that
the one line per call is written once by the dispatch rather than nineteen
times by the tools that remembered. That exact path and no other: a tool is
free to grow into a directory, and `src/tools/thing/mod.rs` is then a tool
like any other. The name deny-list still covers the collection, so the
exemption is from the list and not from the rule.

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

One file per MCP tool, holding its definition and its handler together, so
that adding a tool is adding a file and reviewing a tool is reading one. What
a tool module does is: ask `registry` or `archive` or `store` for what it
needs, shape an answer through `page`, and fail through `error`. What it does
not do is fetch, cache or paginate by hand.

A tool writes down types rather than JSON. The `Tool` trait's associated
`Args` and `Output` generate the input schema, the output schema and the
structured answer, so a schema cannot disagree with the handler beside it; the
description and the three behaviour hints are required associated items, so a
tool that omits one does not compile. `mod.rs` holds the interface, the one
list of tools, and the generic call path where arguments are validated and
`Failure` is put on its channel — which is why a handler returns
`Result<Output, Failure>` and never names a result type of MCP's.

`Ctx` is what one request carries, built once by the service factory
`router::router_with` takes and cloned into every call. For a handler that
means the seams it may reach: `Archive` and `Catalogue` today and `DiffStore`
(#20) beside them, while `registry`, `page` and `handle` are named directly
because a pure module has nothing to hand over. Beside them it carries what
the dispatch needs and a handler never touches — the log's `Sink`, and the
`Spent` that the phases of one call add up in. `Ctx::archive()` hands back the
archive seam with that stopwatch already on it, so a fetch cannot go uncounted
and a handler's call is unchanged.

That factory is also the seam the suite drives: a test builds a `Ctx` over the
fixture adapters and a capturing sink, and reaches both through the path
production takes rather than around it.

It is built two ways and only two: `Ctx::new` is every seam live and
`Ctx::fixture` is every seam reading from the checked-in sets under
`fixtures/`. Both name every field, so a seam added later is a compile error
in each of them and its author answers for production and for the suite at
once. There is deliberately no builder that supplies one seam and fills the
rest, because filling them meant filling them live: a test naming the archive
carried a live catalogue beside it, and the first tool to read a version list
through such a context would have asked npm from CI. `tests/ctx.rs` is what
holds this — it drives every seam a context carries rather than a tool.

`Ctx::logging_to` is not a third way and shows what a fourth would have to
look like: it takes `self` and spreads `..self`, so it changes a context that
has already chosen its world rather than filling in the half it was not
given. A spread of `..Self::new()` is the shape that reopens this.

### `src/registry.rs` — what a registry is

npm, crates.io and PyPI, described once: the identifier used in a parameter and
in a cache key (`npm`, `crates`, `pypi`), the name the registry calls itself in
a message to a model (`npm`, `crates.io`, `PyPI`), where a version's archive is,
where versions and search come from, the hosts it may be reached at, and the
name rules an agent would otherwise guess at. Every per-registry fact is a
`match` over the three variants in this one file, so the compiler enumerates
what a fourth registry owes.

Two things are *derived* here rather than written down beside it. The `registry`
enum in every tool's schema is generated from the variant list, so a registry
cannot reach a model's schema late. And the outbound host allowlist is the hosts
of the URLs this module builds — `archive` may fetch what `registry::allows`
permits and nothing else — so a source added here is reachable the moment it exists,
rather than through a second list someone has to remember to widen.

It fetches nothing. This module says *where* and *what shape*; `archive` does
the fetching, and that is what lets every registry fact be tested with no
network at all. The `diffpack://registries` resource (#16) is a projection of
this module rather than a hand-written copy of it, and Go support (#28) is a
value added here rather than an edit in five places. See [ADR
0004](adr/0004-one-registry-module.md).

### `src/archive/` — a version's files

`fetch(registry, package, version) -> FileMap`. Everything on the other side of
that signature — resolving the download URL, PyPI's second hop, the HTTP
client, the timeout, the size cap, decompressing, untarring, stripping the
top-level directory — is this module's and nobody else's. Two adapters sit
behind the same interface: the live one over `reqwest`, and a fixture one
reading `fixtures/archives/`, which is what lets the suite assert what a
version's files are with no network and what lets the conformance suite (#24)
run offline. See [ADR 0001](adr/0001-the-archive-seam-is-a-filemap.md).

Three things are the same code for both adapters rather than the live one's
alone, because each is a rule about what this server does rather than about
where bytes come from: the host allowlist `registry` derives, the size cap,
and extraction. The fixture index is keyed by URL for the same reason — a
fetch path that built a URL of its own instead of asking `registry` finds
nothing there, so `tests/archive.rs` is a test of resolution as well as of
extraction.

The index has a third answer beside "here are the bytes" and "this URL is not
in the set": `null`, meaning the registry serves nothing there. It is the
offline suite's way of reaching the path a `404` takes, and the refusal it
produces is built by the same constructor the live adapter uses, so the two
cannot disagree about what a missing version reads like.

Redirects are followed only while they stay on allowed hosts. A `302` is a
request to wherever it points, so the alternative is an allowlist whose holes
the registry chooses.

### `src/catalogue/` — what a package has released

`versions(registry, package) -> Vec<Version>`, newest first. The second seam
over the network, beside `archive` and shaped the same way: one interface, two
adapters, and a fixture set keyed by the URL `registry` builds.

It is not part of `archive` because that seam *is* a `FileMap` ([ADR
0001](adr/0001-the-archive-seam-is-a-filemap.md)) and this is none of it: a
document that is read rather than extracted, about a package rather than one
version of one, and never cached.

**Newest first means most recently published first**, and the date it is
computed from is the only thing in these documents that can produce the
order. Not a direction to read the document in: deps.dev sorts PyPI's versions
lexically by version string, so `requests` ends at 2.9.2 and reversing it
announces a 2016 release as the newest; and npm's own order is gone before
this crate sees it, because its versions are a JSON object and `serde_json`'s
map here is a `BTreeMap`. A version the source gives no date for — one of
`requests`' 161, thirty-seven of `numpy`'s — is still a published version, so
it is listed last rather than dropped.

Nothing here is cached. The 256 MB budget is for diff results, and a package's
version list goes stale the moment somebody publishes.

### `src/fetch.rs` — the one HTTP client

Every outbound request this server makes. It was `archive`'s until `catalogue`
needed one too, and two copies would be two places to fix a timeout, a user
agent or a redirect policy — which is the thing ADR 0001 argued against in the
first place. What leaves this module is bytes or a `Failure`, never a status
code and never a `reqwest` type, so the seams above it stay the only things a
tool sees.

A caller passes the two refusals that differ between seams rather than this
module guessing them. A `404` is a missing *version* to `archive` and a
missing *package* to `catalogue`, and a body over the cap is named after
whichever of the two was being read — an archive has a smaller thing to ask
for instead and a version list does not.

### `src/store/` — cached diff results

`DiffStore`: `get(&DiffKey)` and `put(entry)`, a whole entry at a time. The
Vercel Blob client is the implementation behind it and is private to this
module, along with the 256 MB budget and the eviction that keeps it (#22). A
tool asks for a result and gets one or does not; how many HTTP calls that took
is not a tool's business. See [ADR 0003](adr/0003-the-cache-seam-is-a-store.md).

### `src/page.rs` — the response ceiling

Vercel's function response limit is 4.5 MB, and every tool that returns a list,
a tree, a file or a patch set can exceed it. One module owns the ceiling, the
cursor format and the "this is a page of N" shape, so that there is one
implementation of staying under it rather than one per tool. See [ADR
0005](adr/0005-one-module-owns-the-response-ceiling.md).

Two interfaces, because a tool's answer comes in two shapes. `paginate` takes
a sequence and returns a `Page`: the items that fit, the next cursor, and the
total. `truncate` takes one blob — a file's content, a file's diff — and
returns an `Excerpt`: as much as fits, a marker saying it was cut, and the
whole thing's real byte count. Truncation lives here rather than in a module
of its own because what the two share is the subtle part and what they differ
in is one field; ADR 0005 records the choice and its cost.

`limit`, `cursor` and `max_bytes` are types this module owns — `page::Limit`,
`page::Cursor` and `page::MaxBytes` — rather than two numbers and a string a
tool describes for itself. They are the only part of the ceiling an agent ever
sees, so the default, the range, the "passed back unchanged" rule and the
"this can only ask for less" rule are written into their schemas and every
tool inherits them by naming the type. `Cursor` deserialises by decoding, so a
cursor that is not ours is `-32602` before a handler runs, the same way a
`DiffHandle` is.

The `Excerpt` a blob-shaped tool returns is flattened into that tool's own
output, so `text`, `truncated` and `bytes` are fields of the answer rather
than a nested object. Their descriptions reach a model that way, which is why
they are written for that reader.

Not every answer is one of the two. `diff_package_versions` returns a
Summary: totals over the whole comparison, and a fixed-size sample of the
files that moved most. It does not reach this module, and that is the rule
rather than an exception to it — a Page and an Excerpt exist because an
answer's size follows from its subject, and a Summary's does not. Twenty
entries is twenty entries whether the pair moved one file or nine thousand,
so there is no ceiling to stay under and nothing for a cursor to resume.
The tree it samples from *is* subject to both, which is why walking it is
`get_diff_tree`'s job (#14) and not this one's.

Three things a caller does not do: count bytes, encode a cursor, or decide
what "too big" means. The ceiling is on *serialised* bytes and is a third of
the platform's cap, because `tools::invoke` puts a tool's answer on the wire
twice — as `structuredContent` and as the escaped text block rmcp mirrors it
into. A tool that counted items instead would be correct until someone diffed
a package whose paths are long.

### `src/handle.rs` — the handle a diff is asked for again by

What passes between the tool that computes a diff (#13) and the three that
read one back (#14, #15, #16). It carries the `diff_id` — the cache lookup,
and the string #27 needs — and beside it the inputs that `diff_id` was minted
from, so that a reading tool whose entry has been evicted recomputes rather
than refusing. See [ADR 0006](adr/0006-the-handle-carries-its-inputs.md).

It travels as one opaque string. `DiffHandle` serialises as that string and
deserialises by decoding it, which is what makes a tool's `-32602` automatic:
`tools::invoke` reads a handler's `Args` before the handler runs, so a handle
that does not decode never reaches one and no handler has to remember to
verify it.

The module sits beside `cache_key` rather than inside it. The two answer
different questions — `cache_key` implements a document that is a contract
with another language, and this is a wire format between two of this server's
own tools — and keeping the second out of the first is what makes "#44 does
not touch `docs/cache-key.md`" a property of the tree rather than a promise.

This is also where `similarity_threshold` and `ignore_whitespace` stop being
arguments. They are fixed by the handle, so the reading tools do not accept
them: two option sets are two diffs, and a tool that let one be changed after
the fact would be answering about a diff nobody computed.

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

### `src/log.rs` — one line per tool call

What an incident is read from. The questions asked when this server has
misbehaved are always the same — which tool ran, what was it asked for, where
did its time go, how did it end — so the answer is one structured line per
call rather than prose in whichever handler wanted it.

The line is written by `tools::call`, not by a tool. A tool that emitted its
own would not fail to compile, and the gap would be invisible until the call
nobody logged was the one being looked for. That is also why `crate::log` is
not on `check-tool-seams.sh`'s list: `src/tools/mod.rs` is exempt from the
import rule because it is the collection rather than a tool, and a tool still
cannot reach the module.

A `Line` is built before it is written, which is what makes the line
testable: `Sink` has a variant that keeps lines in memory, a `Ctx` carries
one, and `tests/log.rs` reads back the line a real `tools/call` produced
rather than one a test built.

Redaction is `error::redact`'s, so what must not reach an operator and what
must not reach a model are one definition. Argument values are redacted and
*then* cut, in that order: a signed URL cut at a hundred characters loses its
`?` and stops looking like one.

Where a call's time goes is accumulated in `Spent`, which a `Ctx` holds for
the length of one request and the seams write into. `Ctx::archive()` hands
back the archive seam with the stopwatch already on it, so a handler is
unchanged and there is no way to read an archive that goes uncounted.

`Spent` keeps the *window* fetching spanned rather than the sum of each
fetch's duration, because `diff_package_versions` asks for two versions
through one `try_join!` and a sum reports a thousand milliseconds where the
call waited five hundred — in the field directly beside `total`, which a
reader compares it against and which it could then exceed. It is also the one
thing about this module a call over the wire cannot show: the fixture
archives answer in under a millisecond, so `tests/log.rs` builds two
overlapping spans by hand against `Spent::starting_at`.

Two phases today. The finer split #26 asks for — download, extract, diff,
store — needs each of the four to be something this crate can time, and none
of them is: download and extract are one interface by ADR 0001, the diff is a
synchronous call inside `src/engine.rs` which by ADR 0007 has no reach into a
request, and there is no store until #20. Each is a decision about a seam
rather than a field to add.

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
