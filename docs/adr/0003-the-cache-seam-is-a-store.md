# 0003. The cache seam is a DiffStore, not a blob client

**Status:** accepted, 2026-09-21. Implemented by #20, #21 and #22.

What the rest of the crate sees of the cache is `DiffStore`: get the Entry for
a `DiffKey`, put an Entry. An Entry is `meta.json` and `patches.json` together
— written together, read together, evicted together. The Vercel Blob client,
the signed URLs, the 256 MB budget and the eviction that keeps it are private
to `src/store/`.

An Entry is the unit because half a cached diff is not a cache hit. A tool that
could read `meta.json` and then fail to read `patches.json` has to decide what
to do about it, and the only correct answer — recompute — is the store's
answer, not a tool's.

## Rejected: exposing the Blob client's own verbs

The alternative is to make #20 a general `put`/`head`/`list`/`delete` client
and let the diff tools call it. It is less code, and it is the shape the Vercel
Blob API documents.

It leaks three size caps into every caller. The Blob API has a limit on a
single upload, this project has a 256 MB budget over the whole store, and
Vercel has a 4.5 MB function response ceiling — and a caller holding
`put`/`head`/`list` has to know all three, because they are what decide whether
a result may be cached whole, in parts, or not at all. Three caps × eight
callers is where "we forgot to check the budget on this path" lives.

It also puts eviction nowhere in particular. Evicting oldest-first (#22) needs
to see every write; a client whose verbs are used directly has writes it never
learns about, so the budget becomes a job someone has to remember to run rather
than a property of writing.

`scripts/check-tool-seams.sh` keeps the line: a module under `src/tools/` that
names the blob store fails the build. The first defence is privacy — the client
is not `pub` — and the check is what notices when someone makes it so.
