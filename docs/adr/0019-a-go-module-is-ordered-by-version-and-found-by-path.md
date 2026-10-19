# 0019. A Go module is ordered by version and found by its path

**Status:** accepted, 2026-10-19. Implemented in #28.

Go is the fourth registry, and the first whose source does not answer the two
questions the other three answer from one document each. `proxy.golang.org`
lists a module's versions at `@v/list` as bare lines — no date, no order, no
pointer at a current release — and it has no search at all. Two decisions
follow, and both are where Go now differs from npm, crates.io and PyPI. On Go
a Package is a module and its name is the module path, so this record says
module where CONTEXT.md would otherwise say package.

## A Go module's versions are highest version first

[ADR 0010](0010-newest-first-is-a-date.md) made newest first a date, and
rejected sorting by parsed version number. For Go there is no date to sort by,
so every Go version would be "a version the source gives no date for", and
0010's rule for those — listed last, in no promised order — would leave the
whole list unordered.

So a Go module's versions are in diffpack.io's order instead: its comparator,
ported, so that the two show a module the same way. The three numbers decide
first, a release sits above its own previews, and build metadata —
`+incompatible`, mostly — plays no part. Undated versions of the other three
registries keep the order their source gave them; this order only separates
two undated Go versions, which on Go is every pair.

It is diffpack.io's order and not semver precedence, and the two differ where
two previews share their numbers: diffpack.io compares what follows the `-`
as text, so `rc.10` sorts below `rc.9`, where semver would compare the 10 and
the 9 as numbers. This crate compares those bytes where diffpack.io calls
`localeCompare`, and the two agree except on capitals — `v1.0.0-RC1` against
`v1.0.0-rc.9` — which Go allows and almost nobody tags.

0010's two objections to version order carry over only in part:

- **It answers a different question.** On npm a patch to an older line lands
  after a newer major most weeks. On Go a new major version is mostly a
  different module path — `github.com/go-chi/chi/v5` lists only `v5.x.y` — so
  most lines that cross in npm's date order are different packages here. Not
  all: a repository that tagged majors before it had a `go.mod` keeps them on
  one path as `+incompatible`, and `github.com/go-chi/chi` lists
  `v4.1.2+incompatible`, from 2020, above `v1.5.5`, from 2023. That is the
  case `currentVersion` exists for — it is `v1.5.5` there, what `go get`
  installs — and the reason it is beside the list rather than the list's first
  entry, as it is for every other registry.
- **A version that parses under neither parser.** Every version in
  `@v/list` is a semantic version, because Go modules have no other kind, so
  there is one parser and nothing it cannot read.

The tool's description says Go is the exception, and a Go entry from the list
has no `publishedAt`, which is what tells an agent this list is not by date.

### Rejected: one `.info` request per version

`@v/{version}.info` carries a `Time`, so the dates exist, one document per
version. `github.com/aws/aws-sdk-go` has over eighteen hundred versions. A
list call that makes eighteen hundred requests to answer one question is a
list call that never finishes inside a function's lifetime.

### Rejected: deps.dev, as PyPI uses it

deps.dev serves Go modules, with a `publishedAt` per version and an
`isDefault` flag — 0010's order with no extra code. It also lists every
pseudo-version anyone ever fetched through the proxy beside the tags: 109 of
the 138 entries for `github.com/go-chi/chi/v5`. Those could be filtered out,
since a pseudo-version has a recognisable shape, so that is not what decides
it. What decides it is that the result would still be a different list in a
different order from the one diffpack.io shows for the same module, read from
a source that learns of a release after the proxy does.

## The Current version is what `@latest` resolves

The list carries no pointer, so the catalogue asks `@latest` beside it, at the
same time. That is what `go get` installs for a caller who names no version,
which is what the Current version is everywhere else, and it is read from a
document the registry serves rather than worked out from the list.

`@latest` is the request most likely to fail of the two, because the proxy may
have to ask the module's repository to answer it. When the list answered,
that failure costs the pointer and not the list: the versions come back with
no `currentVersion`, as for an npm package with no `latest` tag.

A module that was never tagged lists nothing at all, and the proxy still
resolves a pseudo-version of its default branch at `@latest`. That
pseudo-version is then the module's one version, dated with the `Time` the
proxy gives it — the one Go entry that has a date — and here a failed
`@latest` does fail the call, because there is no list to answer with.

## A Go search is the module path itself

There is nothing to send a Go query to. The proxy has no search endpoint,
`pkg.go.dev` publishes no API, and deps.dev answers exact names only. So a Go
query is taken to be a whole module path and asked of `@latest`: a hit is that
module and the version `go get` installs, and anything the proxy cannot
resolve — a word, a partial path, a path with no module at it — is no hits
rather than a failure. A scheme and trailing slashes are dropped first, since
that is how a path is pasted from a browser, and the hit's name is the path
without them, because it is what every other tool is then asked for.

diffpack.io also asks for a whole module path, for the same reason, but its
hit differs in two ways that are deliberate here. Its version is the top of
its sorted list, which on `github.com/go-chi/chi` is `v4.1.2+incompatible`;
this one is `@latest`'s, because a Hit's version is what the registry would
install. And its description counts the versions, which this one leaves out,
because a Hit's description is the package's own and the proxy has none.

What makes it a miss rather than an outage is that this search source names a
package. [ADR 0011](0011-what-a-registry-publishes-is-its-own-seam.md) reads a
`404` from a search source as the source having moved, because nothing in
npm's or crates.io's search URL names a package. The proxy's `@latest` URL
does, so `SearchSource::names_a_package` says so and a `404` there is that
module not existing.

### Rejected: scraping pkg.go.dev's search page

It is the search Go users actually use, and it is an HTML page whose markup is
no contract. A reader of it breaks when the page is redesigned, and the
symptom is a search that quietly finds nothing.

### Rejected: an index of every module, as PyPI's is held

PyPI's index is held for ten minutes and searched here. Go's equivalent,
`index.golang.org`, is a feed of every module version the proxy has seen since
2019, read a page at a time. Building a searchable index from it is a crawler
and a store, not a registry seam.

### Rejected: checking the path's shape before asking

diffpack.io refuses a query without a dot in its first segment before asking,
to spare a request per keystroke. An agent does not type, and the proxy
already answers a malformed path with the same `404` as a missing module, so a
second rule for what a module path looks like would only be a rule that could
disagree with the proxy's.
