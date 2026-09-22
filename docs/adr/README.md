# Decisions

Why this crate is shaped the way [`../architecture.md`](../architecture.md)
describes. Each record names the alternative that was rejected and why: an ADR
that says only what was chosen is a changelog entry, and the rejected
alternative is the part that stops a future review re-suggesting it.

Numbered in the order they were written down, which is not the order they were
taken — 0007 to 0009 were decided in phases 1 and 2 and recorded here when the
directory was created (#40).

| | Decision | Lands with |
| :--- | :--- | :--- |
| [0001](0001-the-archive-seam-is-a-filemap.md) | The archive seam is a FileMap, not an HTTP client | #10 |
| [0002](0002-one-module-per-tool.md) | One module per tool | done, #41 |
| [0003](0003-the-cache-seam-is-a-store.md) | The cache seam is a DiffStore, not a blob client | #20 #21 #22 |
| [0004](0004-one-registry-module.md) | One Registry module | done, #42 |
| [0005](0005-one-module-owns-the-response-ceiling.md) | One module owns the response ceiling | done, #43 |
| [0006](0006-the-handle-carries-its-inputs.md) | The diff handle carries its inputs | done, #44 |
| [0007](0007-one-importer-of-the-engine.md) | `src/engine.rs` is the only importer of `diffpack-engine` | done, #4 |
| [0008](0008-no-sessions.md) | No sessions, for any client | done, #6 |
| [0009](0009-origin-validation-on-host-validation-off.md) | `Origin` validation on, `Host` validation off | done, #6 |
| [0010](0010-newest-first-is-a-date.md) | Newest first is a date, not a direction | done, #18 |
| [0011](0011-what-a-registry-publishes-is-its-own-seam.md) | What a registry publishes is its own seam | done, #19 |
| [0012](0012-a-tree-is-paged-as-a-flat-sequence.md) | A tree is paged as a flat sequence | done, #14 |
| [0013](0013-the-patch-renderer-lives-in-the-engine-seam.md) | The patch renderer lives in the engine seam | #21 #15 |

A new one goes in at the next number. The bar is the usual three: hard to
reverse, surprising without the context, and the result of a real trade-off. A
decision that fails any of them is a comment in the code, not a file here.
