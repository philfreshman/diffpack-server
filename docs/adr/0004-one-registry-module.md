# 0004. One Registry module

**Status:** accepted, 2026-09-21. Implemented by #42.

What a registry *is* is written once, in `src/registry.rs`: the identifier used
in a parameter and in a cache key (`npm`, `crates`, `pypi`), the name the
registry calls itself in a message a model reads (`npm`, `crates.io`, `PyPI`),
and how to reach it. Every tool that takes a `registry` parameter parses it
through this module, and the `diffpack://registries` resource (#16) is a
projection of it.

Go support (#28) is then a value added here and the tools that already exist
gain it, rather than five edits made in five files by someone who has to find
them all first.

## Rejected: a match per tool, and a hand-written catalogue

The path of least resistance: each tool matches on the registry string it was
given, and the `diffpack://registries` resource lists the three registries in
prose.

That is four copies of the same knowledge today and twelve by the end of phase
3, with the catalogue resource as the one that goes stale first — it is the
copy no test exercises, because nothing breaks when a document is wrong. An
agent reading `diffpack://registries` to decide what it may ask for is then
reading the least reliable statement in the system.

The copies also disagree in a way that is hard to see. `crates` is the cache
key's spelling and `crates.io` is what a model should be told; a per-tool match
gets that pairing right until the fourth one, at which point some error
messages say `crates` and a user wonders whether that is a different registry.

The cost is that adding a registry touches a module rather than a line, and
that the identifier and the display name have to stay distinct. Both are
recorded in [`CONTEXT.md`](../../CONTEXT.md) for exactly that reason.
