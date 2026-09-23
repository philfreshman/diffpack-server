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
//! # Why the answer is not just a page
//!
//! There are two answers here and only one of them is a sequence. Beside the
//! versions is the one the registry itself points at — `npm install` with no
//! version, `cargo add`, `pip install` — and on a package with several live
//! release lines it is not the first entry. `@types/node` publishes a 24.x
//! patch minutes after a 26.x release most weeks, so an agent handed the top
//! of this list as the current release is wrong most weeks.
//!
//! It is a field beside the page rather than a flag on each entry because a
//! flag paginates badly: the answer is a [`Page`], so a package whose pointer
//! is on the third page answers `limit: 5` with five falses — truthfully,
//! and indistinguishably from a package the registry points at nothing for.
//! [`Page`] itself is untouched, because it is shared by every listing tool
//! and a tool-specific field on it is what [ADR
//! 0005](../../docs/adr/0005-one-module-owns-the-response-ceiling.md) rules
//! out.
//! What is left is a wrapper — [`Output`] — which is the shape
//! `diff_package_versions` already has.
//!
//! # Where the descriptions come from
//!
//! Every doc comment on a field of [`Args`], [`Output`] and [`Version`]
//! becomes a `description` in a schema a model reads, so it is written for
//! that reader and names nothing in this repository. `cursor` and `limit`
//! have no doc comment on purpose: they are [`crate::page`]'s types and that
//! module writes their descriptions, including the rule that an out-of-range
//! `limit` is clamped rather than refused.

use serde::{Deserialize, Serialize};

use crate::error::Failure;
use crate::page::{self, Page};
use crate::registry::Registry;
use crate::tools::{Call, Tool};

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

/// What the registry says the package has.
///
/// A wrapper around the page rather than a page on its own, because there are
/// two answers here and only one of them is a sequence. See the module
/// header.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    /// The published versions, most recently published first.
    pub versions: Page<Version>,

    /// The version this registry itself points at: the one it installs for
    /// someone who names no version, and what to answer when you are asked
    /// which version a package is on. It is **not** the first entry of
    /// `versions` — that is the most recently published version, which on a
    /// package with several live release lines is often a patch to an older
    /// one.
    ///
    /// Beside the list rather than marked on an entry, so that asking for a
    /// small page cannot hide it: it is the same answer whichever page you
    /// asked for, and it may name a version that is not on this one.
    ///
    /// Absent where the registry names none.
    pub current_version: Option<String>,
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
    /// a development build — rather than a release. Use it to avoid diffing
    /// against a preview by accident; do not use it to pick the version a
    /// package is on, which is `currentVersion` and is often neither a
    /// preview nor the newest entry.
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
        major released last month. The release the registry itself installs \
        is a separate answer in the same result, `currentVersion`, and it is \
        often not the first entry — read it rather than the top of the list \
        when you want the version a package is on today.";

    /// It reads a registry's metadata; it changes nothing anywhere.
    const READ_ONLY: bool = true;

    /// The same arguments give the same answer until somebody publishes,
    /// which is the sense of idempotent a client caches on.
    const IDEMPOTENT: bool = true;

    /// The arguments name a package on a registry, which is a world this
    /// server does not control.
    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Output;

    async fn call(args: Args, call: &Call) -> Result<Output, Failure> {
        let listed = call.catalogue().versions(args.registry, &args.package).await?;

        let versions: Vec<Version> = listed
            .all
            .into_iter()
            .map(|version| Version {
                version: version.version,
                published_at: version.published_at,
                prerelease: version.prerelease,
            })
            .collect();

        Ok(Output {
            versions: page::paginate(versions, args.limit, args.cursor)?,
            current_version: listed.current,
        })
    }
}
