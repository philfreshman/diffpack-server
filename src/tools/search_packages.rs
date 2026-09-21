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
//! # Where the descriptions come from
//!
//! Every doc comment on a field of [`Args`] becomes a `description` in a
//! schema a model reads, so it is written for that reader and names nothing
//! in this repository. `cursor` and `limit` have no doc comment on purpose:
//! they are [`crate::page`]'s types and that module writes their
//! descriptions, including the rule that an out-of-range `limit` is clamped
//! rather than refused.
//!
//! The one asymmetry an agent does have to know about is in the answer rather
//! than in the code — a PyPI hit carries a name and nothing else, because
//! PyPI's index carries nothing else — so the description says so out loud.

use serde::{Deserialize, Serialize};

use crate::error::Failure;
use crate::page::{self, Page};
use crate::registry::Registry;
use crate::tools::{Ctx, Tool};

/// The tool.
pub struct SearchPackages;

/// One package the search found.
// Unlike the doc comment on `Args`, this one reaches a model: an inlined
// schema keeps its `description` where a root schema's is stripped. So it is
// a sentence for that reader, and the note to the next reader of this file is
// the comment you are reading.
//
// Inline rather than a `$ref` into `$defs`, for the reason
// `Registry` and `page`'s two wire types are: the reader is a model, and a
// shape it has to resolve a reference to learn is a shape it will guess at.
// Three fields are cheaper to repeat than to look up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[schemars(inline)]
pub struct Hit {
    /// The package name, spelled the way the registry spells it. Pass it
    /// back verbatim to any tool that takes a package.
    pub name: String,

    /// The version the registry would install if you named none. Absent
    /// means this registry does not say here, not that the package has
    /// published nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// What the package says it is, in the registry's own words. Absent on
    /// registries that do not carry one here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

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
    /// a package's description. `http client`, `zod`, `yaml`. Blank finds
    /// nothing — there is no way to ask a registry for everything it has.
    pub query: String,

    // No doc comment on either, on purpose: `page` writes both, and a
    // sentence here would replace the one that carries the numbers that bind.
    // See `list_package_files`, which does the same for the same reason.
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
        package name to diff. Takes a registry and a query. Hits come back \
        best match first, each with the name to pass to the other tools. \
        npm and crates.io hits also carry the current version and the \
        package's own description; PyPI hits carry a name alone, because the \
        index PyPI publishes has nothing else in it. A query that matches \
        nothing is an empty list rather than an error. Each registry has its \
        own ceiling on one answer, under what you can ask for here: npm \
        gives at most 250 and crates.io at most 100, so the total is what \
        this search found and not how many the registry has.";

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
        let query = args.query.trim();

        // A blank query is not a search, and the three sources disagree about
        // what it is instead: PyPI's index matches every name it has, because
        // every name begins with nothing, and npm and crates.io each answer
        // with whatever a query-less query means to them. Answering it here
        // makes it one thing on all three — and the honest one, since a
        // caller that asked for nothing has not named a package.
        if query.is_empty() {
            return page::paginate(Vec::new(), args.limit, args.cursor);
        }

        // The limit is asked of the source rather than applied to what comes
        // back: npm and crates.io each take one, and asking for two hundred
        // to answer with ten is somebody else's bandwidth. `page` is what
        // says how many that is, so the number in the schema is the number
        // that travels.
        let wanted = page::wanted(args.limit);

        let hits = ctx.search().hits(args.registry, query, wanted).await?;

        // Mapped into this module's own shape rather than serialised where
        // it was built, so that every sentence a model reads about a hit is
        // written in the file the tool is in. `list_package_versions` does
        // the same with a version.
        let hits: Vec<Hit> = hits
            .into_iter()
            .map(|hit| Hit {
                name: hit.name,
                version: hit.version,
                description: hit.description,
            })
            .collect();

        // The sequence is already in the order it should be read in — each
        // source ranks its own answer, and PyPI's is ranked where it is
        // read. Paginating it is about the response ceiling rather than
        // about order: a page is what fits, and `total` is what says there
        // was more.
        //
        // The cursor is taken rather than refused because `paginate` can hand
        // one back — a page of long descriptions reaches the ceiling before
        // it reaches the limit — and a `nextCursor` a caller has nowhere to
        // send is a sequence this tool claims to have and does not. What it
        // resumes is this call's answer rather than the last one's: the
        // source is asked again, and a ranking that moved in between is the
        // idempotency hint above saying so.
        page::paginate(hits, args.limit, args.cursor)
    }
}
