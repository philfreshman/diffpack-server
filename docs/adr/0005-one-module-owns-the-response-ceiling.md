# 0005. One module owns the response ceiling

**Status:** accepted, 2026-09-21. Implemented by #43.

Vercel caps a function's response at 4.5 MB. A file listing for a large
package, a diff tree, and a patch set can each exceed it, and a response that
does is not a truncated answer — it is a platform error with nothing useful in
it for the client.

`src/page.rs` owns the ceiling: the budget itself, the cursor format, and the
shape of "this is a page, here is how to ask for the next". A tool hands it
items and gets back a Page. Where the cut falls is one decision in one place,
and the cursor a client receives from one tool looks like the cursor it
receives from another.

## Rejected: each tool staying under the limit itself

Five or six tools each carrying a constant and a `take(n)` is less indirection
and reads fine at each call site.

It is five implementations of the same subtle thing. The cap is on the
*serialised* response, so a correct implementation measures encoded bytes
rather than counting items — and the tool that counts items instead is correct
until someone diffs a package whose file paths are long, at which point it
fails on a size no test covered. Five copies means five chances to get that
wrong, and the failure lands on the client as a platform error rather than as
anything this repository can explain.

Cursors are the other half. Pagination is only useful if a client can continue;
four tools inventing four cursor encodings is four things for #23 to document
and #24 to check, for no gain.

The cost is that a tool cannot stream a huge answer cheaply — everything goes
through one shaping step. That is acceptable while a function's answer has to
fit in one response anyway.

## The ceiling is a third of the cap

Not a detail: it is the reason this is a module rather than a constant.

`tools::invoke` answers with `CallToolResult::structured`, which sets
`structuredContent` to the value **and** mirrors `value.to_string()` into a
text block, so that a client which cannot read structured output still sees
the answer. The mirror is a JSON string, so every `"` and `\` in it is
escaped — at worst doubling it. A payload of *n* serialised bytes therefore
costs up to *3n* on the wire, and `page::PAYLOAD_CEILING` is
`(4.5 MB - envelope) / 3`.

Worst case rather than typical, deliberately. Real JSON escapes nearer 15%
than 100%, so an ordinary answer leaves most of a megabyte unused. Being wrong
the other way does not produce a slightly large response; it produces a
platform error with nothing in it this repository can explain.

## The arguments an agent sees belong here too

Enforcing the ceiling is half of "no tool names the number". The other half is
the schema, because `limit` and `cursor` are the only part of this module an
agent ever reads, and a tool declaring `limit: Option<u32>` with a sentence
about the default would be naming the number again — in the copy no test
compares against `MAX_LIMIT`, and in the one place #23 says an agent looks.

So `page::Limit` and `page::Cursor` are types this module owns and every
paginating tool declares. The default, the minimum and the maximum are written
into `Limit`'s schema; the "passed back unchanged, never written by hand" rule
is written into `Cursor`'s. A tool inherits both by naming the type, and
changing `MAX_LIMIT` changes every tool's schema in the same commit.

`page::MaxBytes` is the third, and it arrived with #12 under this same
argument rather than as a new decision: `max_bytes` carries the rule that the
ceiling is not the caller's to raise — a larger value is narrowed rather than
refused — and that rule is this module's, so this module writes the sentence
carrying it. A tool spelling out `max_bytes: Option<u32>` would be naming
`PAYLOAD_CEILING` in the copy no test compares against it, once per
blob-shaped tool. Unlike `Limit` it declares no default: omitting it means the
ceiling, because that is what `truncate` falls back to, and a default in the
schema would be a second answer to a question that already has one.

`Cursor` deserialises by decoding, which is the property
[`DiffHandle`](0006-the-handle-carries-its-inputs.md) has for the same reason:
`tools::invoke` reads a handler's `Args` before the handler runs, so a cursor
that is not ours is `-32602` without any handler remembering to check.

### Rejected: two constants and a documented convention

`DEFAULT_LIMIT` and `MAX_LIMIT` are public, so a tool can already write
`#[schemars(range(max = 1000))]` and a description quoting them, and a
convention in `docs/architecture.md` can say that it must.

That is the shape this ADR rejected one level down, arrived at from the schema
instead of from the enforcement. The numbers in a `schemars` attribute are
literals — the attribute cannot take a constant — so the tool's schema and the
module's clamp agree only until one of them moves, and the disagreement is
silent: an agent is told the maximum is 1000, asks for 1000, and gets 200
because `MAX_LIMIT` changed. Five tools is five chances at that, which is the
same argument as the ceiling itself.

The cost is two more types in a module named for neither, and a tool that
wants a narrower default of its own has to clamp it after the fact rather than
declaring it. Nothing in phases 3 or 4 wants one.

## Truncation lives here too, as a second interface

#12 and #15 do not paginate. They return one blob — a file's content, a file's
diff — and cut it. #43 asked for this to be decided rather than defaulted
into, and it is decided: `page::truncate` and `page::Excerpt` sit beside
`page::paginate` and `page::Page` in the same file.

What the two halves share is the part that is subtle — the ceiling, and the
rule that it is measured on *serialised* bytes rather than on length. A file
of quotes costs twice its own length once it is a JSON string, so a cut taken
on raw length measures under the ceiling and produces a response over it,
which is the page-shaped mistake in its blob-shaped form. What differs is one
field: a sequence has a next cursor and a blob does not.

### Rejected: a second module for truncation

`src/excerpt.rs` beside `src/page.rs`, each named for exactly what it does.

It splits the measurement rule from its only other caller, and it gives the
ceiling two consumers — which is the thing this ADR exists to prevent, arrived
at from the inside rather than from a tool. The second module's whole content
would be a cut, a marker and a byte count; one field of difference is not a
module.

The cost is that `page.rs` is named for the sequence half, so a reader looking
for truncation has to be told where it is. `docs/architecture.md` and the
module's own first line both say so, and `Excerpt` is a noun in
[`CONTEXT.md`](../../CONTEXT.md) rather than an unnamed shape inside a tool.
