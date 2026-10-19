use serde::{Deserialize, Serialize};

use crate::error::Failure;
use crate::page::{self, Page};
use crate::registry::{PackageName, Registry};
use crate::tools::{Call, Tool};

pub struct ListPackageVersions;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(
    description = "What a caller asks for.\n\
    \n\
    Nothing is normalised: the package name goes to the registry exactly as it\n\
    arrives, the rule `docs/cache-key.md` fixes for the cache key and every\n\
    other tool here follows."
)]
#[serde(deny_unknown_fields)]
pub struct Args {
    #[schemars(description = "The registry that publishes the package.")]
    pub registry: Registry,

    pub package: PackageName,

    #[serde(default)]
    pub cursor: Option<page::Cursor>,

    #[serde(default)]
    pub limit: Option<page::Limit>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(
    description = "What the registry says the package has.\n\
    \n\
    A wrapper around the page rather than a page on its own, because there are\n\
    two answers here and only one of them is a sequence. See the module\n\
    header."
)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    #[schemars(description = "The published versions, most recently published first.")]
    pub versions: Page<Version>,

    #[schemars(
        description = "The version this registry itself points at: the one it installs for\n\
        someone who names no version, and what to answer when you are asked\n\
        which version a package is on. It is **not** the first entry of\n\
        `versions` — that is the most recently published version, which on a\n\
        package with several live release lines is often a patch to an older\n\
        one.\n\
        \n\
        Beside the list rather than marked on an entry, so that asking for a\n\
        small page cannot hide it: it is the same answer whichever page you\n\
        asked for, and it may name a version that is not on this one.\n\
        \n\
        Absent where the registry names none."
    )]
    pub current_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[schemars(description = "One published version of the package.")]
#[serde(rename_all = "camelCase")]
pub struct Version {
    #[schemars(
        description = "The version as the registry spells it. Pass it back exactly as it is\n\
        written here: `v4.0.0` and `4.0.0` are different versions."
    )]
    pub version: String,

    #[schemars(
        description = "When the registry says this version was published, in the registry's\n\
        own wording. This is what the list is ordered by.\n\
        \n\
        Absent where the registry does not say. A version without one is\n\
        listed after every version that has one, so it is at the end of the\n\
        list without being the oldest release. Go's module proxy lists\n\
        versions without dates, so a Go list is in version order instead,\n\
        highest first."
    )]
    pub published_at: Option<String>,

    #[schemars(
        description = "Whether this is a preview — an alpha, a beta, a release candidate or\n\
        a development build — rather than a release. Use it to avoid diffing\n\
        against a preview by accident; do not use it to pick the version a\n\
        package is on, which is `currentVersion` and is often neither a\n\
        preview nor the newest entry."
    )]
    pub prerelease: bool,
}

impl Tool for ListPackageVersions {
    const NAME: &'static str = "list_package_versions";
    const TITLE: &'static str = "List package versions";
    const DESCRIPTION: &'static str = "\
        List the published versions of a package, most recently published \
        first, so you can find the versions to diff without being told them. \
        Takes a registry and a package name spelled the way the registry \
        spells it; when you are not sure of the spelling, find it with \
        `search_packages` first. The order is by publish date, not by \
        version number: a \
        patch to an older release line published yesterday comes before a \
        major released last month. Go is the exception: its module proxy \
        lists a module's versions without dates, so a Go module's are \
        highest version first. The release the registry itself installs \
        is a separate answer in the same result, `currentVersion`, and it is \
        often not the first entry — read it rather than the top of the list \
        when you want the version a package is on today.";

    const READ_ONLY: bool = true;

    const IDEMPOTENT: bool = true;

    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Output;

    async fn call(args: Args, call: &Call) -> Result<Output, Failure> {
        let listed = call.catalogue().versions(args.registry, args.package.as_str()).await?;

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
