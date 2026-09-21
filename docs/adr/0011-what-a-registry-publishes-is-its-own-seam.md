# 0011. What a registry publishes is its own seam

**Status:** accepted, 2026-09-21. Implemented in #19.

Two tools need something from a registry that is not a version's files:
`list_package_versions` (#18) needs a package's releases, and `search_packages`
(#19) needs the packages that answer to a query. Neither downloads an archive
and neither has a version to name, so neither fits through
`archive::fetch(registry, package, version) -> FileMap`.

Each gets a seam of its own, beside `archive` and shaped like it: one
interface, two adapters — a live one over `src/fetch.rs` and a fixture one
reading bodies from a directory keyed by URL — and every per-registry fact
still in `registry` rather than in any of them. #18 landed
`catalogue::versions(registry, package) -> Vec<Version>`. #19 lands
`search::hits(registry, query, limit) -> Vec<Hit>`.

Both join the allow-list in `scripts/check-tool-seams.sh`. `fetch` does not,
and must not: a tool that could name it would be a tool that can fetch.

## Rejected: one seam for both questions

#19 was written against a base where #18 had not landed, and proposed a single
`catalogue` covering both. The two shipped separately instead, and the reasons
hold now that both exist rather than being an accident of what merged first.

They answer different questions about different things. A catalogue is asked
about a package the caller can already name; a search is what a caller reaches
for when it cannot. Their refusals invert: a `404` on a version document is a
package that does not exist, and a `404` on a search source is the source
itself having moved, because nothing in that URL named a package. Their costs
differ by two orders of magnitude — one document per package against, for
PyPI, the index of every project there is — and that difference buys the one
policy `search` has that `catalogue` must not: a warm instance holds the index
it already fetched for the ten minutes PyPI's own `cache-control` gives it.
Ten minutes is a constant in `src/search/` rather than a header read back off
each answer — nothing here parses one — so a `max-age` PyPI changed is a
change made here and not one that arrives on its own. In one module that
policy would sit under a header that says it is about one package's releases,
and the size cap would be one number doing two jobs.

What they do share is the fixture convention and the client, and both are
already shared without being one module.

## Rejected: another method on the archive seam

`archive` already has the size cap and the host check, so a
`document(url) -> String` beside `fetch` is four lines and no new module.

It undoes ADR 0001 rather than extending it. The archive seam is deep because
its interface is a `FileMap` — a caller asks for a version's files and cannot
tell how many requests that took. A method that hands back an undifferentiated
body makes the same module an HTTP client with extra steps, and the next
caller that needs "just one small document" has a precedent to point at. The
seam would still be called `archive` while no longer being about archives.

## Rejected: fetching inside `registry`

The per-registry facts a search needs — where each source is, what to ask it
for, how to read what comes back — are already in `src/registry.rs`, so the
request could go there too and the seam would not exist at all.

#42 made that module pure on purpose, and the purity is what makes every
registry fact testable with no network: `tests/registry.rs` asserts the URL
npm is asked for, the media type PyPI's index needs and the order a query
ranks its names in, all without a socket. Putting a request in there would
trade that for one fewer file.
