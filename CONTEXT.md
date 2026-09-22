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

**Listing**:
The metadata document a registry serves when a version's Archive is not at a
path a caller could have constructed — PyPI's, today, and nobody else's. A
Listing is fetched and *chosen from*: it names a version's files, one of
which is the Archive. Which of the two a registry serves is the registry's
fact and not a caller's, so a tool asks where a version's archive is and is
given either.
_Avoid_: metadata, index, manifest, JSON

**Catalogue**:
What a registry says a package's versions are: every published version, with
the date the registry says it was published and whether it is a preview, and
the Current version. It is about a Package where a FileMap is about one
Version of one, it is read rather than extracted, and it is never cached —
registry metadata goes stale when somebody publishes, and the Budget belongs
to Diffs.
_Avoid_: version list, releases, index, metadata

**Current version**:
The one release a registry itself points at: what it installs for somebody who
names no version. Every registry carries one and each spells it its own way —
npm's `dist-tags.latest`, crates.io's `default_version`, the `isDefault` flag
deps.dev puts on a PyPI version. It is a third answer and not either of the
other two: on `@types/node` the Newest first entry is a 24.x patch, 26.6.2 is
the Current version, and neither is a Preview. It is read out of the Catalogue
document and never looked up in the versions beside it, so a registry pointing
at a version this server did not receive is reported as the registry spelled
it rather than as no current version at all. It is the same fact a Hit's
version is, read out of a different document: a Search answer carries it for
npm and crates.io, and PyPI's Index carries no version at all.
_Avoid_: latest as a name for it — the word is ambiguous between this and
Newest first, so it appears here only in quotes, as the question an agent
arrives with; default, stable, `dist-tag`

**Newest first**:
The order a Catalogue is answered in: most recently published first. Not the
highest version number — npm's `@types/node` publishes a 22.x patch after a
26.x release most weeks, and both registries' own listings show the patch on
top. Not a direction to read a source's document in either: deps.dev sorts
PyPI's versions lexically, and npm's own order does not survive parsing. The
date is the only thing that produces it, and a version the source gives no
date for is listed last rather than dropped or guessed at.
_Avoid_: latest, sorted, descending, semver order

**Preview**:
A version that is an alpha, a beta, a release candidate or a development
build, flagged so that an agent asked for "the last two versions" does not
diff against one without knowing. Which spellings count is the registry's:
npm and crates.io are semver, so it is what follows the first `-`; PyPI is PEP
440, where `1.0rc1` is one and there is no separator at all. Build metadata
and a post-release are neither.
_Avoid_: prerelease (as a concept — the field is `prerelease`), unstable,
beta, draft

**FileMap**:
An Archive after extraction: every file path in that version mapped to its
entry, with the archive's top-level directory already stripped. It is what a
diff is computed from, and it is the boundary the rest of the crate sees — a
tool asks for a FileMap, never for an Archive. Every entry's content is text,
because extraction decodes it that way: bytes that are not valid UTF-8 become
replacement characters rather than an error, so a FileMap holds a readable
rendering of a binary file and not the file. Nothing downstream can undo
that, which is why a tool returning content says whether it happened.
_Avoid_: tree, file list, contents, extracted archive

**Index**:
The document a registry publishes naming every package it has — PyPI's,
today, and nobody else's. It is fetched and *searched*: a query is matched
against it here rather than sent, because PyPI has no search endpoint to send
one to. Distinct from a Listing, which is about one version of one package;
from a Catalogue, which is one package's versions and is what "index" is a
banned synonym for there; and from the DiffStore's Entries, which are this
server's own.
_Avoid_: simple index, package list, catalogue

**Hit**:
One package a Search found: the name to pass to any other tool, and beside
it the Version the Registry would install for a caller that named none, and
the package's own description — *where that registry carries them*. npm and
crates.io carry all three; PyPI's Index carries a name and nothing else, so
a PyPI hit has a name and nothing else. An absent version is a registry that
does not say here, never a package that has published nothing.
_Avoid_: result, match, search result, package (unqualified)

**Search**:
Which packages a Registry has that answer to a query, and the interface the
rest of the crate has to that question: a query and a Limit in, Hits out. It
is the Catalogue's sibling and not the Catalogue — that one is asked about a
package a caller can already name, and this is what a caller uses when it
cannot — and neither downloads anything. A tool asks it a question rather
than learning that one registry answers with a ranked reply and another with
its whole Index.
_Avoid_: lookup, find, query (that is the argument), catalogue, index

**Name rule**:
What one registry's spelling of a package name costs a caller, in a sentence
that can be shown to it: npm's scopes, crates.io's `-` against `_`, PyPI's
absent normalisation. A rule is told, not enforced — nothing here refuses a
name for breaking one, because the registry decides what exists. The version
rule is the same kind of sentence, and is one sentence for all three.
_Avoid_: validation, name format, constraint, schema

**Size cap**:
The most one downloaded body may weigh before this server refuses it
unread. It is about what comes *in*: an 80 MB crate is an ordinary thing to
diff and a five-gigabyte one is somebody using a package name to fill this
function's memory. Distinct from the Response ceiling, which bounds one
answer on the way out, and from the Budget, which is cumulative and the blob
store's.
_Avoid_: size limit, max size, quota, ceiling

**Allowed host**:
A host this server may send an outbound request to. The set is derived from
the URLs the registry module builds, so it grows when a Registry is added and
never on its own. Distinct from an allowed *origin*, which points the other
way: a browser this server will answer.
_Avoid_: allowlist (unqualified), whitelist, origin, domain

### Diffs

**Diff**:
The comparison of one version of a package against another, in one direction.
A→B is not B→A.

**Engine**:
The `diffpack-engine` release this build computes with — the same code the
web app runs, pinned. It is a field in the DiffKey rather than a label on the
build: a new Engine means new Diffs, so Entries written by the previous one
are a different key rather than a stale answer.
_Avoid_: core, library, differ, version (unqualified)

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

**Handle**:
What a Diff is asked for again by, and the only way it is: the diff_id
together with the inputs it was minted from, encoded as one opaque string.
`diff_package_versions` mints it and the tools that read a Diff back take it.
It is not a diff_id — a diff_id names a Diff and a Handle is enough to produce
one — and it is minted, never written: a Handle whose halves disagree is
refused ([ADR 0006](docs/adr/0006-the-handle-carries-its-inputs.md)).
_Avoid_: token, reference, diff id (for the handle), session

**Status**:
What happened to one file between the two versions: `added`, `removed`,
`modified`, `renamed`, `unchanged`. The engine's five, deliberately unchanged —
a sixth or a rename of one of these would be a difference between what the web
app shows and what an agent is told.
_Avoid_: change type, state, kind

**Tree**:
A Diff arranged the way the two versions' files are: every directory and
every file in either of them, each with its Status and the lines it gained
and lost. It is one Diff's shape where a FileMap is what one Version ships,
and it is ordered where a FileMap is a map. Two things about it are the
engine's and neither is guessable from an answer, so both are said out loud
wherever one is served: a directory's counts are the sum of its children's,
and a directory a rename left empty is not in the Tree at all. One file or
one directory in it is a *node* — not an Entry, which is the cache's, and not
a FileMap's entry either.
_Avoid_: file tree, hierarchy, listing, entry (for a node)

**Subtree**:
The part of a Tree under one directory: what `get_diff_tree`'s `path` names
and what its `depth` bounds. A directory is not inside its own subtree, so
the one that was asked for is not in what comes back — the rule
`list_package_files`'s `prefix` follows. Distinct from a Page, which is how
much of a subtree one answer carries: a subtree is what was asked for and a
Page is as much of it as fits.
_Avoid_: branch, folder, section, sub-directory

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
written together and evicted together. Half an Entry is not a cache hit, and
a FileMap's entry is a file rather than one of these.
_Avoid_: record, object, blob, cached diff

**Blob**:
One file in the blob store: a pathname, a size in bytes, and the moment it
was uploaded. Two Blobs make an Entry, and nothing outside `src/store/` names
one — a tool asks for a cached result, not for a file under a path. The three
fields are all the cache is built on: the size is what the Budget is counted
in, and the upload moment is the order eviction runs in, so there is no
separate index to keep in step with the store.
_Avoid_: object, file (unqualified — a FileMap's entries are files too), key

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

**Ctx**:
What a Tool's handler is allowed to reach: the seams that carry state a
handler should not build — `archive`, `catalogue`, `search`, the DiffStore —
built once per request and handed to every call. A pure module is not in it
and does not need to be: a handler names `registry`, `page` and `handle`
directly. Anything a handler needs that is neither in Ctx nor a pure module
is a seam it has gone around. It is built whole or not at all: every seam
live, or every seam reading from the fixture sets. There is no half of one,
because the half that was not asked for would have to be live.
_Avoid_: state, globals, services, dependencies

**Hints**:
The three facts a Tool states about itself beside its schema: read-only,
idempotent, open-world. Each is that tool's own answer and none has a
default, because both ways of being wrong are spent on a person: read-only
guessed false asks for a confirmation nobody needed, and guessed true skips
one somebody wanted.
_Avoid_: flags, options, metadata, annotations (the MCP field that carries
them is not the fact)

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
0005](docs/adr/0005-one-module-owns-the-response-ceiling.md)). The byte count
is the text's own, so for a file that did not decode it is the size of the
readable rendering rather than of what the registry served.
_Avoid_: snippet, preview, head

**Summary**:
What `diff_package_versions` answers with: the Totals for a whole Diff, and a
bounded sample of the files that moved most. It is neither a Page nor an
Excerpt — there is no cursor and nothing was cut short, because it was never
the whole tree to begin with. The tree is `get_diff_tree`'s and is paginated;
a Summary is what an agent reads to decide whether to ask for it.
_Avoid_: overview, stats, report, result

**Churn**:
One file's lines added plus its lines removed, and the order a Summary's
sample is in. It is a ranking and not a measurement — two files with the same
churn are separated by path, so that the same Diff always samples the same
files.
_Avoid_: size, delta, weight, score

**Totals**:
How much changed across a whole Diff: one count per Status, and lines added
and removed. The counts are files and never directories — the engine gives a
directory the sum of what is under it, so counting both would report every
change once per directory above it.
_Avoid_: stats, summary (that is the whole answer), counts (unqualified)

**Cursor**:
Where a walk of a sequence resumes. One format across every paginating tool,
minted by `src/page.rs` and opaque to a client: it is passed back unchanged or
not at all. A cursor a client wrote for itself is refused. A tool declares it
as that module's type, so the rule reaches an agent in the schema rather than
in a sentence the tool wrote.
_Avoid_: token, offset, page number

**Cap**:
How many bytes a caller asks one Excerpt for — `max_bytes`, and the Limit's
opposite number on the blob-shaped half. It is the same kind of politeness
and the same kind of not-protection: it can only ask for *less* than the
Response ceiling already allows, because the ceiling is the platform's and
not a caller's to raise. Omitting it means the ceiling alone, which is what
makes a long file come back cut whether or not anyone asked.
_Avoid_: max bytes (as a concept), truncation limit, size (unqualified)

**Limit**:
How many items a caller asks one Page for. Clamped to a documented maximum
and filled in when absent, both by `src/page.rs` — and documented by it too:
a tool declares that module's type, so the default and the range in its schema
are the ones that bind. It is politeness about how much an agent reads at once
rather than protection: the Response ceiling is the protection, and a Page
inside a Limit can still be cut short by it.
_Avoid_: count, size, max results, budget

**Response ceiling**:
The 4.5 MB Vercel allows a function's response body. Distinct from Budget,
which is the blob store's 256 MB: this one is per answer and the platform's,
that one is cumulative and ours.
_Avoid_: response limit, size cap

### Running it

**Line**:
What one tool call leaves behind: a single JSON object naming the tool, a
summary of what it was asked for, where its time went and how it ended. One
per call and written by the dispatch rather than by a tool, so a count of
lines is a count of calls. Distinct from a Failure's message, which is
written for a model and reaches a client — a Line is written for an operator
and goes no further than the platform's logs.
_Avoid_: log, log entry, event, trace, record

**Phase**:
One part of a call that is timed by itself. Two of them today: the whole
call, and the part of it spent waiting for a Registry. Every seam that leaves
this process counts towards the second — an Archive, a Catalogue and a Search
alike — because the question is the wait and not which document was waited
for. A Phase that did not happen is absent from a Line rather than zero,
because zero is a measurement and a percentile taken over one describes
neither population.

Where two fetches overlap — a Diff asks for both versions at once — the
Phase is the window they span and not the sum of their durations. It answers
how much of the call went on waiting, which is the question it is next to the
total to answer; how much Registry work the call caused is a different
question and nothing asks it yet.
_Avoid_: span, step, stage, timing

**Cause**:
Which Failure a call ended in, in one word, as a Line carries it —
`no_such_version`, `rate_limited`, `too_large`. It is the Failure's kind and
not its message: a message is a sentence carrying the package name a caller
sent, so counting by it would give one bucket per call.
_Avoid_: error, reason, status, code
