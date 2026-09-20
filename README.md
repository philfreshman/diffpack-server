# diffpack-server

A placeholder so far: this repository holds one commit and this README. Nothing is built here yet,
and neither of the sibling repositories imports, calls or deploys anything from it.

## The sibling repositories

diffpack is three repositories meant to be checked out side by side under one parent directory:

```
<parent>/
├── diffpack/          github.com/philfreshman/diffpack
├── diffpack-engine/   github.com/philfreshman/diffpack-engine
└── diffpack-server/   github.com/philfreshman/diffpack-server   ← this one
```

| Repo | Sibling path | What lives there |
| --- | --- | --- |
| **diffpack** | `../diffpack` | The web app at [diffpack.io](https://www.diffpack.io) — TanStack Start on Vite, the UI, the registry adapters and the worker that drives the diff. Deployed to Vercel, where the server function renders the page shell and nothing more: package archives go from the registry straight to the browser. Read its `README.md`, `CONTRIBUTING.md` and `AGENTS.md`. |
| **diffpack-engine** | `../diffpack-engine` | The Rust crate compiled to WebAssembly that fetches, extracts and diffs package archives. Published to npm as `@philfreshman/diffpack-engine`; the app consumes a pinned version rather than building it. |
| **diffpack-server** | *this checkout* | Empty. |

Whatever lands here, the shape to keep in mind is that today no package content passes through a
diffpack server — the app is client-side by design, and the engine runs in the browser. Anything
built here changes that property, so it belongs in the other two repositories' docs as well.
