# 0004. One Registry module

**Status:** accepted, 2026-09-21. Implemented in #42.

What a registry *is* is written once, in `src/registry.rs`: the identifier used
in a parameter and in a cache key (`npm`, `crates`, `pypi`), the name the
registry calls itself in a message a model reads (`npm`, `crates.io`, `PyPI`),
and how to reach it. Every tool that takes a `registry` parameter parses it
through this module, and the `diffpack://registries` resource (#16) is a
projection of it.

Two things follow from the module rather than sitting beside it. The `registry`
enum in a tool's schema is generated from the variant list, and the outbound
host allowlist is the hosts of the URLs this module builds. A list kept
separately from the URLs it is meant to cover is a list that will one day be
missing an entry the code needs — and the symptom is a broken registry, so
someone widens the allowlist to fix it. Deriving it makes that impossible
rather than merely discouraged.

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

## Rejected: a trait with an implementation per registry

The shape this would take in a larger system: a `Registry` trait declaring
`archive`, `versions`, `search`, `hosts`, and three types implementing it,
resolved through a `dyn Registry` or a lookup.

It buys extensibility that nothing here wants. A registry cannot arrive from
outside this crate: it is a variant in a tool's input schema, an identifier in
a cache key and an entry in the outbound allowlist, all of which are compiled
in. Paying for a plugin boundary that no plugin can cross is a cost with no
matching benefit.

What it costs is the reading. The question anyone actually brings to this
module is comparative — *how does PyPI differ from npm here?* — and three arms
of one `match` answer it in a screen, while three types answer it by making
someone open three files and hold them side by side. The `match` also makes the
compiler the checklist: a fourth variant produces one error per fact the new
registry owes, in one file, which is exactly the property #28 needs. A trait
gives the same guarantee only if every method is required, and then the third
implementation is written by copying the second — which is how `crates` and
`crates.io` end up swapped in one of them.

Proven rather than assumed: a throwaway `Go` variant was added while writing
#42. The compiler named eight matches, all in `src/registry.rs` and none
anywhere else; filling them in put `proxy.golang.org` into the derived
allowlist and `go` into the tool schema with no edit outside the file, and the
only tests that failed were the three that pin the list to three registries.
The variant was then removed.

The honest cost is that a registry needing genuinely different *behaviour* —
an authenticated fetch, a paged version source — will strain a `match` where a
trait would absorb it. Three registries and a known fourth are not that, and
this decision is worth revisiting when a fifth is.