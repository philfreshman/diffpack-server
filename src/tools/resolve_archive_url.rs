use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Failure;
use crate::registry::{ArchiveSource, PackageName, Registry, VersionName};
use crate::tools::{Call, Tool};

pub struct ResolveArchiveUrl;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(
    description = "What a caller asks for.\n\
    \n\
    Every field is required and none is normalised: the package name and the\n\
    version go to the registry exactly as they arrive, the same rule\n\
    `docs/cache-key.md` fixes for the cache key. `@types/node` keeps its\n\
    scope and `v4.0.0` keeps its `v`."
)]
#[serde(deny_unknown_fields)]
pub struct Args {
    #[schemars(
        description = "The registry that publishes the package: `npm` or `crates`.\n\
        \n\
        `pypi` parses and is a registry this server knows, but this tool\n\
        cannot resolve a URL for it: PyPI lists a version's files in its\n\
        metadata rather than serving them from a predictable path."
    )]
    pub registry: Registry,

    pub package: PackageName,

    pub version: VersionName,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(description = "Where that version's archive is served from.")]
pub struct Output {
    #[schemars(description = "The URL this server fetches the archive from.")]
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
        the arguments. To see what is inside a version, call \
        `list_package_files` instead.";

    const READ_ONLY: bool = true;

    const IDEMPOTENT: bool = true;

    const OPEN_WORLD: bool = false;

    type Args = Args;
    type Output = Output;

    async fn call(args: Args, _call: &Call) -> Result<Output, Failure> {
        match args.registry.archive(args.package.as_str(), args.version.as_str())? {
            ArchiveSource::Archive { url } => Ok(Output { url }),
            ArchiveSource::Listing { .. } => Err(Failure::UnresolvableArchiveUrl {
                registry: args.registry.id().to_owned(),
                resolvable: Registry::ALL
                    .iter()
                    .filter(|registry| {
                        matches!(
                            registry.archive(args.package.as_str(), args.version.as_str()),
                            Ok(ArchiveSource::Archive { .. })
                        )
                    })
                    .map(|registry| registry.id().to_owned())
                    .collect(),
            }),
        }
    }
}
