# The diff cache key

**Status:** normative, schema `1`.

This document is the contract between two codebases that will never share a
line of code: `diffpack-server` in Rust, which writes cached diff results, and
`diffpack` in TypeScript, which will read them. Neither implementation is the
source of truth — this document is, and
[`fixtures/cache-key-vectors.json`](../fixtures/cache-key-vectors.json) is the
file both sides are held to.

If the two sides compute keys differently, nothing breaks loudly. The cache
simply never hits, every diff is recomputed, and the only symptom is a bill.
That is why the vectors exist.

You should be able to implement this from this document alone, without reading
any Rust.

## The algorithm

1. Build the canonical string (below).
2. `diff_id = sha256(canonical)`, as **lowercase hex**, 64 characters.
3. The blob pathnames are:

   ```
   diffs/v1/{diff_id}/meta.json
   diffs/v1/{diff_id}/patches.json
   ```

   The `v1` segment tracks the `schema` field: `schema: 2` would store under
   `diffs/v2/`.

## The canonical string

Compact JSON — no whitespace anywhere, not between tokens and not after the
separators — with the eight keys below in **ASCII sort order**, encoded as
UTF-8 before hashing.

```
{"engine":"0.3.0","from":"3.25.76","ignore_whitespace":false,"package":"zod","registry":"npm","schema":1,"similarity_threshold":"0.7500","to":"4.0.0"}
```

ASCII sort order for these keys is fixed and total, so you can hard-code it:

```
engine, from, ignore_whitespace, package, registry, schema,
similarity_threshold, to
```

Do not reach for a generic "canonical JSON" library. The set of keys is closed
and their order is known, so building the string by concatenation is both
simpler and impossible to get subtly wrong at a library-version boundary.

### The fields

| Key | JSON type | Example | Why it is in the key |
| --- | --- | --- | --- |
| `engine` | string | `"0.3.0"` | The `diffpack-engine` version that computed the result. Rename detection and line counts are engine behaviour: without this field the cache would serve trees the current engine would not produce. |
| `from` | string | `"3.25.76"` | The base version. |
| `ignore_whitespace` | boolean | `false` | Changes statuses and line counts, so it changes the result. |
| `package` | string | `"zod"` | The package name, verbatim (see *Normalisation*). |
| `registry` | string | `"npm"` | One of `npm`, `crates`, `pypi`. |
| `schema` | number | `1` | The shape of the stored payload. Bumping it invalidates every entry at once. |
| `similarity_threshold` | string | `"0.7500"` | Rename-detection threshold, as a fixed-precision string (see below). |
| `to` | string | `"4.0.0"` | The target version. |

`registry` is an open enum in the sense that adding `go` later is additive: a
new value, no schema bump, because the registry is already part of the key.

The pair is **ordered**. `from` and `to` are not interchangeable: A→B and B→A
are different diffs and must be different keys.

### `similarity_threshold`: a string, never a float

The threshold is a float everywhere else in both systems. It is a string here,
and this is the one rule most likely to be implemented wrong.

Rust and JavaScript do not format floats identically, and the difference is not
always visible. `serde_json` and `JSON.stringify` agree on `0.75`; they do not
reliably agree once arithmetic has been anywhere near the value. A string
deletes the entire class of divergence rather than betting that the two
formatters stay in step.

**The rule:** format the float with exactly four digits after the decimal
point, rounding half away from zero, and emit it as a JSON string.

```
0.75        -> "0.7500"
0.8         -> "0.8000"
1           -> "1.0000"
0.12345     -> "0.1235"     (rounded)
0.750000001 -> "0.7500"     (rounded; see below)
```

In TypeScript, `value.toFixed(4)` is this rule for every value in range.
In Rust, `format!("{value:.4}")` is, with the one caveat below.

> **Rounding mode.** `toFixed` rounds half away from zero; Rust's `{:.4}`
> rounds half to even. They differ only for a value whose decimal expansion is
> *exactly* a half at the fifth place — which, for a binary float, essentially
> only happens for values a caller typed as a literal, such as `0.00005`. A
> threshold is a rename-detection knob in `0.0..=1.0` that users set in coarse
> steps, so the case is vanishingly rare; it is specified rather than left
> open so that an implementer who hits it has an answer. A Rust implementation
> must therefore round half away from zero explicitly rather than relying on
> `{:.4}`.

**A consequence, stated so it is not mistaken for a bug:** two thresholds that
agree to four decimal places produce the *same* key. `0.75` and `0.750000001`
are the same cache entry. That is the intent — four decimal places is a
deliberate choice about how finely the cache should be partitioned, and a
threshold difference of one part in a billion does not change a diff. If you
want those to be distinct entries, the precision is the thing to change, and
changing it is a `schema` bump.

## Normalisation, v1: verbatim

There is none. Names and versions go into the key exactly as the caller
supplied them:

- **No case folding.** `Zod` and `zod` are different keys.
- **No [PEP 503](https://peps.python.org/pep-0503/) normalisation for PyPI.**
  `typing_extensions`, `typing-extensions` and `Typing.Extensions` are three
  different keys, even though PyPI treats them as one project.
- **No `v`-stripping.** `v4.0.0` and `4.0.0` are different keys.
- **No scoped-name rewriting for npm.** `@types/node` goes in with the `@` and
  the `/` intact.

This is deliberate. The cost of being too strict is a cache miss — a diff gets
recomputed and the result is still correct. The cost of being too loose is
serving the wrong diff, which is a correctness bug a user may never notice.
v1 takes the cheap failure.

Normalisation can arrive later as `schema: 2`, at which point the rules go in
this document and the vectors grow to cover them.

## The vectors

`fixtures/cache-key-vectors.json` holds input tuples with their expected
`canonical` string and `diff_id`. Both implementations test against it.

Each vector carries the canonical string as well as the hash so that a failure
localises: if `canonical` matches and `diff_id` does not, the bug is in your
hashing or your hex encoding, not in your field order.

The expected values in that file were produced from this document, by an
implementation written for that purpose and thrown away — not by running
either of the two implementations it checks. A vector file generated by the
code it tests asserts only that the code is unchanged.
