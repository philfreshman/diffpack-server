//! `resolve_archive_url` — where a version's archive comes from.
//!
//! The cheapest tool this server has and the first one it got: it answers
//! from the package name and the version alone, without asking a registry
//! anything, so it costs a string and no network. What it buys an agent is
//! the ability to show its work — the diff it is reading came from *this*
//! file, at *this* URL — and what it buys #10 is a tool whose answer is the
//! same function the fetch path uses, rather than a second copy of a URL
//! pattern.
//!
//! # Two registries, not three
//!
//! npm and crates.io publish an archive at a path anyone can construct. PyPI
//! does not: a version's artefacts are listed in its own metadata and nowhere
//! else, so there is no URL to build from a name and a version. That is a
//! property of PyPI rather than something unfinished here, and the answer for
//! it is a tool error the model can act on. #10 adds the metadata hop, and
//! this tool starts answering for PyPI with no change to what it promises.
//!
//! # Where the URL comes from
//!
//! [`crate::registry`], which routes it through the engine — the same code
//! the browser runs. This module matches on no registry name of its own: what
//! a registry is has one home (ADR 0004, #42), and a copy here would be the
//! fifth one that ADR rejects.
//!
//! Which is also why the note about that below is a `//` comment and not a
//! `///` one: a doc comment on a field of [`Args`] reaches a model, and this
//! is a thing to know about the code rather than about the argument. See
//! [`crate::tools`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Failure;
use crate::registry::{ArchiveSource, Registry};
use crate::tools::{Call, Tool};

/// The tool.
pub struct ResolveArchiveUrl;

/// What a caller asks for.
///
/// Every field is required and none is normalised: the package name and the
/// version go to the registry exactly as they arrive, the same rule
/// `docs/cache-key.md` fixes for the cache key. `@types/node` keeps its
/// scope and `v4.0.0` keeps its `v`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// The registry that publishes the package: `npm` or `crates`.
    ///
    /// `pypi` parses and is a registry this server knows, but this tool
    /// cannot resolve a URL for it: PyPI lists a version's files in its
    /// metadata rather than serving them from a predictable path.
    // The enum itself comes from `crate::registry`, so the list an agent is
    // shown is the list this server has rather than a description of one.
    pub registry: Registry,

    /// The package name as the registry spells it, scope included:
    /// `zod`, `@types/node`, `serde`.
    pub package: String,

    /// The version as the registry spells it: `4.0.0`. Not a range, not a
    /// tag — one published version.
    pub version: String,
}

/// Where that version's archive is served from.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Output {
    /// The URL this server fetches the archive from.
    pub url: String,
}

impl Tool for ResolveArchiveUrl {
    const NAME: &'static str = "resolve_archive_url";
    const TITLE: &'static str = "Resolve archive URL";
    const DESCRIPTION: &'static str = "\
        Return the URL a published package version's archive is served from, \
        so a diff can name where its bytes came from. Works for `npm` and \
        `crates` (crates.io), which publish at a predictable path. Takes a \
        registry, a package name and one exact version, all spelled the way \
        the registry spells them. Fetches nothing: the answer is built from \
        the arguments.";

    /// It reads nothing and writes nothing — it builds a string.
    const READ_ONLY: bool = true;

    /// The same three arguments always give the same URL. The URL pattern is
    /// a registry's public contract, not something that varies per call.
    const IDEMPOTENT: bool = true;

    /// The arguments name a package on a registry, which is a world this
    /// server does not control — and this tool does not check that the
    /// version exists. A URL that resolves is not a promise of a 200.
    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Output;

    async fn call(args: Args, _call: &Call) -> Result<Output, Failure> {
        // `crate::registry` owns where an archive is; this tool owns which
        // of the two answers it can serve. A registry whose archive is
        // listed rather than built needs a fetch, and fetching is #10's.
        match args.registry.archive(&args.package, &args.version)? {
            ArchiveSource::Archive { url } => Ok(Output { url }),
            // The way forward is the registries this tool could have
            // answered for, asked of the same module with the same call
            // rather than written out as a sentence that #28 would have to
            // find and widen.
            ArchiveSource::Listing { .. } => Err(Failure::UnresolvableArchiveUrl {
                registry: args.registry.id().to_owned(),
                resolvable: Registry::ALL
                    .iter()
                    .filter(|registry| {
                        matches!(
                            registry.archive(&args.package, &args.version),
                            Ok(ArchiveSource::Archive { .. })
                        )
                    })
                    .map(|registry| registry.id().to_owned())
                    .collect(),
            }),
        }
    }
}
