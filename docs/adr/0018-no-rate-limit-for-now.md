# 0018. No rate limit on the public endpoint, for now

**Status:** accepted, 2026-09-25 (#26).

`/mcp` is served without a rate limit on either of its public hostnames,
`mcp.diffpack.io` and `diffpack-server.vercel.app`. Nothing counts how often a
caller comes back. That is a risk taken on purpose rather than a gap nobody
noticed, and this is what was weighed.

#26 asks for Vercel Firewall rules with diff calls limited more tightly than
cached reads, and a comment on it holds three rules and their numbers: 20
`diff_package_versions` a minute and 200 an hour per address, and 600 requests
of any kind a minute. None of them can be created. The rate-limit action is
Vercel Pro and above, and this project is on Hobby. The tighter-for-diffs half
narrows it further: MCP is one `POST` path, so the only thing a rule can tell
a diff from a read by is the `Mcp-Name` header, and of the realistic options
only Vercel Pro matches a request header. #26's first two criteria are struck
through as blocked on the plan, not on code, and they stay that way.

## What is being accepted

Not a confidentiality risk. Everything this server returns is public package
content, and it is unauthenticated for the same reason `diffpack` is.

What is exposed is what an answer costs. Since `list_package_files` (#11), a
tool call can make this function download and extract a published archive on
demand, and a miss on `diff_package_versions` is two of them and a tree. A
loop — a confused agent as easily as a hostile caller — spends three things:

* **Function time and memory.** On Hobby, usage past the plan's allowance
  pauses the project rather than billing it, so the exposure is availability,
  not an invoice: a loop can take the endpoint away from everyone else.
* **Registry goodwill.** Every download is a request to npm, crates.io or PyPI
  under a `User-Agent` naming this repository (`src/fetch.rs`). A registry that
  throttles or blocks this server does it to every caller at once.
* **The cache.** A loop over distinct comparisons fills the blob store with
  entries nobody reads again and evicts the ones people do.

## What bounds it today

None of these is a rate limit — each bounds what one call or one instance can
cost, and none bounds how many calls arrive — but they are why the worst a
single call can do is a known number.

* **The size cap.** `SIZE_LIMIT` in `src/archive/mod.rs`: 128 MB per body,
  refused on `Content-Length` before a byte of it is read and on the running
  total as it arrives (`read_within` in `src/fetch.rs`).
* **The download slots.** `DOWNLOADS_AT_ONCE` in `src/fetch.rs`: four bodies
  in flight, and the process's rather than a request's, so an instance serving
  several callers holds four in all. A caller that waits out `SLOT_WAIT` for
  one is refused as `Busy` rather than queued — the guard #26 asks for.
* **The time limits.** `UPSTREAM_TIMEOUT` in `src/error.rs`, 30 s per request,
  so `fetch::bytes` is bounded at twice that; and `maxDuration: 300` in
  `vercel.json` over the whole invocation.
* **The budget.** `CACHE_MAX_BYTES` in `src/store/mod.rs`: 256 MB over the
  whole blob store, swept oldest first (#22). A loop can churn the cache; it
  cannot grow it.
* **The line.** `src/log.rs` writes one structured line per tool call — tool,
  arguments, result, cache outcome, phase timings — to Vercel's runtime logs.
  A loop shows there as the same tool and arguments over and over, or as a
  miss rate no review makes. It names no caller, so it shows a pattern rather
  than an address; and a `resources/read` writes no line yet
  (`read_resource` in `src/mcp.rs`), though a read that misses the cache
  works out a whole comparison.

## When to revisit

Any one of these reopens it, and #26's rules are where the answer starts
rather than a fresh design:

* The project moves to Vercel Pro. The three rules then apply as written, once
  the two things the comment leaves open are settled: which path the firewall
  matches, `/mcp` or the rewritten `/api/mcp`, and that #25's ordinary agent use
  never trips them.
* The log shows a loop or a single source dominating: `diff_package_versions`
  at a rate no person drives, or one comparison asked for again and again.
* Vercel warns that the project is near its usage allowance, or pauses it.
* A registry answers this server with `429` — the `rate_limited` cause on the
  line — or its operator writes to the address in the `User-Agent`.

## Rejected: upgrading to Vercel Pro now

The only option that meets #26's criterion as worded, and not rejected for
good: it is the first trigger above. Not now, because it is a monthly charge
against traffic that barely exists yet. The tools that download went to
production the day before this was written (#107), and nothing in the log
looks like abuse.

The numbers would be tuned against nothing, too. #26's second criterion is that
ordinary agent use is never throttled, and that cannot be checked until #25
runs. And Pro changes the shape of the risk as well as its price: past its
included usage it bills rather than pauses, so the upgrade that buys a limit
also turns the outage above into an invoice unless spend management is set to
put the pause back.

## Rejected: a rate limiter inside the function

A counter in the process, keyed on the caller's address. It is code this crate
could write today and a test could drive, and it limits nothing that matters.
Invocations share no memory ([ADR 0008](0008-no-sessions.md)), so a count in
one process sees whichever of a caller's requests happened to reach that
instance, and a loop spread across instances gets an allowance per instance.
The download slots can be process-wide because of what they bound: bytes held
in one process are a fact about that process, where a rate is a fact about a
caller across all of them.

A shared counter would work, and needs a store that can count. The one this
project has cannot: the blob store has no atomic increment, and its listing is
an estimate — `CACHE_TARGET_BYTES` leaves room partly because a delete takes up
to a minute to show. Another store is a dependency, a credential, and a round
trip on every call, including the cached reads #26 says must not be limited
into uselessness.
Vercel's own `checkRateLimit` is no way round the plan: it consults a
rate-limit rule made in the firewall, which Hobby cannot create, and it is a
JavaScript package where this function is Rust.

## Rejected: a proxy in front, such as Cloudflare

Cheaper than Pro, and short on both halves of the problem.

The tighter-for-diffs rule needs a header match, and Cloudflare offers request
headers to rate limiting rules only at Enterprise. Its free tier is one
path-only rule on a ten-second window: one blunt limit on one `POST` path,
serving a diff loop and the burst of file reads an ordinary review makes, which
pull opposite ways. It would also mean moving `diffpack.io` off the Vercel
nameservers it is on today.

And it covers one front door of two. A proxy sits in front of
`mcp.diffpack.io`; `diffpack-server.vercel.app` is the production alias, public
for the same reason the custom domain is, and reaches the function without
passing through it. A limit that anyone can step around by using the other
hostname limits only the callers who were not trying.

## Rejected: requiring a key

A key per caller would give a limit something better to count by than an
address. It would also end a posture this server has on purpose — public
content, unauthenticated, like `diffpack` — and a key anyone can have for the
asking is one a loop can have another of.
