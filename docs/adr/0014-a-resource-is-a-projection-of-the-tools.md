# 14. A resource is a projection of the tools, not a second answer

Lands with #16.

## The decision

A module under `src/resources/` computes nothing a tool already computes. It
calls the tool's code and arranges the result into a document:

- `diffpack://registries` serialises `src/registry.rs` ([ADR
  0004](0004-one-registry-module.md)).
- `diffpack://diff/{handle}` calls `diff_package_versions::totals` for the
  totals and `get_diff_tree::nodes` for the tree.
- `diffpack://diff/{handle}/file/{path}` calls `get_file_diff::render` for the
  patch and `get_diff_tree::node_at` to find where a renamed file was. Since
  #93 it calls `Comparison::file_patch` instead — see the end.

So four things that were private to a tool module are now public, each with a
doc comment naming its second caller. What a resource owns is the document:
which fields are in it, what the media type says, how long it stays fresh, and
what stands in for a tree that does not fit.

## What was rejected

**A resource with its own walk.** The obvious shape: the resource has the
tree, walking it is fifteen lines, and the tool modules stay closed. It is
also two answers to one question. A reader that got different totals from
`diffpack://diff/{handle}` and from `diff_package_versions` would have nothing
to say which was wrong, and neither would we — the two would have agreed on
the day they were written and drifted at the first change to either. That is
the failure [ADR 0013](0013-the-patch-renderer-lives-in-the-engine-seam.md)
records for the patch renderer, and this issue is where it could have happened
twice more.

**A resource that calls the whole tool.** Stronger still, and wrong for one of
the three: reading a file's diff needs the comparison's tree, to find where a
renamed file was, and `get_file_diff` fetches both archives itself. A resource
that called it would download them a second time — on a package of any size,
the whole cost of the read paid twice. So that tool's work is split at the
line where the archives are in hand, and both callers take the second half.
The other two resources do call what the tools call, because there is nothing
to pay twice.

**A shared module both sides import.** The usual answer to two callers: lift
the walks into `src/diff.rs` and have tools and resources reach it. It moves
code that has exactly one obvious owner — the totals are the summary tool's
answer and the tree is the tree tool's — into a module named after neither, to
avoid `src/tools/` and `src/resources/` naming each other. They name each
other anyway, in the other direction: `diff_package_versions` carries a
`resource_link` and so has to know that resource's URI. The cycle is what it
looks like when a tool's answer points at a resource and a resource is built
out of a tool's walk, and Rust has no objection to it.

## What this costs

The tool modules have a public surface that is not their tool. `totals`,
`nodes`, `node_at` and `render` are each reachable from outside the module
that owns them, which is four more things to keep working than a tool would
otherwise have. That is the price of the guarantee, and it is cheap next to
what it buys: the resource cannot answer differently from the tool, because it
is not answering.

The direction to watch is the next one. A fifth export, or one whose doc
comment cannot name why a resource needs it, is the sign that a resource has
started doing its own work through the tool's front door — and the answer then
is not another `pub`, it is the module that should have owned the thing.

## Since: #93

The file-diff resource was the one that went the way the paragraph above
warns. It took the stored patch, then `get_file_diff::presented`, then both
versions' files, then `get_file_diff::render` — the tool's own steps, in the
tool's order, through three of its exports, with the handle passed back in
beside files that belonged to it. The answer was the module that owns the
thing: `diff_package_versions::Comparison::file_patch` now takes those steps
for both callers, and the resource calls it the way the tool does. `render`
is gone and `presented` has one caller outside its module rather than two;
the things tool modules make public for another module went from eleven to
eight.
