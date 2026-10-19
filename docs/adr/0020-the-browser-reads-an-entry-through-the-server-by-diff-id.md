# 0020. The browser reads an Entry through this server, by diff_id

**Status:** accepted, 2026-10-19. Implemented by #27.

`diffpack`, the web app, reuses the Diffs this server has already computed. It
computes the `diff_id` itself from [`../cache-key.md`](../cache-key.md) and
asks `GET /diffs/v1/{diff_id}` on this server, which looks the Entry up in the
DiffStore and answers with its key, tree and patches, or a 404 miss.

The Blob store is **private** (`vercel blob get-store`, 2026-10-19, recorded on
#27). Access is fixed when a store is created, so the browser cannot be pointed
at the blobs, and something holding the store's credential has to read them.
That is this server, and the route is read-only: it can answer with an Entry
that exists and nothing else.

## Rejected: handing the browser a signed download URL

The store's own read gives back a `downloadUrl` with a token in it, and passing
that on would leave this function out of the second hop. But the token is a
credential for the store, however short-lived, and it would be in a browser's
network log, its history and any extension that reads either. `src/error.rs`
already redacts these URLs from every message for that reason. The route
streams the bytes itself instead.

## Rejected: asking with the key's fields instead of the diff_id

`GET /diffs/v1?registry=npm&package=zod&from=…` would let this server build the
`DiffKey` and call `DiffStore::get` unchanged. It would also make the app's own
implementation of the key pointless, so the vectors would stop being what
holds the two in step, and every request would spell out the comparison a
visitor opened. The app is client-side by design, and asking by `diff_id`
keeps the package and the versions out of the request.

That is all it buys. It does not stop this server from knowing: on a hit it
reads an Entry that names its comparison, and on a miss the inputs are a name
and two versions, so a likely comparison can be hashed and checked. What the
request carries is less than the comparison; what the server can work out is
not.

The cost is that the server cannot check the asker's inputs. It checks the
Entry instead: an Entry is served only when its own key hashes to the
`diff_id` that was asked for.

## Rejected: one route per blob

Serving `meta.json` and `patches.json` at their pathnames would mirror the
store exactly. But half an Entry is not a hit (ADR 0003), and two routes would
hand the reader that decision. One route answers with a whole Entry or a miss.

## Consequences

- An Entry can be 8 MiB and a response 4.5 MB. Over the ceiling, the Entry is
  served without its patches, and the app computes the files it opens. A tree
  alone over the ceiling is a miss.
- A hit is `public, max-age=86400, s-maxage=31536000`: a day in a browser,
  and a year in Vercel's CDN, whose copy a deployment replaces. Not
  `immutable`: an Entry is write-once and a `diff_id` names its inputs, but
  the caps that shape a served body are not in the key, and a mistake in how
  one is assembled should not outlive a deployment in anyone's browser. A miss
  is `no-store`, so an Entry written later is seen.
- Any origin may read, with no credentials. What is served is a diff of two
  published packages, so there is nothing to keep from another site.
- Like the MCP endpoint (ADR 0018), the route has no rate limit. A request that
  misses costs a store lookup, and a hit costs reading up to an Entry's 8 MiB,
  once per CDN copy. Anyone can loop over made-up `diff_id`s; what that costs
  is Blob API operations, and it is the same exposure ADR 0018 accepts.
