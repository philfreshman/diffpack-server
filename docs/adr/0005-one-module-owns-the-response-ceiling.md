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
