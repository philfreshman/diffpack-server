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
