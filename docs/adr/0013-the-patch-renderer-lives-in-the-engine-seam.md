# 0013. The patch renderer lives in the engine seam

**Status:** accepted, 2026-09-22. Implemented by #21, used by #15.

`engine::patch` renders one file's Patch: the four cases a file can be in
between two versions, and which of them is a diff at all. It is written out in
`src/engine.rs` rather than re-exported, which is the one thing that module
otherwise never does.

The engine has this function. It is `build_diff_result`, and it is private to
the crate's `wasm_bindgen` layer — not part of the native surface a Cargo
dependent links against, so there is nothing to re-export. Only the fourth
case is a diff the engine computes, and `get_diff_content` *is* exported; the
other three are a header and a line prefix over a file one side does not have.

Two callers need it. #21 renders every changed file at the moment both
archives are extracted, because that is when a patch is nearly free; #15
renders one on demand, by which time it is two downloads. Those two renderings
have to agree byte for byte, or the same file reads differently depending on
whether anyone had asked for it before.

They were written in parallel and each arrived with its own transcription,
which is this decision's own rejected alternative happening anyway. The two
were byte for byte identical, so collapsing `get_file_diff`'s onto this one
left every test in `tests/get_file_diff.rs` passing untouched — but nothing
would have failed on the day they stopped being identical, which is the whole
reason for the rule.

So it goes in the module that names the engine version it is pinned to. A
rendering that drifts from the engine's is then one file to fix, and it is the
file a reader already goes to in order to learn what this server takes from
the engine.

## Rejected: a release of the engine that exports it

Making `build_diff_result` public upstream is the version with no copy
anywhere, and it is the right answer if this ever grows.

It is not the right answer now. The engine version is a field in the cache key
([`docs/cache-key.md`](../cache-key.md)), so a release taken to export a
function nothing else needs invalidates every cached entry — and it blocks
this repository on work in another one. The cost of waiting is a page of
straight-line code with a test against a checked-in fixture; the cost of not
waiting is a cold cache and a cross-repository dependency for a line prefix.

## Rejected: rendering it in `diff_package_versions`

The smallest diff at the time, because that is the tool that first needs it.
It makes #15 either import another tool module — which is not what
`src/tools/` is for ([ADR 0002](0002-one-module-per-tool.md)) — or write its
own, which is the drift above with an extra step.

## Rejected: a `src/patch.rs` of its own

A module for one function, whose whole content is "what the engine does". It
would need adding to `check-tool-seams.sh`'s allow-list, to the module map and
to `docs/architecture.md`, and a reader looking for what this server takes
from the engine would have two places to look instead of one.

## Since: #95

`patch` is no longer the one function `src/engine.rs` writes out rather than
re-exports. `build_diff_tree` is written out there too, for a smaller reason
than this record's: the engine's function takes the map a FileMap now keeps
private (ADR 0001), so the version here takes two FileMaps and hands the
engine the map inside each. Nothing of the engine's is re-implemented for it.
The tree is still the engine's own, and only the crossing is written here.
