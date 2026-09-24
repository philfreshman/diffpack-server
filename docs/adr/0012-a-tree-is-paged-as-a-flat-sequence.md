# 0012. A tree is paged as a flat sequence

**Status:** accepted, 2026-09-21. Implemented by #14.

`get_diff_tree` answers with a Page of nodes, each carrying its whole path,
rather than with the nested shape the engine builds. A directory comes
immediately before what is under it, and `path` and `depth` are how a caller
asks for part of the tree instead of all of it.

A Page is a slice of a sequence, and the whole of what makes a cursor possible
is that a sequence has positions to resume at
([ADR 0005](0005-one-module-owns-the-response-ceiling.md)). A tree does not,
until it is walked — and the walk is the thing that has to be the same twice,
because a cursor names a position in it.

So the tool walks the tree once, in the engine's own order, and hands the
result to `page::paginate` exactly as `list_package_files` hands it a list of
files. The nesting is not lost: every node carries its full path, so a caller
that wants the shape back has it, and a caller that wants one level asks for
one level.

## Rejected: returning the nested subtree

The obvious answer for a tool with "tree" in its name: `children` inside
`children`, the way the engine builds it and the way the web app renders it,
with `depth` bounding how deep the nesting goes.

It has no page in it. A package with ten thousand files exceeds the response
ceiling as one object, and there is no cut to take that leaves a valid
answer — half a subtree is a subtree that says its directory holds three
files when it holds nine hundred, which is a wrong answer an agent cannot
detect.
Making it resumable means a cursor that names a path *and* an offset inside
it, and then says what happens when the walk resumes inside a directory whose
parent was already returned. That is a second cursor format, in the crate
whose ADR 0005 exists because one is enough.

The cost is real and it is worth naming: an agent that wants the nested shape
has to rebuild it from paths, and a deeply nested package is more rows than
the same information nested would be. Against that, an agent reading a diff
is reading paths — the next call it makes is `get_file_diff` with one of
them (#15) — and the rows it does not want are the ones `path`, `depth` and
`status` exist to leave out.

## Rejected: files only, with directories left out

Flat and smaller again: list the files and drop the directory nodes, since a
directory is not something an agent can read.

It throws away the two things about this tree that an agent cannot work out
for itself. A directory carries the sum of what is under it, which is how a
caller sees that a change is concentrated in one place without paging through
the files to add them up. And a directory that a rename emptied is *absent*,
which is only visible if directories are listed at all. #14 asks for both to
be surfaced rather than hidden, and a listing with no directories in it hides
both.

It also makes `depth` meaningless: the top level of a package is mostly
directories, so "one level down" over files alone is whatever happens to sit
in the root.

The cost is that a status filter matches directories too, on a status the
engine gives them as a summary of what is under them — so asking for
`modified` returns the directories above a modified file as well as the file.
That is stated in the tool's description and in the `status` argument's own,
and every node carries a `type` for a caller that wants one kind and not the
other.
