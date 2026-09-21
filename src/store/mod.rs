//! Cached diff results.
//!
//! [`DiffStore`] is what the rest of the crate will see of the cache — get
//! the entry for a `DiffKey`, put an entry — and everything underneath it is
//! this module's: the Vercel Blob client, the 256 MB budget and the eviction
//! that keeps it. See [ADR
//! 0003](../docs/adr/0003-the-cache-seam-is-a-store.md).
//!
//! Today this module holds only the client (#20). `DiffStore` arrives with
//! #21 and the budget with #22, which is why nothing here is public yet: the
//! seam is the store, and a client that a tool could name would be the seam
//! moving down to the HTTP verbs that ADR 0003 rejected.

mod blob;
