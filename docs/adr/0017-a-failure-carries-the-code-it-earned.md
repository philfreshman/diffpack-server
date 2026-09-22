# 0017. A failure carries the code it earned, not the one its caller picked

**Status:** accepted, 2026-09-22. Implemented by #85.

Every `Failure` names its own JSON-RPC code, in one exhaustive match —
`Failure::channel` — beside the answer to whether a model can act on it. A
`tools/call` uses the second fact to choose between MCP's two channels; a
`resources/read`, which has only one, uses the first. Neither surface decides
anything.

Five codes. Two were already here and three are new:

| Code | What it says | Variants |
| :--- | :--- | :--- |
| `-32602` | Invalid params: the caller can fix this from what it was already told | `InvalidParams`, `NoSuchTool`, `NoSuchResource` |
| `-32000` | A fault of this server's | `Internal` |
| `-32001` | Ask for something else: the request was understood and there is no answer to it | `NoSuchPackage`, `NoSuchVersion`, `MalformedArchive`, `UnreadableVersions`, `VersionsTooLarge`, `UnreadableSearch`, `NoSuchFile`, `PathIsDirectory`, `UnresolvableArchiveUrl` |
| `-32003` | Try again: nothing was served, and another attempt might be | `RateLimited`, `TimedOut`, `Busy`, `Unreachable`, `Unavailable` |
| `-32004` | Ask for less: the answer is there, is over a limit, and has a narrower form | `TooLarge`, `SearchTooLarge`, `ItemTooLarge` |

The three new ones are the three remedies `Failure::message` already writes
out in prose. A transient failure's sentence says "again" and a permanent
one's does not — that distinction was there to save a model from parsing the
message, and on a read the model never gets the message at all. So the code
carries it, or nothing does.

## Why it was the call site's, and what that cost

`Failure::refuse` ran `respond` and re-emitted anything that came back as a
tool error under `-32602`. Its only caller was `read_resource`. So the same
failure had two identities: `NoSuchVersion` reached a `tools/call` as
`isError: true` with a sentence naming the versions that do exist, and reached
a `resources/read` as `-32602` — the code `NoSuchResource` also had.

*"npm has no version 9.9.9 of diffable"* and *"this URI is not one of ours"*
were one answer. A client that reads the envelope, which on a read is all
there is, could not tell a mistyped version from a URI it had built wrongly,
and the two have nothing in common as remedies. `tests/resources.rs` told them
apart by looking for a phrase in a message, which is the shape a test takes
when the thing it is about is not on the wire.

## Why these codes and not others

JSON-RPC 2.0 predefines six codes and reserves `-32000..-32099` for
implementation-defined server errors. Of the six:

* **`-32602` invalid params** is the right one for a URI that resolves to
  nothing, a tool name nothing answers to, and arguments a schema refused. All
  three are the caller's to fix from the lists this server already published,
  and `2026-07-28` moved resource-not-found here by name (SEP-2164) from
  `-32002`, which is where rmcp still sends it for a peer on an older
  revision. Nothing was gained by moving off it.
* **`-32601` method not found** describes the method, and `resources/read`
  exists. A URI is a parameter of it.
* **`-32603` internal error** is JSON-RPC's own, for a fault in the JSON-RPC
  machinery rather than in what the request asked for. `src/mcp.rs`'s panic
  guard uses it, which is exactly that case; a registry that timed out is not.
* **`-32700` parse error** and **`-32600` invalid request** are about the
  frame, which was fine in every case here.

That leaves the implementation range for everything that happened to a
well-formed request, which is what that range is for. Within it the MCP
specification has taken `-32020..-32099`, leaving `-32000..-32019`.

`-32002` is skipped inside that run, and the gap is the point: it is
`RESOURCE_NOT_FOUND`, and a peer below `2026-07-28` still receives it — rmcp
raises it to `-32602` only for one negotiating that revision or newer.
A code of ours in that slot would arrive at some clients as *no such resource*
— the one sentence these codes exist to stop being said about a version that
was simply never published.

## Rejected: an `isError` equivalent for a resource read

The reading that makes this problem disappear, and the reading the protocol
does not support. `ReadResourceResult` carries `contents` and nothing else.
There is no flag, no status field, and no second result type — a read either
answers with contents or is a JSON-RPC error.

Every way of inventing one is worse than the collapse it fixes. A document
with an `error` key inside `contents` is a *successful* read of a resource
that does not exist, which a generic client renders as content and caches
under this server's own `ttlMs`; a client that did not know the convention
would show a model a JSON object describing a failure as though it were the
comparison. A second MIME type says the same thing to the same client and is
no more visible to one that does not read our documentation. Both of them also
make a failure something a resource has to remember not to cache.

The honest version of this alternative is "resources are the wrong surface for
anything that can fail", and that is a re-opening of
[ADR 0014](0014-a-resource-is-a-projection-of-the-tools.md) rather than an
answer here: a resource is a projection of the tools, and the tools fail.

## Rejected: one code for everything that is not the caller's fault

`-32001` for all of it, which meets the criterion — a real failure is
distinguishable from a URI that resolves to nothing — with one constant
instead of three.

Against it: it stops at the criterion. The question a client asks next is not
"whose fault was this" but "what do I do now", and there are three answers, not
one. "Retry in ten seconds" and "this version was never published" are
opposite instructions, and a client that had to read prose to tell them apart
is back where `tests/resources.rs` was. The three codes cost nothing at the
call sites — no `Failure` gained a field — and one match arm each.

## Rejected: one code per variant

Twenty-one codes, one for each `Failure`, which is the most information the
wire can carry.

Against it: the variants are deliberately finer than the remedies. `TooLarge`,
`VersionsTooLarge` and `SearchTooLarge` are three variants because a `413`
means a different thing to each seam and the *message* has to say which — that
is the split #79 lists as a trap not to fold. Where two of them imply the same
action from a client, a code apiece would publish a distinction that exists
for the prose. It also makes every new variant a wire change, and a code
nobody branches on is a code that will be wrong.

The grouping is by remedy and not by cause, which is not the same cut, and
`VersionsTooLarge` is where the two come apart. It is the third of the `413`
seams and it does not take `-32004`: a package's release history has no
narrower form, and `Failure::message` has said so since it was written —
*"there is no shorter answer to ask for"*. A client reads `-32004` as *narrow
and ask again*, and there is nothing to narrow, so it would come back with the
identical call. It takes `-32001` with the rest of what this server has no
answer to, and the three seams keep their three messages. Being over a limit
is the cause; having something smaller to ask for is the remedy, and the code
carries the remedy.

## What this costs

The codes are a wire contract now, in a way `-32602`-for-everything was not.
Changing one later is a client-visible break, which is what puts this on the
hard-to-reverse side of the bar. The suite spells them as literals rather than
importing the constants, so a change has to be made twice on purpose.

It also changes the HTTP status of a failed read. rmcp maps `-32602` to `400`
and everything outside its short list to `200` with a JSON-RPC error in the
body, so a read that fails on a version the registry does not have moves from
`400` to `200`. That is the more accurate of the two — nothing was wrong with
the request — but it is a change an intermediary counting `4xx` will see.

And it leaves one pair sharing a code on purpose. `InvalidParams` and
`NoSuchResource` are both `-32602`, because both really are invalid
parameters: a URI this server does not serve and a percent-escape that decodes
to nothing are each the caller's to fix out of what it already has. The test
that used to separate them by prose now does it by the shape of the answer —
it reads a path that works through the same template first — which is a
stronger claim than the sentence was.
