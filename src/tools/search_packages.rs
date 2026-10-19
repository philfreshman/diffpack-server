use serde::{Deserialize, Serialize};

use crate::error::Failure;
use crate::page::{self, Page};
use crate::registry::Registry;
use crate::tools::{Call, Tool};

pub struct SearchPackages;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[schemars(description = "One package the search found.")]
#[schemars(inline)]
pub struct Hit {
    #[schemars(
        description = "The package name, spelled the way the registry spells it. Pass it\n\
        back verbatim to any tool that takes a package."
    )]
    pub name: String,

    #[schemars(
        description = "The version the registry would install if you named none. Absent\n\
        means this registry does not say here, not that the package has\n\
        published nothing."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    #[schemars(
        description = "What the package says it is, in the registry's own words. Absent on\n\
        registries that do not carry one here."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(
    description = "What a caller asks for.\n\
    \n\
    The doc comments below are read by a model — see [`crate::tools`]."
)]
#[serde(deny_unknown_fields)]
pub struct Args {
    #[schemars(description = "The registry to search.")]
    pub registry: Registry,

    #[schemars(
        description = "What to look for: a name, part of one, or a word that would appear in\n\
        a package's description. `http client`, `zod`, `yaml`. Blank finds\n\
        nothing — there is no way to ask a registry for everything it has."
    )]
    pub query: String,

    #[serde(default)]
    pub cursor: Option<page::Cursor>,

    #[serde(default)]
    pub limit: Option<page::Limit>,
}

impl Tool for SearchPackages {
    const NAME: &'static str = "search_packages";
    const TITLE: &'static str = "Search packages";
    const DESCRIPTION: &'static str = "\
        Find packages on a registry from a name you only half remember, or \
        from a word that describes what they do, so you can get an exact \
        package name to diff. Once you have the exact name, call \
        `list_package_versions` instead: a search is for finding a name, not \
        its versions. Takes a registry and a query. Hits come back \
        best match first, each with the name to pass to the other tools. \
        npm and crates.io hits also carry the current version and the \
        package's own description; PyPI hits carry a name alone, because the \
        index PyPI publishes has nothing else in it. Go has no search at \
        all, so a Go query has to be a whole module path, such as \
        `github.com/go-chi/chi/v5`: the one hit is that module with the \
        version `go get` installs, and anything else finds nothing. A query \
        that matches nothing is an empty list rather than an error. Each registry has its \
        own ceiling on one answer, under what you can ask for here: npm \
        gives at most 250 and crates.io at most 100, so the total is what \
        this search found and not how many the registry has.";

    const READ_ONLY: bool = true;

    const IDEMPOTENT: bool = false;

    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Page<Hit>;

    async fn call(args: Args, call: &Call) -> Result<Page<Hit>, Failure> {
        let query = args.query.trim();

        if query.is_empty() {
            return page::paginate(Vec::new(), args.limit, args.cursor);
        }

        let wanted = page::wanted(args.limit);

        let hits = call.search().hits(args.registry, query, wanted).await?;

        let hits: Vec<Hit> = hits
            .into_iter()
            .map(|hit| Hit {
                name: hit.name,
                version: hit.version,
                description: hit.description,
            })
            .collect();

        page::paginate(hits, args.limit, args.cursor)
    }
}
