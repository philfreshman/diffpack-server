# 0015. One implementation beneath three registry seams

**Status:** accepted, 2026-09-22. Implemented in #81.

`archive`, `catalogue` and `search` each ask a registry for something, and
before any of them does the thing that makes it different, all three do the
same four: build a URL, check it against the hosts `registry` names, go to a
live registry or to a fixture set, and weigh what came back against the size
cap. Only the last step is genuinely per-seam — one extracts an archive, one
reads a version list, one reads hits.

Written three times, those four steps had begun to disagree about things
nobody had decided. Three readers of one fixture index format, in 73, 57 and
67 lines. `Failure::Internal { doing }` spelled four times in one function in
two of them and once through a closure in the third. Three `weight >
self.limit` comparisons, one of which could never fire. Three comments saying,
in three wordings, that this was the same code as the other two.

`src/document/` is those four steps once. Each seam holds one and hands it an
`About` — the registry, what the request accepts, whether it may be
compressed, whether a body is worth holding between invocations, and the three
refusals: a URL off the allowlist, a registry with no such thing, a body over
the cap. That is `fetch::About`'s arrangement one level up, and for
`fetch::About`'s reason: a `404` means a missing version to one seam and a
missing package to another, and one refusal for all three is a message a model
cannot act on.

What the module does **not** take is a way to read the body. That stays with
the seam, and it is why there are still three of them.

A fourth registry — Go, in #28 — now costs a parse rather than a copy.

## This does not reopen ADR 0011

[0011](0011-what-a-registry-publishes-is-its-own-seam.md) settled that what a
registry publishes is its own seam, and rejected **one seam for both
questions**: a single `catalogue` that both `list_package_versions` and
`search_packages` would ask through. That rejection stands, and this record
does not weaken it.

The two are different arrangements. 0011 is about the *interface* a caller
names: a catalogue is asked about a package the caller can already name and a
search is what a caller reaches for when it cannot, their `404`s mean opposite
things, and their costs differ by two orders of magnitude. All of that is
still true and all three interfaces survive here unchanged — `Archive::fetch`,
`Catalogue::versions` and `Search::hits` take what they took and return what
they returned, and every suite driving them passed untouched.

This is about the *implementation* underneath, which no caller can name.
`src/document/` is deliberately absent from `scripts/check-tool-seams.sh`'s
list for the reason `src/fetch.rs` is: a tool that could name either would be
a tool that can fetch. 0011's own closing sentence is the one this builds on —
*what they do share is the fixture convention and the client, and both are
already shared without being one module*. This is the third and fourth things
they share, shared the same way.

The distinction is worth writing down because the next review will find three
modules that look alike and reach for 0011 to explain why they were not
merged. 0011 explains why the *interfaces* were not merged. It never argued
for three copies of a host check.

## Rejected: one seam for all three questions

The arrangement 0011 refused, extended to cover `archive` as well: one
`registry_client` a tool asks for an archive, a version list or a search, with
the answer's shape switched on inside it.

It is a worse idea now than when 0011 refused it, because there is a third
seam to fold in and it is the one ADR 0001 is about. The archive seam *is* a
`FileMap` — a caller asks for a version's files and cannot tell how many
requests that took — and a module that also answers "which packages are there"
is a module whose interface is a body again. Every argument 0011 makes about
the two would survive: a `404` would still mean three different things, the
size cap would be one number doing three jobs, and the ten-minute hold on
PyPI's index would sit under a header claiming to be about a package's files.

What changes is who pays. Under 0011's arrangement the cost of *not* merging
was three copies of four steps, and that cost is what this record removes. So
the case for merging the interfaces is weaker than it has ever been: the
duplication that was the only argument for it is gone.

## Rejected: a trait with three implementations

`Source` is an enum with a `Live` and a `Fixture` variant in `src/document/`,
as it was in each of the three seams before.

The same reasoning as [ADR 0004](0004-one-registry-module.md): neither adapter
can arrive from outside this crate, so the extensibility a trait buys has no
buyer, and two arms of one `match` are read in a screen where two types are
read in two files. Nothing about there being one of them now rather than three
changes that.

## Rejected: leaving the cap where it was

The cap could have stayed in each seam, weighing what `document` handed back.
Four lines each, and the refusal would be built beside the constructor that
names it.

It is the shape that produced the finding. On the live path `fetch` has
already refused the body twice — on a declared length before a byte is read,
and on a running total as the chunks arrive — so a third comparison after
buffering restates a decision already made, in the one place a reader looks
for the rule. Deleting it outright was not the fix either: nothing streams a
fixture body or one a warm instance is already holding, so that comparison was
also the only thing keeping the cap a rule about what this server will read
rather than about where bytes came from. It is in `document` once, on the
paths that have not already had one, and the seams still name the number and
the refusal.

## What this costs

A seam no longer reads end to end in one file. Someone asking *what happens
when npm answers a version request with a 404* reads `src/catalogue/mod.rs`
for the refusal, `src/document/` for the dispatch and `src/fetch.rs` for the
status mapping — three files where it used to be two. That is the price of
not having the host check written three times, and it is paid by a reader
rather than by a caller.

The mitigation is that `About` is the whole of what a seam says. Every
question `document` cannot answer for a caller is a field on it, so what is
per-seam about a fetch is readable in one struct literal in the seam's own
file, and what is shared is not in that file at all.
