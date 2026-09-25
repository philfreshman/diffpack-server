# 0018. No rate limit on the public endpoint, for now

**Status:** accepted, 2026-09-25 (#26).

`/mcp` is served without a rate limit on either of its public hostnames,
`mcp.diffpack.io` and `diffpack-server.vercel.app`. Nothing counts how often a
caller comes back. That is a risk taken on purpose rather than a gap nobody
noticed, and this is what was weighed.

#26 asks for Vercel Firewall rules with diff calls limited more tightly than
cached reads, and a comment on it holds three rules, each keyed per IP
address: 20 `diff_package_versions` a minute, 200 an hour, and 600 requests of
any kind a minute. MCP is one `POST` path, so a rule tells a diff from a read
by the `Mcp-Name` header, which rmcp requires on every `tools/call`.

#26 recorded the rate-limit action as Vercel Pro and above, and struck its
first two criteria through as blocked on the plan, this project being on
Hobby. Vercel's published documentation now says otherwise: its
[changelog](https://vercel.com/changelog/rate-limiting-now-available-on-hobby-with-higher-included-usage-on-pro)
and [WAF pricing](https://vercel.com/docs/vercel-firewall/vercel-waf/usage-and-pricing)
give Hobby one rate-limit rule per project, fixed-window and keyed by IP, and
its [custom rules](https://vercel.com/docs/vercel-firewall/vercel-waf/custom-rules)
page puts the `header` condition on every plan. None of it has been confirmed
in this project's dashboard. If it holds, #26's first rule can be created on
Hobby today, and on its own it limits diffs more tightly than reads.

The decision is the owner's nonetheless: no limit, for now. Two things on #26
bear on a rule going in, and neither is settled. The rules comment leaves open
which path the firewall sees, `/mcp` as sent or `/api/mcp` after the rewrite in
`vercel.json`; and #26's second criterion wants #25 to show that ordinary
agent use never trips a limit, and #25 has not run. As this record reads the
trade-off, a rule applied before both are settled risks throttling legitimate
use, and no rule risks what follows. The second is the one accepted.

## What is being accepted

Not a confidentiality risk. Everything this server returns is public package
content, and it is unauthenticated for the same reason `diffpack` is.

What is exposed is what an answer costs. Since `list_package_files` (#11), a
tool call can make this function download and extract a published archive on
demand, and a miss on `diff_package_versions` is two of them and a tree. A
loop — a confused agent as easily as a hostile caller — spends three things:

* **Function time and memory.** On Hobby, usage past the plan's allowance
  pauses the whole team — every project and deployment on it, not only this
  one — until usage resets or Vercel's support lifts it
  ([Hobby](https://vercel.com/docs/plans/hobby), [a blocked deployment](https://vercel.com/kb/guide/why-is-my-account-deployment-blocked)).
  The exposure is availability, not an invoice, and wider than this endpoint.
* **Registry goodwill.** Every download is a request to npm, crates.io or PyPI
  under a `User-Agent` naming this repository (`src/fetch.rs`). A registry that
  throttles or blocks this server does it to every caller at once.
* **The cache.** A loop over distinct comparisons fills the blob store with
  entries nobody reads again and evicts the ones people do.

For scale: the production deployment, `dpl_D7Wj5mj2BiVw1pUgXubPivCnVkYs` at
3b5a9d5 (#107's merge), logged no request in the 48 hours to 2026-09-25
18:03 UTC but one `/health` probe, checked in Vercel's runtime logs that day.

## What bounds it today

None of these is a rate limit — each bounds what one call or one instance can
cost, and none bounds how many calls arrive — but they are why the worst a
single call can do is a known number.

* **The size cap.** `SIZE_LIMIT` in `src/archive/mod.rs`: 128 MB per body,
  refused on `Content-Length` before a byte of it is read and on the running
  total as it arrives (`read_within` in `src/fetch.rs`).
* **The download slots.** `DOWNLOADS_AT_ONCE` in `src/fetch.rs`: four bodies
  in flight, and the process's rather than a request's, so an instance serving
  several callers shares those four. A caller that waits out `SLOT_WAIT` for
  one is refused as `Busy` rather than left queued — the guard #26 asks for.
* **The time limits.** `UPSTREAM_TIMEOUT` in `src/error.rs`, 30 s per request,
  so `fetch::bytes` is bounded at twice that; and `maxDuration: 300` in
  `vercel.json` over the whole invocation.
* **The budget.** `CACHE_MAX_BYTES` in `src/store/mod.rs`: 256 MB over the
  whole blob store, swept oldest first (#22). A loop can churn the cache; it
  cannot grow it.
* **The line.** `src/log.rs` writes one structured line per tool call — tool,
  arguments, result, cache outcome, phase timings — to Vercel's runtime logs.
  A loop shows there as the same tool and arguments over and over, or as a
  miss rate no review makes. It records no caller address, so one source
  dominating would have to be seen in Vercel's request logs or the firewall's
  traffic view rather than here; and a `resources/read` writes no line yet
  (`read_resource` in `src/mcp.rs`), though a read that misses the cache
  works out a whole comparison.

## When to revisit

Any one of these reopens it, and the section below is where the answer starts
rather than a fresh design:

* The line shows a loop — `diff_package_versions` at a rate no person drives,
  or one comparison asked for again and again — or Vercel's request logs show
  a single source dominating.
* Vercel warns that the team is near its usage allowance, or pauses it.
* A registry answers this server with `429` — the `rate_limited` cause on the
  line — or its operator gets in touch through the repository the
  `User-Agent` links to.
* The project moves to a plan with room for more than one rate-limit rule.

## Deferred: the one rule Hobby allows

The first thing to do when this is revisited, whichever trigger reopens it:
confirm in this project's firewall dashboard that Hobby offers a rate-limit
rule with a `header` condition; settle which path the firewall sees, which #26
says only a real blocked request answers; and apply #26's rule 1 — `POST` on
that path with `mcp-name` equal to `diff_package_versions`, 20 a minute per IP
address, deny for 60 s. That rule alone meets #26's first criterion; its
second still wants #25 to show that ordinary agent use never trips it.

Rules 2 and 3 are a starting point to check against the plan's limits, not
rules to apply as written. With rule 1 they are three against Hobby's one, and
rule 2's one-hour window may be longer than any plan below Enterprise allows:
sources citing Vercel put the longest at ten minutes, which is unconfirmed.

## Rejected: upgrading to Vercel Pro now

#26 had it as the only way to meet its first criterion. On Vercel's published
limits Hobby's one rule does that, and what Pro would add is room for more
rules than one, which rules 2 and 3 need. Not now: it is a monthly charge, and
what it buys waits on the same two open questions as the first rule.

Pro would also change the shape of the risk rather than remove it. It bills
on-demand usage up to a spend amount, and then [spend
management](https://vercel.com/docs/spend-management), on by default,
[pauses production](https://vercel.com/changelog/spend-management-now-pauses-production-deployments-by-default):
a capped invoice followed by the same outage.

## Rejected: a rate limiter inside the function

A counter in the process, keyed on the caller's address. It is code this crate
could write today and a test could drive, and it limits nothing that matters.
Instances share no memory ([ADR 0008](0008-no-sessions.md)), so a count in
one instance sees whichever of a caller's requests happened to reach it, and
a loop spread across instances gets an allowance per instance. The download
slots can be per instance because of what they bound: bytes held in one
process are a fact about that process, where a rate is a fact about a caller
across all of them.

A shared counter would work, and needs a store that can count. The one this
project has cannot: the blob store has no atomic increment, and its listing is
an estimate — `CACHE_TARGET_BYTES` leaves room partly because a delete takes up
to a minute to show. Another store is a dependency, a credential, and a round
trip on every call, including the cached reads #26 says must not be limited
into uselessness.

Vercel's own `checkRateLimit` is no shortcut either: it is the JavaScript
`@vercel/firewall` package, and this function is Rust.

## Rejected: a proxy in front, such as Cloudflare

Free at its lowest tier, and short on both halves of the problem.

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
