# 0010. Newest first is a date, not a direction

**Status:** accepted, 2026-09-21. Implemented in #18.

`list_package_versions` promises one order across three registries: newest
first. The order is computed from the publish date each source carries, and
that date is in the answer.

#18 specified something cheaper. Each registry was to carry a direction beside
its URL — npm oldest-first and so reversed, crates.io already newest-first,
deps.dev oldest-first and so reversed — and `src/registry.rs` carried an
`Order` enum saying so. It was wrong twice, and neither was visible without
asking the sources.

**deps.dev does not list PyPI's versions chronologically.** It sorts them
lexically by version string, so `requests` runs `0.10.0 … 2.31.0, 2.34.2,
2.9.0, 2.9.2` and ends at 2.9.2, because `"2.9.2"` sorts after `"2.34.2"`.
Reversing that document announces a release from 2016 as the newest version of
`requests`, and it does so confidently, with no error and nothing in the answer
that looks wrong.

**npm's order does not survive parsing.** Its versions are the keys of a JSON
object, and `serde_json` here has no `preserve_order`, so its map is a
`BTreeMap` and the document's order is gone before this crate sees it. The
direction was not merely wrong for npm; there was nothing to reverse.

All three documents carry a date per version — npm's `time`, crates.io's
`created_at`, deps.dev's `publishedAt` — so that is what the order is built
from. The dates are compared as the strings the source wrote: each source
spells them one way, an answer comes from one source, and no calendar has to
be parsed to put releases in order.

## What newest first then means

Most recently published, not the highest version number. The two differ
constantly: npm's `@types/node` publishes a 22.x patch after a 26.x release
most weeks, `tokio` 1.51.4 landed between 1.52.4 and 1.52.3, and both
registries' own listings show the recent patch on top. An agent asking what
changed in the last two releases means the last two that happened.

It is a third thing again from the registry's own pointer at a current
release — npm's `dist-tags.latest`, crates.io's max stable version, deps.dev's
`isDefault`. For `@types/node` today, `latest` is 26.6.2 and the most recently
published is 24.13.6. This tool answers the question it was asked and puts the
date beside every entry, so an agent can see which question it is looking at.

## Rejected: dropping a version the source gives no date for

deps.dev omits `publishedAt` for some versions: one of `requests`' 161, and
thirty-seven of `numpy`'s 171, including ordinary releases like 1.10.0 and
1.10.3 rather than only `win32-py2.5` oddities.

Dropping them makes the ordering total and the code simpler, and it answers
"what versions does this package have" with a list missing a fifth of them —
silently, since nothing in the answer says a version was left out. A model
told numpy has no 1.10.0 will conclude numpy has no 1.10.0.

So they are listed, with no date, after every version that has one. Last is not
a claim that they are oldest: the promise is newest first, and a release this
server cannot date is one it cannot call the newest. The date being absent in
the answer is what says so.

## Rejected: sorting by parsed version number

A semver parser for npm and crates.io and a PEP 440 parser for PyPI would give
a total order with no dates needed, and would put `@types/node` 26.6.2 above
24.13.6.

It answers a different question from the one asked, it needs two parsers and
two dependencies for a tool that reads one document, and it has no answer at
all for a version that parses under neither — which, given deps.dev serves
`2.23.0-py2.7` for `requests`, is not hypothetical.
