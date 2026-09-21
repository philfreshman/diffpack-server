# 0007. `src/engine.rs` is the only importer of `diffpack-engine`

**Status:** accepted, 2026-09-13 (#4). Enforced by
`scripts/check-engine-seam.sh` since then; recorded here in 2026-09-21.

The server depends on `diffpack-engine` as a native Cargo dependency pinned to
a git tag, and names it in exactly one module. Everything the crate uses is
re-exported from `src/engine.rs`, and `src/engine.rs::VERSION` is the pinned
release — a field in the cache key rather than a label, since rename detection
and line counts are engine behaviour.

One seam means one file to change when the engine moves, and one file to read
to know what we depend on. It also makes "what did the engine's API just break"
a question with a bounded answer.

## Rejected: importing it where it is needed

The normal thing to do with a dependency, and the reason this is worth
recording: a reviewer who has not seen this document will read the re-export
list and think it pointless indirection.

The failure it prevents is silent. A second importer costs nothing on the day
it is written — it compiles, the tests pass — and it turns the next engine bump
into an archaeology exercise across the crate, at a moment when something is
already broken. Prevented by a check rather than by a convention, because a
convention holds until someone is in a hurry, and this one has to hold for two
more phases of work.

The version constant is the second reason. It is in the cache key, so the
engine version and the value the cache is keyed on must be the same fact;
`tests/engine.rs` fails when that constant and the tag in `Cargo.toml`
disagree. That test has one place to look because there is one place to name.

## Also rejected: re-implementing the diff

Considered and dismissed early. The output format has to stay byte-identical
with what the browser renders, and two implementations of it with nothing
keeping them honest is a divergence nobody sees until a user compares the web
app with an agent's answer.
