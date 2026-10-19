# 0020. The compute deadline lives on the call, not in the engine

**Status:** accepted, 2026-10-19 (#26).

Every tool call and every resource read has a deadline, `CALL_DEADLINE`,
240 s from when `tools::run` starts it. When it passes, the caller is answered
`Failure::OutOfTime` on the try-again channel (`-32003`), and the line's
cause is `out_of_time`. The steps that work rather than wait run on tokio's
blocking pool so that the deadline can pass while they run: extraction,
inside `Archive::fetch`, and the tree build with its patches and the one-file
patch, through `Call::compute`. Both go through `compute::off_the_runtime`.

## Why there had to be one

#26 asks for "a check that nothing takes the function to its duration limit
rather than answering". Every wait already had one: `UPSTREAM_TIMEOUT` per
request, and `SLOT_WAIT` for a download slot. The work after the waits had
none below `maxDuration: 300` in `vercel.json`. The engine caps decompression
at 1032 times the compressed size (`MAX_DEFLATE_RATIO` in `diffpack-engine`),
which against a 128 MB body is no limit at all. A comparison of two large
archives was a request Vercel killed, not one this server answered.

## Why 240 seconds

The longest a call can wait on registries is a PyPI listing and then its
archive. Each is a slot and a request, so 4 × `UPSTREAM_TIMEOUT` = 120 s, and
the two versions are fetched at once. 240 s outlasts that, so a slow registry
is still answered as `timed_out` or `busy`, which names it, rather than as
`out_of_time`. 240 s also leaves 60 s of the function's 300 s to answer in.
`tests/errors.rs` holds both against `vercel.json`, so moving either number
fails a test rather than a request.

## What it does not do

**The work does not stop when the deadline passes.** A task on the blocking
pool cannot be cancelled from outside. It runs to the end, holding its thread,
its memory and its share of the CPU, and then its result is dropped. Nothing
is cached from it either: the store write comes after the work, in the future
that was abandoned. So the guarantee is that the caller is answered, not that
the work stops. On a one-vCPU instance, the next call shares the CPU with
whatever outlived its deadline. The pool's own thread limit (tokio's default,
512) bounds how many such leftovers can run, but it is no memory bound: two
archives' worth of files each.

**A tool can forget.** `Archive::fetch` puts extraction on the pool itself,
so no tool can skip that. The tree build and a patch go through
`Call::compute` only because the code that calls them asks it to. A tool that
calls the engine inline escapes the deadline and nothing fails to compile. The
argument for the line ("a tool that forgot would not fail to compile") applies
here too, and is not met. It is accepted because the engine has three callers
of that kind, all in `diff_package_versions`. Making the engine's functions
reachable only through `Call` would turn a pure module into a seam (ADR 0007),
for a guarantee review already gives.

## Rejected: an unpacked-size cap in the engine

The coordinator's alternative, and the first one the issue named. It needs a
`diffpack-engine` release, a pin bump in both consumers, and a move of
`engine::VERSION`, which changes every `diff_id` and starts the cache cold.
It also bounds the wrong thing. Size is not time: a modest archive of many
small files, or one where rename detection compares many candidates, is slow
at a size no cap would refuse. A size cap may still be worth having as a
*memory* bound, and that is a separate decision.

## Rejected: a deadline without the blocking pool

A timer fires only when the future it races yields, and extraction and the
tree build are long synchronous stretches that never do. On an instance with
one vCPU, tokio's runtime has one worker thread, and the work would hold it, so
the timer could not run at all. `vercel.json` does not choose the instance
size, so this crate cannot count on having more than one.

## Rejected: spawn the whole call and time out the task

`tokio::spawn` the dispatch and race its `JoinHandle` against the deadline.
On one worker thread this has the same problem, because the spawned task holds
that thread. It also needs the call's borrowed `Ctx` and arguments to be
`'static`.

## Rejected: a timeout per step

One limit for extraction and another for the tree build. What has to fit
inside 300 s is the sum of the steps and the waits, and a timeout per step
says nothing about the sum. A single deadline on the call is one number,
checked against `vercel.json` in one test.

## Rejected: answering `Busy` or `TimedOut`

`Busy` says the server was fetching all it would at once and the call never
got a download slot. `TimedOut` names a registry that did not answer. Neither
is what happened, and each sends a model somewhere that will not help.
`OutOfTime`'s sentence says to try once more, because a slow registry or a
busy instance passes. It also says that a second time out means what was
asked for is too large to work out here.

## Rejected: stopping the work cooperatively

Have the engine check a flag between files and give up when the deadline
passes. That would make the work stop as well as the wait. It is a change
to the engine's interface, which needs an engine release, and it reaches into
the engine's loops from a request (ADR 0007). Worth revisiting if leftover
work ever shows up as an instance's memory, rather than as a slow answer.
