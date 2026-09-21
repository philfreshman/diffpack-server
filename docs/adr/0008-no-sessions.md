# 0008. No sessions, for any client

**Status:** accepted, 2026-09-17 (#6). Recorded here in 2026-09-21.

`/mcp` is stateless. The transport is configured with
`NeverSessionManager` and `with_legacy_session_mode(false)`, so no answer ever
carries an `Mcp-Session-Id` — not for a client on revision `2026-07-28`, which
has no protocol-level sessions anyway, and not for one on `2025-11-25`, which
does.

The deployment is what decides this. Two invocations of a Vercel function share
no memory and there is no warm process between them, so a session id is a
promise the deployment cannot keep. Saying so in the type means the answer to
"where did the session go" is that there was never one to go.

## Rejected: the in-memory session manager with sessions switched off

`rmcp`'s default for older revisions: mint an id, keep the state in a local
map. It is the path of least resistance and it appears to work in a test, where
one process serves every request.

In production it is a map that is always empty by the time it is read. The
client is handed an id, sends it on the next call, reaches a different
invocation that has never heard of it, and gets an error that names a session —
which sends whoever is debugging it looking for a session store that does not
exist. A feature that works on a developer's machine and not in production is
worse than one that is absent in both.

## Rejected: refusing older revisions

Narrowing `supported_protocol_versions` to the revision that has no sessions
would make the statelessness a protocol fact rather than a configuration one.

The clients that matter are not all on the current revision, this server holds
no state an older revision's session rules could contradict, and a client
turned away here has no fallback: there is one endpoint. Serving them
statelessly costs nothing, because there is nowhere for their session to live
either.
