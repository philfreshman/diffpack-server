# 0010. What a registry publishes is its own seam

**Status:** accepted, 2026-09-21. Implemented in #19.

Two tools need something from a registry that is not a version's files:
`search_packages` (#19) needs the packages that answer to a query, and
`list_package_versions` (#18) needs a package's releases. Neither downloads an
archive and neither has a version to name, so neither fits through
`archive::fetch(registry, package, version) -> FileMap`.

They get `catalogue::search(registry, query, limit) -> Vec<Hit>`, in a module
beside `archive` and shaped like it: one interface, two adapters — live over
the shared HTTP client, and a fixture adapter reading bodies from
`fixtures/searches/` keyed by URL — and every per-registry fact still in
`registry` rather than in either of them.

The client itself moves down into `src/http.rs`, private to the crate and
named by the two live adapters only. That is the part of ADR 0001 that
generalises: what was being kept out of eight tool modules is equally worth
keeping out of two adapters, because a timeout, a user agent and a redirect
policy fixed in two places are fixed in neither.

`catalogue` joins the allow-list in `scripts/check-tool-seams.sh`. `http` does
not, and must not: a tool that could name it would be a tool that can fetch.

## Rejected: another method on the archive seam

`archive` already has the client, the size cap and the host check, so a
`document(url) -> String` beside `fetch` is four lines and no new module.

It undoes ADR 0001 rather than extending it. The archive seam is deep because
its interface is a `FileMap` — a caller asks for a version's files and cannot
tell how many requests that took. A method that hands back an undifferentiated
body makes the same module an HTTP client with extra steps, and the next
caller that needs "just one small document" has a precedent to point at. The
seam would still be called `archive` while no longer being about archives.

The cost of the alternative is also concrete rather than theoretical: PyPI's
search source is 44 MB fetched once per warm instance, with a freshness window
of its own (`catalogue/live.rs`). That is a policy about *catalogues* — an
archive is immutable and wants no such thing — and it would have landed in a
module whose header says it is about a version's files.

## Rejected: fetching inside `registry`

The per-registry facts a search needs — where each source is, what to ask it
for, how to read what comes back — are already in `src/registry.rs`, so the
request could go there too and the seam would not exist at all.

#42 made that module pure on purpose, and the purity is what makes every
registry fact testable with no network: `tests/registry.rs` asserts the URL
npm is asked for, the media type PyPI's index needs and the order a query
ranks its names in, all without a socket. Putting a request in there would
trade that for one fewer file.
