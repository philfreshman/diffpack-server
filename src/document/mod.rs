//! Whatever a registry serves at a URL, and what this server will hold of it.
//!
//! Three seams ask a registry for something — [`crate::archive`] for a
//! version's files, [`crate::catalogue`] for a package's releases,
//! [`crate::search`] for the packages that answer to a query — and before any
//! of them does the thing that makes it different, all three do the same four:
//! check the URL against the hosts [`crate::registry`] names, go to a live
//! registry or to a fixture set, weigh what came back against the cap, and
//! hand the bytes on. Only the last step is genuinely per-seam: one extracts
//! an archive, one reads a version list, one reads hits.
//!
//! This module is those four steps, once. What was three copies of them is
//! now three calls to [`Document::body`], and the copies that had already
//! started to drift — three fixture readers of one index format, in 73, 57
//! and 67 lines — are one reader.
//!
//! # This is not a fourth seam
//!
//! [ADR 0011](../docs/adr/0011-what-a-registry-publishes-is-its-own-seam.md)
//! settled that what a registry publishes is its own seam, and nothing here
//! reopens it. `archive`, `catalogue` and `search` are still three
//! interfaces, still answer three questions, and still fail in three
//! directions. 0011 rejected *one seam for both questions* — one interface
//! callers would ask both through. This is one implementation underneath
//! three interfaces, which is the opposite arrangement: a caller cannot name
//! this module at all. `scripts/check-tool-seams.sh` keeps it that way, and
//! it is deliberately absent from that script's list for the reason
//! [`crate::fetch`] is — a tool that could name either would be a tool that
//! can fetch. [ADR
//! 0015](../docs/adr/0015-one-implementation-beneath-three-registry-seams.md)
//! has the rest.
//!
//! # Why the caller supplies the refusals
//!
//! [`crate::fetch`] already takes its refusals as parameters, because a `404`
//! means a missing version to one seam and a missing package to another, and
//! a single "not found" for both is a message a model cannot act on. Every
//! word of that argument applies one level up, to the two refusals `fetch`
//! never sees: a URL off the allowlist, and a fixture set that says a URL
//! serves nothing. So [`About`] is `fetch::About` plus the questions a host
//! check and a fixture index ask, and this module builds nothing a caller
//! could have named better.
//!
//! What it does *not* take is a way to read the body. That half stays with
//! the seam, and it is why there are still three of them.
//!
//! # A body is weighed once
//!
//! There is one comparison against the cap in this module, in
//! [`Document::weighed`], and every body that was not streamed goes through
//! it exactly once.
//!
//! A body that *was* streamed does not, and that is the point rather than an
//! omission. [`crate::fetch`] refuses one twice over on the way in — on a
//! declared length before a byte is read, and on a running total as the
//! chunks arrive — which is a cap applied before the memory it guards has
//! been spent. Weighing the result again is a comparison that cannot fire.
//! `archive`, `catalogue` and `search` each made it anyway, with the same
//! number and the same constructor, in the one place a reader would go
//! looking for the rule.
//!
//! Deleting it outright was not the fix. Nothing streams a fixture body and
//! nothing streams one this instance is already holding, so that comparison
//! was also the only thing making the cap a rule about what this server will
//! read rather than about where bytes came from — a fixture set that answered
//! with something the registries would have been refused for would otherwise
//! go straight through. So it is kept, once, on the paths that need it.
//!
//! # Two adapters, as a variant each
//!
//! [`Document::live`] fetches from the registries. [`Document::fixture`]
//! reads bodies from a directory on disk, keyed by URL, which is what lets
//! every suite assert what a registry answers with no network. A variant each
//! rather than a trait, for the reason [ADR
//! 0004](../docs/adr/0004-one-registry-module.md) gives: neither can arrive
//! from outside this crate, so the extensibility a trait buys has no buyer.
//!
//! The key being a *URL* is load-bearing and not a convenience. A fetch path
//! that built a URL of its own rather than asking `registry` finds nothing in
//! a fixture set, so an offline test of any of the three seams is a test of
//! where that registry is asked as well as of what comes back.

mod fixture;
mod live;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::error::Failure;
use crate::registry::{self, Registry};

/// Where a registry document comes from.
#[derive(Debug)]
pub struct Document {
    source: Source,
    /// The most a body may weigh before this server refuses it. A field
    /// rather than a constant read at the point of use, so that the refusal
    /// can be exercised with a real body and a small limit instead of with a
    /// package nobody wants to download in a test. The number itself is the
    /// asking seam's — an 80 MB crate is an ordinary thing to diff and an
    /// 80 MB list of version numbers is not an ordinary anything — so each
    /// one names its own and this module only applies it.
    limit: u64,
}

/// The two adapters, as a variant each rather than a trait.
#[derive(Debug)]
enum Source {
    Live(live::Live),
    Fixture(fixture::Fixture),
}

/// What a request is for, in the words its failures need.
///
/// [`crate::fetch::About`]'s five fields and three more, and every one of the
/// eight is a question this module has no way to answer for a caller. The
/// three that are not `fetch`'s are the two refusals it never sees — a URL
/// off the allowlist, and a fixture index that says a URL serves nothing —
/// and the one policy that has to sit under the cap rather than over it.
pub struct About<'a> {
    /// The registry being asked, for the messages that name it.
    pub registry: Registry,

    /// What the request says it will take back, where the source serves more
    /// than one thing at the same URL. `None` is every source that serves one
    /// representation. See [`crate::fetch::About::accept`].
    pub accept: Option<&'static str>,

    /// Whether this request may be answered compressed. See
    /// [`crate::fetch::About::compressed`], which is where the trade is.
    pub compressed: bool,

    /// How long a body fetched from this URL may be handed to a later call
    /// rather than fetched again.
    ///
    /// `None` for every source but one, and that one is PyPI's index: a
    /// source that answers every query with the same whole document is the
    /// one worth holding between invocations, where a source that answers a
    /// query is not. Whether a source is like that, and for how long its
    /// answer stays current, are both `crate::search`'s to say — the duration
    /// is that module's constant, read off PyPI's own `cache-control` by a
    /// person rather than off each response by this code.
    ///
    /// It is a parameter here rather than a memo in the asking seam because
    /// of where the cap is. A body handed back from a warm instance is still
    /// a body this server is holding, and a seam built `with_limit` has to be
    /// able to refuse it — so the holding has to happen *below* the weighing,
    /// and the weighing is this module's.
    pub remember_for: Option<Duration>,

    /// What the status is, when a fixture set says a URL serves nothing.
    ///
    /// `null` in a fixture index is a third answer beside "here are the
    /// bytes" and "this URL is not in the set", and a real one: the registry
    /// serves nothing there. It is how an offline suite reaches the path a
    /// refusal takes without one, so what it stands in with is the number a
    /// registry would have answered with — a `404` for a version or a package
    /// that does not exist, a `503` for a search source having a bad day. It
    /// goes to [`missing`](Self::missing), which is the live adapter's own
    /// constructor, so a fixture set cannot answer differently from the
    /// registries.
    pub nothing_there: u16,

    /// What to return when the URL is not one this server may fetch from.
    ///
    /// Every outbound request is checked, and not only the one that could
    /// plausibly be wrong: a URL this crate built is allowed by construction,
    /// and a URL out of a document somebody else serves is not. Checking both
    /// is one line and leaves nothing to keep in step — and it is the line
    /// that stops a registry naming another host from turning a package name
    /// in a tool argument into a request wherever it liked.
    pub blocked: &'a (dyn Fn() -> Failure + Send + Sync),

    /// What to return when the registry says there is no such thing, given
    /// the status it said it with. See [`crate::fetch::About::missing`].
    pub missing: &'a (dyn Fn(u16) -> Failure + Send + Sync),

    /// What to return when the body is larger than the cap, given its weight.
    pub too_large: &'a (dyn Fn(u64) -> Failure + Send + Sync),
}

impl Document {
    /// Documents from the registries, which is what production runs.
    ///
    /// The cap is an argument and not a default, so there is no way to build
    /// one of these without saying what it will hold. Which number that is
    /// belongs to the seam asking.
    pub fn live(limit: u64) -> Self {
        Self {
            source: Source::Live(live::Live::new()),
            limit,
        }
    }

    /// Documents read from `dir` rather than from a registry.
    ///
    /// `dir` holds an `index.json` mapping a URL to the file beside it that
    /// stands in for what that URL serves, or to `null` for a URL that serves
    /// nothing.
    ///
    /// `doing` names the set, for the one failure a fixture adapter has that
    /// a live one does not: a URL the index never mentions is a hole in the
    /// fixture set rather than something a model can act on, so it takes the
    /// internal channel — and an operator reading that line needs to know
    /// which of the three sets was short.
    pub fn fixture(dir: impl Into<PathBuf>, doing: &'static str, limit: u64) -> Self {
        Self {
            source: Source::Fixture(fixture::Fixture::new(dir.into(), doing)),
            limit,
        }
    }

    /// The same source, refusing anything over `limit` bytes.
    pub fn with_limit(self, limit: u64) -> Self {
        Self { limit, ..self }
    }

    /// The cap in force, for the refusal that names it.
    ///
    /// A caller reads it rather than carrying its own copy beside the one it
    /// handed over, because two numbers that have to agree are two numbers
    /// that can stop agreeing — and the one a refusal names has to be the one
    /// that was applied.
    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// Whatever is served at `url`, or the refusal `about` names for what
    /// went wrong.
    pub async fn body(&self, url: &str, about: &About<'_>) -> Result<Body, Failure> {
        if !registry::allows(url) {
            return Err((about.blocked)());
        }

        match &self.source {
            Source::Live(live) => match live.held(url, about) {
                // Fetched for an earlier call and still current. Nothing
                // streamed it *for this* call, so it is weighed here with
                // everything else this module did not stream.
                Some(body) => self.weighed(body, about),

                // Weighed as it arrives, by `fetch`: a declared length is
                // refused before a byte of the body is read and the running
                // total is stopped at the limit, which is a cap applied
                // before the memory it guards has been spent rather than
                // after. A body that gets back here is one that already
                // passed, so weighing it again could only agree.
                None => live.fetch(url, self.limit, about).await,
            },

            // Nothing streamed these either.
            Source::Fixture(fixture) => self.weighed(fixture.body(url, about)?, about),
        }
    }

    /// `body`, unless it is more than this server will hold.
    ///
    /// The one comparison against the cap in this module, and the reason the
    /// three that used to sit in `archive`, `catalogue` and `search` are
    /// gone: on the live path they restated a refusal `fetch` had already
    /// made, and on every other path they were the only thing making the cap
    /// a rule about what this server will read rather than about where bytes
    /// came from.
    fn weighed(&self, body: Body, about: &About<'_>) -> Result<Body, Failure> {
        let weight = body.len() as u64;
        if weight > self.limit {
            return Err((about.too_large)(weight));
        }
        Ok(body)
    }
}

/// Whatever a registry served, as bytes, and whether this call is the only
/// thing holding it.
///
/// One type over two, because one of the sources is a whole index that a warm
/// instance holds between invocations and the rest are documents read for one
/// call and nobody else's. Handing every caller a shared body would copy an
/// archive of up to the cap on its way out of the fetch that produced it, and
/// handing every caller its own would copy PyPI's 44 MB index once per search
/// — which is the saving the holding was for, spent again.
///
/// It derefs to the bytes, so a caller reads one the way it reads the other
/// and no seam has to know which it was given.
pub enum Body {
    /// Read for this call, and nobody else is holding it.
    Owned(Vec<u8>),
    /// Held between invocations, and handed out by the pointer.
    Shared(Arc<[u8]>),
}

impl std::ops::Deref for Body {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::Shared(bytes) => bytes,
        }
    }
}

/// What it weighs and not what is in it.
///
/// A body is up to the cap in bytes and most of them are not text, so the
/// derived version would put a hundred megabytes of an archive into whatever
/// printed it.
impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Body({} bytes)", self.len())
    }
}
