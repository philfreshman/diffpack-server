# 0006. The diff handle carries its inputs

**Status:** accepted, 2026-09-21. Implemented by #44.

`diff_package_versions` (#13) computes a diff; `get_diff_tree` (#14),
`get_file_diff` (#15) and the diff resources (#16) read one back. What passes
between them is a **handle**: the `diff_id`, and beside it the inputs it was
minted from — registry, package, from, to, similarity threshold,
ignore-whitespace.

The inputs are there because `diff_id` is `sha256` of the canonical DiffKey and
a hash does not invert. A tool holding only a `diff_id` can look in the cache
and, if the Entry is not there, can do nothing else at all. With the inputs it
recomputes and answers.

The handle stays verifiable: the `diff_id` is recomputable from the inputs
beside it, so a handle whose two halves disagree is rejected rather than
trusted.

## Rejected: a bare diff_id

The smaller, tidier handle — a single opaque string, the way an object id
usually is.

It makes eviction a correctness problem rather than a performance one. #22
evicts oldest-first to stay inside the 256 MB budget, which is a routine,
expected event; with a bare handle, every eviction silently disables three
tools for whichever diff it evicted. An agent that computed a diff an hour ago
and asks for one file of it gets "no such diff" — for a diff that did exist,
from a server that could recompute it in seconds, with nothing in the message
suggesting the fix is to run the whole diff again.

That also inverts what the cache is. With a bare handle the cache is not an
optimisation for the reading tools: it is their only source of truth, and a
cache the correctness of three tools depends on is not a cache. With the inputs
in hand, a miss costs time and nothing else.

The cost is a larger handle and one more thing for #23 to describe to an agent.
Worth it: the alternative's cost is an error a user cannot act on.
