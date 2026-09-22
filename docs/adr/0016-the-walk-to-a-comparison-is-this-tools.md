# 0016. The walk from a handle to a comparison is `diff_package_versions`'s

**Status:** accepted, 2026-09-22. Implemented by #83.

Getting from a handle to a compared tree is: look in the store, fetch both
versions concurrently if it is not there, build the tree, render the patches
while both archives are still in hand, write the entry back. It lives in
`src/tools/diff_package_versions.rs`, as `compare`, and all four diff paths go
through it — the tool that mints a handle, `get_diff_tree`, `get_file_diff`,
and the two resources under `diffpack://diff/`.

It was written four times before this, and the copy that consulted the store
was the one that did not need to: `diff_package_versions` is the tool that
computes the comparison in the first place. The other three each paid two
archive downloads, an extraction and a tree build for a tree the store was
already holding, and the comment saying the two downloads do not depend on
each other had travelled with every copy.

It is that module's because that is the tool which *computes* a comparison.
The other three read back what it worked out, which is the relationship
already written into the handle: `diff_package_versions` mints one and the
rest take one. A comparison is that tool's answer before it is anybody's
argument.

## What this is not

**Not a re-opening of [ADR 0014](0014-a-resource-is-a-projection-of-the-tools.md).**
0014 says a module under `src/resources/` computes nothing a tool computes,
and lists the three things each resource calls instead. `resources::diff` was
the exception to its own record: it did not call a tool for the comparison, it
built one — the fetch, the extraction and the tree build were all in that
file. This is the sentence being made true rather than argued with. A reviewer
holding 0014 should read this as its last unfinished clause.

## Rejected: leaving the walk in `src/resources/`

The smallest change, and it was already half-done: `resources::diff::compare`
existed, it was the walk, and three tools could have imported it. `crate::resources`
is on `scripts/check-tool-seams.sh`'s allow-list and `diff_package_versions`
already imports it for the `resource_link` on its answer, so nothing would
have had to be widened.

Against it: three tools calling into a resource for the comparison they
themselves are about is the shape 0014 argues against, pointing the other way.
A resource exists to arrange a tool's answer into a document; a resource that
three tools ask for their answer is a tool with a URI attached. The direction
of the arrow is the whole content of 0014, and this alternative keeps the
duplication out of the tree by reversing it.

It also puts the store in the wrong place. The lookup belongs beside the
`put`, because a miss is what repairs an entry — and the `put` renders patches
out of two archives that are in hand at exactly that moment, which is
`diff_package_versions`'s own work and not a document's.

## Rejected: a module of its own

`src/comparison.rs`, or `src/diff.rs` — the usual answer to two callers, and
0014 rejected it by name for the walks it lists. The reasoning carries: it
moves code with an obvious owner into a module named after neither side, to
avoid `src/tools/` and `src/resources/` naming each other, and they name each
other anyway in the other direction. A comparison's obvious owner is the tool
whose answer it is.

It would also need a decision about the seam script for no gain. A new
top-level module is not on `ALLOWED_MODULES`, so this would have widened the
list a tool may reach — for a function that is a tool's own.

## Rejected: a non-tool module under `src/tools/`

`src/tools/comparison.rs`: the walk near the tools that use it, without
growing the biggest tool module further. It breaks the rule the directory is
held to — every module under `src/tools/` is one MCP tool, definition and
handler together ([ADR 0002](0002-one-module-per-tool.md)) — and the
`check-tool-seams.sh` header says so in as many words. The exemption
`src/tools/mod.rs` has is written as one path precisely so that it is not a
licence the next file under there inherits. One file that is not a tool is how
that rule stops being true.

## What this costs

`diff_package_versions` grows. It was already the largest tool module and it
now holds two public types, a public function and a method that three other
modules call. 0014 counted four exports from this directory with a caller
outside the module that owns them and named the fifth as the direction to
watch; these are the fifth onwards, so the warning is spent here rather than
approached, and the next one has no room left in it.

The difference is which way the work moved. 0014's warning is about a resource
doing its own work through a tool's front door; this is work *leaving* a
resource. The test it suggests still applies, and `compare`'s doc comment
answers it: a resource needs this because the comparison is the tool's answer
and the document is the resource's arrangement of it.

The other cost is `get_file_diff`, which pays for the store and does not yet
collect, and it pays twice over. It renders from both versions' contents and
an entry holds none, so a warm cache saves it a lookup it cannot spend and
neither download. A cold one now costs it a tree build and every changed
file's patch on top of the two downloads it already paid — work this tool did
not do before and does not read, since it fetched two archives and rendered
one file and built no tree at all. That is spent on the three paths that
*can* be served out of an entry: a first call here leaves the comparison
behind instead of forgetting it. What would pay this tool back is the entry's
own patches — rendered on every write since #21 and read by nothing — which
is #84, and this is the reader #84 was waiting for.
