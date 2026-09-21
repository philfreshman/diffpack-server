# 0001. The archive seam is a FileMap, not an HTTP client

**Status:** accepted, 2026-09-21. Implemented in #10.

Eight tools need the files in a published version of a package. The interface
they get is `archive::fetch(registry, package, version) -> FileMap`: a whole
version's extracted files, or a `Failure`. Resolving the download URL, the HTTP
client, the timeout, the retry, the user agent, gunzip, untar, and stripping
the archive's top-level directory are all on the far side of that signature.

The seam is deep on purpose. What a tool wants is a version's files; how many
round trips that takes, and which of them failed, is a detail it cannot act on
anyway — `error::Failure` already carries the part a model can act on. Two
adapters fit behind it: the live one over `reqwest`, and a fixture adapter
reading tarballs from `fixtures/`, which is what lets the MCP Inspector
conformance suite (#24) run in CI without reaching npm.

## Rejected: a `Fetcher` seam

The obvious alternative is to share an HTTP client — `Fetcher::get(url) ->
Bytes` — and let each tool build its URL and extract its own archive.

It leaks HTTP into eight modules. Each one then has an opinion about where
crates.io puts a `.crate`, which of PyPI's dozen files for a version is the
sdist, what a `429` means, and how long to wait — and those opinions are
written months apart and drift. The first bug in that shape is not a crash but
a divergence: two tools disagreeing about what version `1.0` of a PyPI package
contains, because one picked a wheel and the other an sdist.

It also fixes the failure mode of every tool at "the network broke", which is
the least useful thing to tell a model. `NoSuchVersion` with the versions that
do exist is a next call; a `404` is not.

`scripts/check-tool-seams.sh` enforces this rather than leaving it to review: a
module under `src/tools/` that names an HTTP client fails the build.
