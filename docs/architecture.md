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
src/resources/      one module per resource: URI and handler together
src/registry.rs     what a registry is: npm, crates, pypi (go later)
src/archive/        fetch(registry, package, version) -> FileMap
src/catalogue/      versions(registry, package) -> Versions, newest first
src/search/         hits(registry, query, limit) -> Vec<Hit>, best match first
src/fetch.rs        the registries' HTTP client: user agent, timeout, redirects, caps
src/store/          DiffStore: get(&DiffKey) / put(entry), inside a budget
src/page.rs         the 4.5 MB response ceiling: pages, and cut blobs
src/handle.rs       the diff handle: mint, encode, decode, verify
src/cache_key.rs    DiffKey, diff_id, blob paths — docs/cache-key.md
src/error.rs        Failure, the two channels, redaction
src/log.rs          one line per tool call: what, cache outcome, how long, how it ended
src/engine.rs       the only importer of diffpack_engine
src/health.rs       the /health body
```

An issue number means the module arrives with that issue. Every module in the
map is in the tree today.

## The import rules

Three, and all three are enforced rather than trusted. `./scripts/checks.sh
seams` runs the first two; `cargo test` runs the third.

**`src/engine.rs` is the only importer of `diffpack_engine`.** Everything the
server needs from the engine is re-exported there, so a bump to a new engine
release is one file to change and one file to read.
[`scripts/check-engine-seam.sh`](../scripts/check-engine-seam.sh) fails the
build otherwise. See [ADR 0007](adr/0007-one-importer-of-the-engine.md).

**A tool or a resource module goes through the seams, not around them.** A
module under `src/tools/` or `src/resources/` may import the standard library,
the MCP and serialisation crates, `futures` for the cases where one of them
waits on two fetches at once, and `crate::{archive, cache_key, catalogue,
engine, error, handle, page, registry, resources, search, store, tools}`. It
may not name an HTTP client or the blob store: those are `fetch`'s and
`store`'s business, and eight tools that each know how to fetch is eight
places to fix a timeout. Nothing checks that this paragraph and the script's
list agree, so a module added to one is added to the other by hand.

`resources` and `tools` are on that list for each other, which is why the two
directories name each other: a tool's answer carries a `resource_link` and so
has to know that resource's URI, and a resource is built out of the walks the
tools own. See [ADR 0014](adr/0014-a-resource-is-a-projection-of-the-tools.md).
[`scripts/check-tool-seams.sh`](../scripts/check-tool-seams.sh) fails the build
otherwise, and its allow-list is the list above.

`src/tools/mod.rs` is exempt from the import half of that rule, because it is
the one file under either directory that is not a tool or a resource: it is
the collection, the `Ctx`, and
the dispatch, and those need what a tool must not have — `crate::log`, so that
the one line per call is written once by the dispatch rather than nineteen
times by the tools that remembered. That exact path and no other: a tool is
free to grow into a directory, and `src/tools/thing/mod.rs` is then a tool
like any other. The name deny-list still covers the collection, so the
exemption is from the list and not from the rule. `src/resources/mod.rs` is a
collection too and deliberately does not get one: it needs nothing a resource
may not have, and an exemption granted before it is needed is a rule weakened
for free.

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

The `ServerHandler`: identity, capabilities, protocol revisions, the sorted
tool list, and the two lists a client discovers this server's resources
through. It collects; it does not describe — a tool's definition lives with
its handler under `src/tools/` ([ADR 0002](adr/0002-one-module-per-tool.md))
and a resource's URI lives with its handler under `src/resources/`. Which URI
resolves to what is `resources::read`'s, the way which name runs what is
`tools::call`'s. `Guarded` is also here: the wrapper
that turns a panic inside a handler into a JSON-RPC error rather than a hung
request, which has to be inside the handler because `rmcp` runs handlers on a
task of their own.

### `src/tools/` — one module per tool

One file per MCP tool, holding its definition and its handler together, so
that adding a tool is adding a file and reviewing a tool is reading one. What
a tool module does is: ask `registry`, `archive`, `catalogue`, `search` or
`store` for what it needs, shape an answer through `page`, and fail through
`error`. What it does not do is fetch, cache or paginate by hand.

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
means the seams it may reach: `Archive`, `Catalogue`, `Search` and
`DiffStore`, while `registry`, `page` and `handle` are named directly because
a pure module has nothing to hand over. Beside them it carries what the
dispatch needs and a handler never touches — the log's `Sink`, the `Spent`
that the phases of one call add up in, and the `Lookup` that says what the
call found in the store. `Ctx::archive()`, `Ctx::catalogue()` and
`Ctx::search()` hand back their seam with that stopwatch already on it, so a
wait on a registry cannot go uncounted and a handler's call is unchanged.

`Ctx::store()` hands back a wrapper of its own rather than that one, and the
difference is a decision rather than an omission: the `fetch` phase answers
how long a call waited on a *registry*, and a cache read counted towards it
would report the call that avoided two downloads as the one that waited
longest. What that wrapper records instead is whether the lookup found its
entry. It clones the handle where an entry is written rather than handing a
caller one to clone, because writing an entry outlives the call that produced
it — the work goes to `waitUntil` and the context is gone by the time it runs.

`Ctx::storing_in` is the third `..self` spread beside `Ctx::logging_to`, and
it is there for the two things a store can be that a fixture directory cannot
express: not there, and slow.

That factory is also the seam the suite drives: a test builds a `Ctx` over the
fixture adapters and a capturing sink, and reaches both through the path
production takes rather than around it. `tests/common/mod.rs` is the one
client that does the reaching — the SEP-2243 headers, the per-request `_meta`,
the protocol revision and the router over a supplied `Ctx`, in one spelling,
so that a suite says which context it is in and nothing else about the wire.

It is built two ways and only two: `Ctx::new` is every seam live and
`Ctx::fixture` is every seam reading from the checked-in sets under
`fixtures/`. Both name every field, so a seam added later is a compile error
in each of them and its author answers for production and for the suite at
once. There is deliberately no builder that supplies one seam and fills the
rest, because filling them meant filling them live: a test naming the archive
carried a live catalogue beside it, and the first tool to read a catalogue
through such a context would have asked npm from CI.

What the compiler checks there is that every field was answered for, not that
the answer was a fixture one, so the second half is `Ctx::seams`. It names the
seams by taking the struct apart, which is a compile error the moment a field
is added, and `tests/ctx.rs` drives what it returns rather than a list of its
own. Adding a seam is therefore three edits the compiler and the suite ask for
in turn: both constructors, the name, and the shortest call that reaches it.

`Ctx::logging_to` is not a third way and shows what a fourth would have to
look like: it takes `self` and spreads `..self`, so it changes a context that
has already chosen its world rather than filling in the half it was not
given. A spread of `..Self::new()` is the shape that reopens this.

### `src/resources/` — what an agent reads rather than calls

One module per resource, in the shape `src/tools/` has and for the reason [ADR
0002](adr/0002-one-module-per-tool.md) gives: a URI, its description and the
handler that answers it, together. `mod.rs` is the collection — the two lists
a client discovers them through, and the dispatch that reads one.

Each module matches its own URI rather than the collection holding a table, so
there is nothing to keep in step with the templates it advertises. There is
also no order to get wrong: a handle carries no `/`, so
`diffpack://diff/{handle}` and `diffpack://diff/{handle}/file/{path}` are told
apart by their own shapes rather than by which matcher is tried first.

The two lists are two methods because the `2026-07-28` schema gives them two
shapes. A `Resource` carries a `uri` a client can follow as it stands and a
`ResourceTemplate` carries a `uriTemplate` with a field to fill in, so
`diffpack://registries` answers `resources/list` and the two diffs answer
`resources/templates/list`. A template listed as a resource would be a URI a
client followed literally and got `-32602` for.

**Nothing here computes an answer a tool already computes.** The catalogue is
`registry` serialised, the totals are `diff_package_versions`'s walk, the tree
is `get_diff_tree`'s, and one file's patch is `get_file_diff`'s renderer. See
[ADR 0014](adr/0014-a-resource-is-a-projection-of-the-tools.md).

A read has one channel. `ReadResourceResult` carries contents and nothing
else, so there is no `isError` half to put a message in and every failure is a
JSON-RPC error — `Failure::refuse`, beside `respond`, which keeps the message
for the failures that have one rather than answering "no resource at this URI"
to a version that does not exist.

What a tool cannot carry, a resource can: `ttlMs` and `cacheScope` are
`CacheableResult`'s fields, and `CallToolResult` extends plain `Result`. Each
resource states its own freshness — the catalogue changes when a build deploys
and a comparison cannot change at all, because a handle that would be answered
differently is one this build refuses.

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

It fetches nothing. This module says *where* and *what shape*; `archive`,
`catalogue` and `search` do the fetching, and that is what lets every registry
fact be tested with no network at all — the URL npm is asked for, the media
type PyPI's index needs and the order a query ranks its names in are all
asserted without a socket. The `diffpack://registries` resource (#16) is a
projection of this module rather than a hand-written copy of it, and Go
support (#28) is a value added here rather than an edit in five places. See [ADR
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

`versions(registry, package) -> Versions`: every version newest first, and
the one the registry itself points at. The second seam over the network,
beside `archive` and shaped the same way: one interface, two adapters, and a
fixture set keyed by the URL `registry` builds.

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

**The current version is the other answer in the same document**, and it is
read out of it rather than picked from the list: npm's `dist-tags.latest`,
crates.io's `default_version`, the `isDefault` deps.dev puts on a PyPI
version. Which of crates.io's four pointers, and why, is argued in
`src/registry.rs` beside the read. It is not checked against the versions
listed with it — a registry pointing past its own list is answered as the
registry spelled it, because the alternative reports no current release for a
package that has one.

Nothing here is cached. The 256 MB budget is for diff results, and a package's
version list goes stale the moment somebody publishes.

### `src/search/` — which packages a registry has

`hits(registry, query, limit) -> Vec<Hit>`, best match first. The third seam
over the network, shaped like the other two: one interface, two adapters, and
a fixture set under `fixtures/searches/` keyed by the URL `registry` builds —
so an offline test is a test of where each registry is asked as well as of
what comes back, and the set's `null` entry is how the suite reaches the path
a source being down takes.

It is beside `catalogue` rather than inside it because the two answer
different questions and fail in opposite directions: a `404` on a version
document is a package that does not exist, and a `404` on a search source is
the source itself having moved, since nothing in that URL named a package. See
[ADR 0011](adr/0011-what-a-registry-publishes-is-its-own-seam.md).

Where a search is asked, what to ask it for, and how to read the answer are
`registry`'s. What is this module's is the request, the size cap, and the one
policy a search needs that a catalogue does not: PyPI's source is the index of
everything it publishes rather than a reply to a query, so a warm instance
holds the document it already fetched for the ten minutes PyPI's own
`cache-control` gives it — a constant in that module rather than a header read
back off each answer, so a `max-age` PyPI changed is a change made here. That
is a document, not an answer — every
query is matched against it afresh — so no search result is cached anywhere
and nothing here goes near the blob store.

### `src/fetch.rs` — the registries' HTTP client

Every request this server makes to a registry. It was `archive`'s until
`catalogue` needed one too, and two copies would be two places to fix a
timeout, a user agent or a redirect policy — which is the thing ADR 0001
argued against in the first place. What leaves this module is bytes or a
`Failure`, never a status code and never a `reqwest` type, so the seams above
it stay the only things a tool sees.

A registry is the whole of what it fetches from, and that is the line rather
than an accident of what was written first. Its policy is a set of rules about
somebody else's servers — which hosts may be reached, where a redirect may
lead, what a `404` means to the seam that asked — and none of them is a rule
about this project's own blob store, which is why `src/store/` has a client of
its own and this module has no verb but `GET`.

Two caps rather than one, and they are halves of the same guard. The size cap
is the most one body may weigh; `DOWNLOADS_AT_ONCE` is the most bodies this
process may be reading at all, taken as a slot before a request is sent and
held until the body is in memory. What a package name in a tool argument can
cost is the product of the two, so a cap on either alone bounds nothing: four
128 MB bodies are the worst this function holds *in flight*, and without the
second number the worst is however many requests the platform sent this
instance.

In flight is not all it holds. A slot is released once the body is in memory
and before it is extracted, so archives already downloaded and still being
read into `FileMap`s sit beside those four. That is deliberate — extraction
is this process's own work and queueing it behind a download slot would cap
CPU with a network cap — but it means the product above is the bound on
*arriving* bytes rather than on the function's memory, and the two are not
the same number.

The slots are the process's and not a request's, which is the point — a `Ctx`
is built per request, so a cap held there would bound one caller against
itself and leave an instance serving several of them unbounded. Four because
two is the floor: three callers ask for two archives through one `try_join!`
— `diff_package_versions`, `get_file_diff`, and `resources::diff::compare`,
which both diff resources read through — and a cap below two would serialise
the only shape of call this server makes concurrently. The third arrived
after the number was chosen and does not move it: each `bytes()` takes one
slot and gives it back before returning, so no caller holds a slot while
waiting on another, and there is no hold-and-wait among them to deadlock on
however many arrive at once.

A fifth body waits rather than being refused, and the wait has a budget of
its own: `SLOT_WAIT`, which is `UPSTREAM_TIMEOUT` again for a different
reason. That constant bounds what a slot's *holder* can do with one — it
covers the request and the body alike, so no slot is held past it however
badly a registry behaves — and that argument is per slot. It says nothing
about how many turns are queued ahead of a caller: at depth `n` the wait is
`⌈n/4⌉` upstream timeouts, and none of that answers to the 300 seconds
`vercel.json` gives the function. So a caller still queued after one full
turn is behind a queue rather than a turn, and is refused as `Busy` — this
server being full, which is what it is, rather than a `TimedOut` naming a
registry nothing asked. The two budgets added are what bounds the whole of
`bytes()`. The wait counts towards the call's fetch phase, because it is the
same thing to whoever is waiting for the answer.

A caller passes the refusals that differ between seams rather than this module
guessing them, and the one header that differs. A `404` is a missing *version*
to `archive`, a missing *package* to `catalogue` and a broken source to
`search`, and a body over the cap is named after whichever was being read — an
archive has a smaller thing to ask for instead and a version list does not.
The header is `Accept`, which only PyPI's index needs: the same URL serves a
web page unless the request asks for PEP 691's JSON.

### `src/store/` — cached diff results

`DiffStore`: `get(&DiffKey)` and `put(entry)`, a whole entry at a time. The
Vercel Blob client is the implementation behind it and is private to this
module, along with the 256 MB budget and the eviction that keeps it. A
tool asks for a result and gets one or does not; how many HTTP calls that took
is not a tool's business. See [ADR 0003](adr/0003-the-cache-seam-is-a-store.md).

Three things are the store's and not a caller's, and each is a rule about the
cache rather than about the blobs underneath it. **A cache failure is never a
diff failure**: every way this module can fail ends in a miss, so neither
method has a `Result`, and what would have been one is a line in the log
instead. **An entry is the unit**: `meta.json` and `patches.json` go together,
because half of one is not a hit and the only right answer to half is the one
this module already gives. And **a write happens after the answer**, through
the runtime's `waitUntil`, because a caller is waiting on a diff and not on a
cache.

Two caps are the store's too — 256 KiB on one file's patch, 8 MiB on one
entry — and both are fields rather than constants read where they are used, so
a test drives them with a real comparison and a small number. Over the first,
that patch is left out and the rest are kept; over the second, `meta.json` is
written with `patches_omitted` and `patches.json` is not written at all. That
flag is the difference between a comparison whose patches were dropped and one
with nothing to patch, which is otherwise the same absent blob.

A `put` heads before it writes. An entry is derived from its contents, so a
blob already at that pathname holds those bytes already — and rewriting it
would reset the moment it was uploaded, which is the order eviction runs in.

A `put` also asks for room before it writes, and that is the budget: 256 MB
the store may hold, swept down to 240 MB whenever admitting an entry would
take it past the ceiling. The total is read from a listing rather than carried,
because a total carried in one invocation is a number two of them would
disagree about; the entries a sweep takes are the oldest by `uploadedAt`, whole
entries at a time, and an entry larger than the whole budget is refused before
anything is listed rather than emptying the cache to no purpose.

**The head comes before the room is asked for, and the order is load-bearing.**
A write skips a blob that is already there, so room asked for first is room a
write can decline to use — and an entry put a second time would evict other
comparisons to make space it never puts anything in. Sharper than wasteful,
because the entry being written is in the listing the sweep reads and may be
the oldest thing in it: the room it asks for can be freed by deleting the very
blobs the write is about to skip, and a delete that has not propagated yet is
a head that still sees one on its way out. Skip, delete, and the entry is
gone. Bounded rather than silent — `get` reads half an entry as a miss and the
miss rewrites it — but a sweep spent for nothing either way. So an entry
already there is not admitted at all, and what makes a second `put` happen is
a lookup that missed when it should not have: a transient failure, or two
invocations computing one comparison at once.

That is a head per blob on a write that does go ahead, and the write is
backgrounded, so no caller waits on it. The head inside the write is still
there and is not the first one repeated: a sweep happens between the two, and
it is long enough for another invocation to land the blob this one is about to
write.

Two numbers rather than one, and the 16 MB between them is the point. Two
invocations can admit at the same moment, each against a total that did not
include the other's entry, and a delete takes up to a minute to propagate — so
a listing taken after a sweep can still count blobs that are gone. Size
accounting here is a good estimate and never a fact, and a hard-edged check
exceeds the ceiling exactly once, in production, with nothing watching. The
budget is fields rather than constants for the reason the two caps are.

Eviction is **insertion age and not least-recently-used**: an entry written a
month ago goes before one written yesterday, however often it was read. `list`
gives `uploadedAt` for free and an LRU would need a separate index with its own
write path and its own races. What makes that affordable is ADR 0006 — a
handle carries its inputs, so a reading tool whose entry has been swept
recomputes and serves rather than refusing. Eviction costs latency and is
invisible to an agent, which is the footing on which the LRU question is worth
revisiting once there is real traffic to argue from.

Three adapters: the blob store, this process's memory, and no store at all.
The third is not a mode invented for the suite — it is what a deployment
missing its credentials gets, and it is what makes "degraded to uncached" a
thing the suite exercises rather than hopes for. The second is what
`Ctx::fixture` carries, and it is blob-shaped rather than entry-shaped, so a
test of the cache pins where an entry lives and not only that one was kept.

Five operations on the client — write a blob, ask whether one is there, read
one back, list what is under a prefix, delete several at once — and no more,
because a general client for the service is the shape ADR 0003 rejected.
Reading one back is two requests: the API has no operation that answers with a
blob's contents, only one that says where to download them, and both hops are
one operation here because "is it there" and "what is in it" are one
question.

There is no published specification for that API and no usable Rust client for
it, so the wire is taken from what `@vercel/blob` sends, read out of its
source. Three of those facts are not guessable and fail only against the real
store: a write's pathname is a query parameter rather than a path segment, the
store id is provisioned with a `store_` prefix the header does not want, and
`access` has no default. The tests drive the client through a stub HTTP server
on a loopback port, so what they hold is the request this module writes rather
than a shape it was told to produce — and because the client is private, they
live in the module rather than in `tests/`.

A stub cannot settle those three, which is why one test is not a stub. It was
written from the same reading of `@vercel/blob` as the client, so it agrees
with the client whether or not the reading was right; only the store can
disagree. So the four operations also run against it once, `#[ignore]`d the
way everything in `tests/networked.rs` is, and in the module for the same
reason the rest are.

The client here is the crate's second, and separate from `src/fetch.rs`'s on
purpose. That one reaches registries: it GETs, it may follow a redirect only
onto a registry's hosts, and it names its refusals after the seam that asked.
This one writes to a store this project owns, with a credential on every
request and no redirect to follow. One client serving both would be one policy
serving two sets of reasons — but it is one *bar*, so what `fetch` settled for
a registry is settled the same way here: a timeout on every request, and no
retry on an answer that repeating the question cannot change.

### `src/page.rs` — the response ceiling

Vercel's function response limit is 4.5 MB, and every tool that returns a list,
a tree, a file or a patch set can exceed it. One module owns the ceiling, the
cursor format and the "this is a page of N" shape, so that there is one
implementation of staying under it rather than one per tool. See [ADR
0005](adr/0005-one-module-owns-the-response-ceiling.md).

Three interfaces, because an answer comes in three shapes. `paginate` takes a
sequence and returns a `Page`: the items that fit, the next cursor, and the
total. `truncate` takes one blob — a file's content, a file's diff — and
returns an `Excerpt`: as much as fits, a marker saying it was cut, and the
whole thing's real byte count. Truncation lives here rather than in a module
of its own because what the two share is the subtle part and what they differ
in is one field; ADR 0005 records the choice and its cost.

`fits` is the third, and the one with no smaller version of itself. A
sequence too long is paged and a blob too long is cut, because half a file is
still a readable half; a comparison's tree is neither, since the first nine
tenths of one reads exactly like all of it. So what does not fit is
*replaced*, and `diffpack://diff/{handle}` writes the statement that stands in
for it. `PAYLOAD_CEILING` is conservative there rather than exact — that
number is a third of the platform's because a tool's answer crosses the wire
twice and a resource read carries its document once — and the margin is left
where it is deliberately.

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
`get_diff_tree`'s job and not this one's.

That tool is also where a shape that is not a sequence is made into one. A
Page is a slice of a sequence and a cursor names a position in it, so the
tree is walked into a flat list of nodes — each carrying its whole path,
a directory immediately before what is under it — and handed here exactly as
a file listing is. `path`, `depth` and `status` are how a caller asks for
part of it, and they narrow the sequence before this module sees it. See
[ADR 0012](adr/0012-a-tree-is-paged-as-a-flat-sequence.md).

Three things a caller does not do: count bytes, encode a cursor, or decide
what "too big" means. The ceiling is on *serialised* bytes and is a third of
the platform's cap, because `tools::invoke` puts a tool's answer on the wire
twice — as `structuredContent` and as the escaped text block rmcp mirrors it
into. A tool that counted items instead would be correct until someone diffed
a package whose paths are long.

### `src/handle.rs` — the handle a diff is asked for again by

What passes between the tool that computes a diff and the ones that read one
back — `get_diff_tree` and `get_file_diff` today, the diff resources (#16)
beside them later. It carries the `diff_id` — the cache lookup,
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
the length of one request and the seams write into. `Ctx::archive()` and
`Ctx::catalogue()` both hand back their seam with the stopwatch already on
it, so a handler is unchanged and there is no way to wait on a registry
uncounted. One wrapper over both, because the phase answers how long the call
waited rather than which document it waited for — and a tool that only reads
a catalogue reporting no wait at all is the reading an operator would take
for "this one never left the process".

What the call found in the store is recorded the same way and by the same
kind of wrapper: `Ctx::store()` hands back the seam with the lookup already
written down, so `ctx.store().get(..)` is the call it always was and there is
no way left to answer out of the cache without the line saying so. It is not
`Timed`, because a cache read is deliberately outside the fetch phase — that
phase answers how long the call waited on a *registry*, and a lookup counted
towards it would report the call that avoided two downloads as the one that
waited longest. What the line carries instead is `hit` or `miss`, and nothing
at all for a tool that never asked: the two are different populations under
one tool name, and a percentile over both describes neither.

`no_store` is the third value and the same rule one level down. A deployment
with no credentials to reach the blob store with has no store at all — the
server that existed before there was a cache, correct and slower — and every
lookup in one answers nothing. Counted as a miss it reads as a flat hundred
percent miss rate, which is what a working cache that is cold reads as too,
so the one thing an operator most needs to tell apart is the one thing the
field could not say. Left absent instead, it would be indistinguishable from
a tool that never asks. The store settles this when it is built and
`Recorded::get` reads it off `DiffStore::is_available`, which is the only
thing that method is for.

A `Note` is the other thing this module writes, and it is deliberately not a
`Line`. A seam that must not fail a call has nowhere else to put a failure: a
`Failure` would reach the model, and a `Line` is the dispatch's — one per
call, so that counting them counts calls, and already written by the time a
backgrounded cache write has failed. The `DiffStore` is the only seam like
that, by ADR 0003, and it carries a `Sink` of its own for the same reason.

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
request, and the store's write happens after the answer, so a call's line is
written before there is anything to report about it. Each is a decision about
a seam rather than a field to add.

### `src/engine.rs` — the one importer of `diffpack-engine`

Re-exports what the server uses and names the pinned version, which is a field
in the cache key rather than a label. The server computes diffs with the same
code the browser runs; re-implementing any of it would mean two copies of an
output format that has to stay byte-identical.

One function here is written out rather than re-exported, and it is the
exception that the paragraph above is the reason for. `patch` renders one
file's Patch — the four cases a file can be in between two versions, and which
of them is a diff at all. The engine has it, as `build_diff_result`, but
private to its `wasm_bindgen` layer and so not part of the surface a Cargo
dependent links against. Two tools need it and they arrived at different
times — #21 renders every changed file while both archives are extracted, #15
renders one on demand, by which time it costs two downloads — so it goes in
the module that names the engine version it is pinned to, where a drift is one
file to fix. Both call it; the two arrived in parallel each with a copy, and
collapsing them was the first thing the merge of the two was for. See [ADR
0013](adr/0013-the-patch-renderer-lives-in-the-engine-seam.md).

### `src/health.rs` — the `/health` body

The one route that needs no MCP client. It names the build that answered, so a
deploy that silently served the previous binary is visible rather than
inferred.

## The decisions behind this

[`adr/`](adr/) records them, each with the alternative that was rejected. The
rejected alternative is the part that stops a future review re-suggesting it.
