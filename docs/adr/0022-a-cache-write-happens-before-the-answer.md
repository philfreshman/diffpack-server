# 0022. A cache write happens before the answer, not after it

**Status:** accepted, 2026-10-19 (#130).

`DiffStore::put` is a write the call awaits. A comparison that missed the
cache is written, with whatever sweep its room needs, before the answer goes
back. The write has a deadline of its own, `store::WRITE_DEADLINE`, 10 s. A
write still unfinished then is dropped and leaves a Note, and the caller is
answered as if the write had happened.

## What it replaces

`docs/architecture.md` said **a write happens after the answer**, through
the runtime's `waitUntil`, because a caller is waiting on a diff and not on a
cache. That was the right trade on paper, and it never wrote anything.

After #135 the store found its credentials and read fine in production: a
lookup was a `miss`, not `no_store`, and an entry written from elsewhere was
served as `cached: true`. But the Blob count stayed at 0, and no Note about a
failed write ever appeared, although every way a write can fail leaves one.
The same build, run as a plain server on a laptop against the same private
store with the same kind of token, wrote the entry, and the repeat was a hit.

`vercel_runtime` 2.4 says why in `awaiter.rs`: `wait_until` is implemented
entirely in-process, the per-request `end` message is sent as soon as the
handler responds, and registered work is drained only at process shutdown.
Nothing tells the platform that work is still running, so the instance gets
no time after the response, and a backgrounded write never reaches either its
end or a failure it could report.

## Why before the answer

- **It is the only order that writes.** Work after the response depends on a
  platform guarantee this runtime does not ask for. Work before it is part of
  the request.
- **The cost is bounded and lands on a miss only.** A hit writes nothing. A
  miss waits for a head per blob, a listing, and two puts. That is a handful
  of requests to an API in the same region, against a diff that has just
  downloaded and extracted two archives. A store that answers slowly costs at
  most `WRITE_DEADLINE` before the write is dropped.
- **A cache failure is still never a diff failure.** A write that fails or is
  dropped is a Note, and the answer is the comparison either way.

## Alternatives rejected

- **Keep `waitUntil` and wait for the runtime to support it.** Nothing
  writes in the meantime. If a later `vercel_runtime` signals pending work to
  the platform, moving the write back is a change to `put` and two tests.
- **Answer first and write in the same request through a streamed
  response.** The answer is one JSON-RPC message. Holding its connection open
  after the body is sent is the same bet on the platform as `waitUntil`, with
  more machinery.
- **No deadline on the write.** The write is inside the call's own deadline
  (ADR 0021), which is about the whole call, not the cache. A store that hung
  would turn into `out_of_time`, a cache failure reaching the caller.

## Consequences

- The longest a call can wait grows by `WRITE_DEADLINE`, to 192.4 s against
  `CALL_DEADLINE`'s 240 s. `tests/errors.rs` adds it to the sum it holds.
- The write is still inside the call's deadline. A comparison that finishes
  within the last ten seconds of it can be answered `out_of_time` while its
  entry is being written. That needs a diff of more than 230 s, which is
  already near the function's limit, and is accepted rather than giving the
  write a deadline that shrinks with the call's.
- A miss's `ms.total` in the log now includes the write.
