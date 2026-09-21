//! `list_package_versions` — what a package has released.
//!
//! The tool that makes the natural question askable: *what changed in the
//! last two releases?* Without it an agent has to be handed exact version
//! strings by a person, because the engine does no discovery at all.
//!
//! # What "newest first" means
//!
//! Most recently published first — not the highest version number. The two
//! are different often enough to matter: npm's `@types/node` publishes a 22.x
//! patch after a 26.x release most weeks, and a crate's 1.52.x backport lands
//! after 1.53.0. Both registries' own listings show the most recent publish on
//! top, and an agent asked for "the last two releases" means the last two
//! that happened.
//!
//! Where that order comes from is [`crate::registry`]'s, because it is a
//! different question per registry and the answer is a date rather than a
//! direction to read in.
//!
//! # Where the descriptions come from
//!
//! Every doc comment on a field of [`Args`] and [`Version`] becomes a
//! `description` in a schema a model reads, so it is written for that reader
//! and names nothing in this repository. `cursor` and `limit` have no doc
//! comment on purpose: they are [`crate::page`]'s types and that module
//! writes their descriptions, including the rule that an out-of-range `limit`
//! is clamped rather than refused.

use serde::{Deserialize, Serialize};

use crate::error::Failure;
use crate::page::{self, Page};
use crate::registry::Registry;
use crate::tools::{Ctx, Tool};

/// The tool.
pub struct ListPackageVersions;

/// What a caller asks for.
///
/// Nothing is normalised: the package name goes to the registry exactly as it
/// arrives, the rule `docs/cache-key.md` fixes for the cache key and every
/// other tool here follows.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// The registry that publishes the package.
    pub registry: Registry,

    /// The package name as the registry spells it, scope included:
    /// `zod`, `@types/node`, `serde`.
    pub package: String,

    // No doc comment on either of these, on purpose: see the module header.
    #[serde(default)]
    pub cursor: Option<page::Cursor>,

    #[serde(default)]
    pub limit: Option<page::Limit>,
}

/// One published version of the package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    /// The version as the registry spells it. Pass it back exactly as it is
    /// written here: `v4.0.0` and `4.0.0` are different versions.
    pub version: String,

    /// When the registry says this version was published, in the registry's
    /// own wording. This is what the list is ordered by.
    ///
    /// Absent where the registry does not say. A version without one is
    /// listed after every version that has one, so it is at the end of the
    /// list without being the oldest release.
    pub published_at: Option<String>,

    /// Whether this is a preview — an alpha, a beta, a release candidate or
    /// a development build — rather than a release. Asked for "the latest
    /// version", prefer the newest entry where this is false.
    pub prerelease: bool,
}

impl Tool for ListPackageVersions {
    const NAME: &'static str = "list_package_versions";
    const TITLE: &'static str = "List package versions";
    const DESCRIPTION: &'static str = "\
        List the published versions of a package, most recently published \
        first, so you can find the versions to diff without being told them. \
        Takes a registry and a package name spelled the way the registry \
        spells it. The order is by publish date, not by version number: a \
        patch to an older release line published yesterday comes before a \
        major released last month.";

    /// It reads a registry's metadata; it changes nothing anywhere.
    const READ_ONLY: bool = true;

    /// The same arguments give the same answer until somebody publishes,
    /// which is the sense of idempotent a client caches on.
    const IDEMPOTENT: bool = true;

    /// The arguments name a package on a registry, which is a world this
    /// server does not control.
    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Page<Version>;

    async fn call(args: Args, ctx: &Ctx) -> Result<Page<Version>, Failure> {
        let versions = ctx.catalogue().versions(args.registry, &args.package).await?;

        let versions: Vec<Version> = versions
            .into_iter()
            .map(|version| Version {
                version: version.version,
                published_at: version.published_at,
                prerelease: version.prerelease,
            })
            .collect();

        page::paginate(versions, args.limit, args.cursor)
    }
}
