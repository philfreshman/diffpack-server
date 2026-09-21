# diffpack-server

An MCP server that diffs two published versions of a package and lets an agent
read the result. These are the nouns its tools, resources and errors are
written in; where those nouns live in the tree is
[`docs/architecture.md`](docs/architecture.md), and why they are shaped this
way is [`docs/adr/`](docs/adr/).

A term used in a tool name, a parameter, a field or a message means what it
means here. Where two terms are easy to confuse, the definition says what
distinguishes them rather than restating the name.

## Language

### Packages and their contents

**Registry**:
One of the three package hosts this server knows: npm, crates.io, PyPI. The
identifier in a key or a parameter is `npm`, `crates`, `pypi`; the name in a
message to a model is the one the registry uses for itself — `npm`,
`crates.io`, `PyPI`.
_Avoid_: package manager, source, ecosystem, repository

**Package**:
One named thing on a registry, across all of its versions. The name is taken
verbatim — `@types/node` keeps the `@` and the `/`, `Typing.Extensions` keeps
its case.
_Avoid_: library, crate, module, dependency

**Version**:
One release of a package, as the registry spells it. `v4.0.0` and `4.0.0` are
different versions here, because nothing is normalised.
_Avoid_: release, tag, revision

**Archive**:
The single compressed file a registry serves for one version: a `.tgz` from
npm, a `.crate` from crates.io, an sdist from PyPI. It is bytes in flight —
what was downloaded, before anything was read out of it.
_Avoid_: tarball, bundle, download

**FileMap**:
An Archive after extraction: every file path in that version mapped to its
entry, with the archive's top-level directory already stripped. It is what a
diff is computed from, and it is the boundary the rest of the crate sees — a
tool asks for a FileMap, never for an Archive.
_Avoid_: tree, file list, contents, extracted archive

### Diffs

**Diff**:
The comparison of one version of a package against another, in one direction.
A→B is not B→A.

**DiffKey**:
The seven fields that decide whether two Diffs are the same Diff: engine
version, registry, package, from, to, similarity threshold, ignore-whitespace —
plus the schema number. [`docs/cache-key.md`](docs/cache-key.md) is normative
and defines them exactly.
_Avoid_: cache key (the string), diff params

**diff_id**:
`sha256` of a DiffKey's canonical string, as 64 lowercase hex characters. It
names a Diff and cannot be read back: a diff_id on its own does not say which
package it came from, which is why a handle passed between tools carries its
inputs beside it ([ADR 0006](docs/adr/0006-the-handle-carries-its-inputs.md)).
_Avoid_: hash, cache key, id

**Status**:
What happened to one file between the two versions: `added`, `removed`,
`modified`, `renamed`, `unchanged`. The engine's five, deliberately unchanged —
a sixth or a rename of one of these would be a difference between what the web
app shows and what an agent is told.
_Avoid_: change type, state, kind

**Patch**:
One file's rendered unified diff — the text with `@@` hunks in it. A Diff
covers a whole version pair; a Patch covers one file inside it.
_Avoid_: hunk, delta, file diff

**Similarity threshold**:
How alike a removed file and an added file must be before the engine calls the
pair a rename. Part of the DiffKey, because changing it changes the Statuses.
_Avoid_: rename threshold, match score

### The cache

**Entry**:
One cached Diff result: `meta.json` and `patches.json` under one diff_id,
written together and evicted together. Half an Entry is not a cache hit.
_Avoid_: record, object, blob, cached diff

**DiffStore**:
The interface the rest of the crate has to cached results: get an Entry for a
DiffKey, put one. Which store is behind it, how many requests it makes and how
it stays inside the budget are its own business.
_Avoid_: cache, blob client, storage

**Budget**:
The hard 256 MB this project's blob store may hold. Staying inside it is the
DiffStore's job, by evicting the oldest Entries first.
_Avoid_: quota, limit (unqualified — the response ceiling is also a limit)

### The protocol surface

**Tool**:
One MCP tool: a name, a description, an input schema and the handler that runs
it, all in one module under `src/tools/`. "A tool" means all four, not just the
definition a client sees.
_Avoid_: command, endpoint, action, handler (alone)

**Resource**:
Something an agent reads by URI (`diffpack://…`) rather than calls. A Resource
answers "what is there"; a Tool does something.
_Avoid_: document, asset

**Failure**:
Anything that goes wrong, together with which of MCP's two channels it reaches
the client on: a *protocol error* the model never sees, or a *tool error* — a
successful response carrying `isError: true` — which it does see and can act
on.
_Avoid_: error (unqualified), exception, fault

**Page**:
As much of an answer as fits under the 4.5 MB response ceiling, with a cursor
for the rest. Every listing tool answers with a Page, whether or not there is
more.
_Avoid_: chunk, batch, slice

**Excerpt**:
The same ceiling over an answer that is one thing rather than a sequence: a
file's content, a file's Patch. An Excerpt has a marker and a real byte count
where a Page has a cursor and a total — which is the whole of the difference,
and why both live in `src/page.rs` ([ADR
0005](docs/adr/0005-one-module-owns-the-response-ceiling.md)).
_Avoid_: snippet, preview, head

**Cursor**:
Where a walk of a sequence resumes. One format across every paginating tool,
minted by `src/page.rs` and opaque to a client: it is passed back unchanged or
not at all. A cursor a client wrote for itself is refused.
_Avoid_: token, offset, page number

**Response ceiling**:
The 4.5 MB Vercel allows a function's response body. Distinct from Budget,
which is the blob store's 256 MB: this one is per answer and the platform's,
that one is cumulative and ours.
_Avoid_: response limit, size cap
