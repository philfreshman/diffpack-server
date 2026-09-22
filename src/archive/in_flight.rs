//! What this process may have arriving at once.
//!
//! The size cap in [`super`] bounds one body. This bounds every body that is
//! arriving at the same moment, added up — which is the number that decides
//! whether this function runs out of memory, because two downloads that are
//! each inside the cap are twice the cap between them.
//!
//! # Why this is bytes rather than a count of downloads
//!
//! A count is the shape the guard is usually written in, and here it would
//! guard nothing. One tool asks for two archives at once — `try_join!` in
//! `diff_package_versions` — and nothing in this server asks for three, so a
//! semaphore of two permits admits every download there has ever been. It
//! would look like a cap, pass a test that counted, and bound nothing.
//!
//! What a download costs is bytes, so bytes are what is reserved. How many
//! downloads that allows is arithmetic rather than a number somebody chose:
//! [`super::IN_FLIGHT_LIMIT`] divided by what each one is allowed to weigh.
//! Move either and the other follows, which is the property a count does not
//! have.
//!
//! # What a reservation is worth
//!
//! The worst case, not the real weight. A download's real weight is in its
//! `Content-Length`, which arrives after the request has been sent and is
//! [`crate::fetch`]'s to see rather than this module's — so what is reserved
//! here is the most the body is *allowed* to be, and a small archive holds a
//! large archive's room for as long as it is arriving. ADR 0013 records the
//! finer version and what it would cost.
//!
//! A reservation is waited for rather than refused. A download that arrived
//! while another was in flight is nobody's mistake — unlike a body over the
//! cap, which is a package this server will not hold whenever it is asked —
//! so the answer is the same answer, slightly later.

use tokio::sync::{Semaphore, SemaphorePermit};

/// The bytes this process may have arriving at once.
///
/// One of these is shared by every [`super::Archive`] in the process — see
/// [`super::Archive::live`] — because the memory it is protecting is the
/// process's and not one request's. A budget per request would be as many
/// budgets as there are requests, which is no budget.
#[derive(Debug)]
pub struct InFlight {
    /// One permit per byte. Bytes rather than megabytes because the size cap
    /// is in bytes and the arithmetic between the two should need no unit
    /// conversion to read.
    bytes: Semaphore,

    /// What the budget was built with, kept because a reservation larger than
    /// the whole budget can never be granted and waiting for one is a hang
    /// rather than a guard. See [`InFlight::reserve`].
    total: u32,
}

impl InFlight {
    /// A budget of `bytes`.
    ///
    /// Clamped to what a semaphore can count, which is far above anything
    /// this function has the memory to hold anyway.
    pub fn of(bytes: u64) -> Self {
        let total = bytes.min(u32::MAX as u64) as u32;
        Self {
            bytes: Semaphore::new(total as usize),
            total,
        }
    }

    /// Room for `bytes`, once there is room for it.
    ///
    /// The reservation lasts as long as the value returned: a caller holds it
    /// for as long as it holds the bytes, and dropping it is what lets the
    /// next download begin.
    ///
    /// A request for more than the whole budget is narrowed to the whole
    /// budget rather than waited on, because a semaphore that will never have
    /// that many permits never answers. It is not a case production reaches —
    /// the budget is a multiple of the size cap — but it is one a smaller
    /// budget in a test could, and a guard whose failure mode is a hung
    /// request is worse than the memory it was guarding.
    pub async fn reserve(&self, bytes: u64) -> Reservation<'_> {
        let want = bytes.min(self.total as u64) as u32;

        // The only way this fails is a closed semaphore, and nothing closes
        // this one. Granting the reservation anyway is the failure worth
        // having if that ever stops being true: an unguarded download is a
        // slower way to run out of memory, and a refused one would be a tool
        // call that failed for a reason no model can act on.
        Reservation {
            _permit: self.bytes.acquire_many(want).await.ok(),
        }
    }
}

/// Room held while bytes are arriving, released when it is dropped.
///
/// Nothing reads it. It is the drop that matters, which is why a caller binds
/// it rather than ignoring it — `let _ = in_flight.reserve(..)` drops it
/// where it stands and reserves nothing at all.
#[derive(Debug)]
#[must_use = "the budget is held for as long as this is, so dropping it here reserves nothing"]
pub struct Reservation<'a> {
    _permit: Option<SemaphorePermit<'a>>,
}
