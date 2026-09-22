# 0013. The archive seam bounds bytes in flight, not downloads in flight

**Status:** accepted, 2026-09-22. Implemented in #26.

#13 gave `diff_package_versions` a `try_join!`, so the one tool at the centre
of this server asks for two versions at once. Each archive may weigh up to the
size cap, so one call can have 256 MB arriving, and a warm instance serving
two such calls can have twice that. #26 asks for a cap on concurrent archive
downloads; this records what the cap counts.

`src/archive/in_flight.rs` holds a budget in **bytes**. A fetch reserves room
before its first request and holds it until the bytes have become a `FileMap`;
a fetch that cannot be given room waits for one already in flight to land.
`IN_FLIGHT_LIMIT` is `2 * SIZE_LIMIT` — one whole tool call's worth — and is
written as that multiple rather than as a number of its own.

The budget is the process's, not the request's. `Ctx` is built per request, so
a budget a `Ctx` owned would be one budget per request and no budget at all:
the case worth guarding is two invocations sharing a warm instance, and
neither of them can see the other's.

## Rejected: a count of concurrent downloads

`Semaphore::new(2)` in the same place is smaller, needs no arithmetic, and is
what "a cap on concurrent downloads" literally asks for.

It guards nothing. Two is what one call asks for and nothing in this server
asks for three, so every download there has ever been is admitted — and it
admits them *whatever they weigh*, which is the part that matters, because
what runs this function out of memory is bytes rather than requests. A count
would also have to be chosen again from scratch the day the size cap moves:
nothing in `Semaphore::new(2)` records that the two is a consequence of 128 MB
bodies in a function that can hold about 256 MB of them.

Counting bytes makes the number of concurrent downloads arithmetic rather than
a decision — `IN_FLIGHT_LIMIT / SIZE_LIMIT`, which is two today — and puts the
quantity being protected in the code where the next person to tune it will
read it.

The honest half of this: because a reservation is the size cap rather than the
body's real weight (below), that arithmetic gives exactly two today, so the two
guards behave identically for a single invocation. They differ for a process
serving more than one, and they differ in what happens when either number
moves.

## Rejected: reserving what the body actually weighs

The finer guard reserves a download's `Content-Length` and falls back to the
cap where there is none. Then two 2 MB archives hold 4 MB rather than 256 MB,
and a burst of ordinary downloads never waits at all.

It cannot be reached from here. The declared length arrives with the response
headers, inside `crate::fetch::bytes`, which is one call from the outside:
sending the request, reading the status and reading the body under a limit
happen without returning. Reserving on the declared length means either
splitting that call in two so `archive` can act between the headers and the
body, or passing a reservation callback down through `About` — a change to the
client all three seams share, for one seam's benefit.

And it could not be tested offline. The live adapter is the only path with a
`Content-Length` on it, and `registry::allows` means a request can only be
made to a registry's own hosts — there is no loopback stub server this crate
can point that adapter at. What would be checked is the arithmetic, in a unit
test, while the wiring that carries it went unasserted. A guard whose
interesting half no test can reach is the kind that is quietly wrong later.

So the reservation is the worst case, and the cost is written down rather than
hidden: a small archive holds a large archive's room for as long as it is
arriving. That cost is paid only when three downloads overlap, which needs
more than one invocation in a process.

## Rejected: refusing rather than waiting

The size cap refuses, and refusing is cheaper than queueing: no wait to bound,
no queue to starve.

A body over the cap is refused because it is over the cap whenever it is
asked for — the answer does not change by trying later, and the model has
something to do about it. A download that arrived while another was in flight
is not like that. Nothing about it is the caller's mistake, the same call
would have been served a second earlier, and "this server was busy" is not a
sentence any of `Failure`'s variants says or should learn to say for a
condition that resolves itself. So it waits, and the answer is the same
answer, slightly later.

What that leaves open: the wait itself has no deadline. In practice it is
bounded by the downloads ahead of it, each of which has the 30 s upstream
timeout on it, and by the function's own 300 s ceiling. Giving it a deadline
of its own would mean a new `Failure` a model can read, which is a decision
about what this server says to a client rather than a number to add here.

## What this does not bound

Extraction. A `FileMap` is every file in an archive, decoded as text, so a
128 MB archive becomes a good deal more than 128 MB of `String`s — and this
budget is released only when that map exists, so it bounds the archives being
held *and* overlaps the extraction they turn into, without bounding the maps
themselves. The map is a different quantity with a different remedy (streaming
extraction, or a cap on extracted size) and is not what #26 asked for.
