//! `search_packages` — finding a package from a name half-remembered.
//!
//! The half of discovery that does not start with knowing what a thing is
//! called. Every other tool here takes a package name spelled exactly the way
//! its registry spells it, which is a thing an agent has to be handed by
//! somebody; this is where it can find one out for itself.
//!
//! # What it does not promise
//!
//! The same answer twice. A registry's index moves under a search — packages
//! are published, rankings shift — which is why this is the one tool here
//! whose idempotency hint is false while every other one's is true. A
//! published version's contents cannot change; what a registry has today can.
//!
//! # Where the three answers come from
//!
//! [`crate::registry`], which holds the source and the reading of it for each
//! registry, and [`crate::search`], which makes the request. This module
//! matches on no registry of its own: what a registry is has one home (ADR
//! 0004), and a search that knew PyPI needed treating differently would be a
//! second copy of that.
//!
//! The one asymmetry an agent does have to know about is in the answer rather
//! than in the code — a PyPI hit carries a name and nothing else, because
//! PyPI's index carries nothing else — so the description says so out loud.

use serde::Deserialize;

use crate::error::Failure;
use crate::page::{self, Page};
use crate::registry::{Hit, Registry};
use crate::tools::{Ctx, Tool};

/// The tool.
pub struct SearchPackages;

/// What a caller asks for.
///
/// The doc comments below are read by a model — see [`crate::tools`].
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// The registry to search.
    // The enum itself comes from `crate::registry`, so the list an agent is
    // shown is the list this server has rather than a description of one.
    pub registry: Registry,

    /// What to look for: a name, part of one, or a word that would appear in
    /// a package's description. `http client`, `zod`, `yaml`.
    pub query: String,

    // No doc comment, on purpose: `page` writes this one, and a sentence here
    // would replace the one that carries the numbers that bind. See
    // `list_package_files`, which does the same for the same reason.
    #[serde(default)]
    pub limit: Option<page::Limit>,
}

impl Tool for SearchPackages {
    const NAME: &'static str = "search_packages";
    const TITLE: &'static str = "Search packages";
    const DESCRIPTION: &'static str = "\
        Find packages on a registry from a name you only half remember, or \
        from a word that describes what they do, so you can get an exact \
        package name to diff. Takes a registry and a query. Hits come back \
        best match first, each with the name to pass to the other tools. \
        npm and crates.io hits also carry the current version and the \
        package's own description; PyPI hits carry a name alone, because the \
        index PyPI publishes has nothing else in it. A query that matches \
        nothing is an empty list rather than an error.";

    /// It reads a registry's index; it changes nothing anywhere.
    const READ_ONLY: bool = true;

    /// The one tool here that is not. A registry's search index moves — new
    /// packages, new rankings — so the same query tomorrow is a fair
    /// question with a possibly different answer, unlike a published
    /// version's contents.
    const IDEMPOTENT: bool = false;

    /// The query names nothing this server minted; what answers it is
    /// whatever the registry has.
    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Page<Hit>;

    async fn call(args: Args, ctx: &Ctx) -> Result<Page<Hit>, Failure> {
        // The limit is asked of the source rather than applied to what comes
        // back: npm and crates.io each take one, and asking for two hundred
        // to answer with ten is somebody else's bandwidth. `page` is what
        // says how many that is, so the number in the schema is the number
        // that travels.
        let wanted = page::wanted(args.limit);

        let hits = ctx.search().hits(args.registry, &args.query, wanted).await?;

        // The sequence is already in the order it should be read in — each
        // source ranks its own answer, and PyPI's is ranked where it is
        // read. Paginating it is about the response ceiling rather than
        // about order: a page is what fits, and `total` is what says there
        // was more.
        page::paginate(hits, args.limit, None)
    }
}
