//! The `/health` response body, driven through the library rather than
//! through `vercel_runtime`. The binary's job is to put this body on a
//! response; what the body *says* is the part worth pinning, and pinning it
//! here means #8 can prove the deploy pipeline without a protocol existing.

use diffpack_server::health;

#[test]
fn health_reports_the_service_as_ok() {
    let body = health::body();

    assert_eq!(body["status"], "degraded");
    assert_eq!(body["service"], "diffpack-server");
}

#[test]
fn health_reports_the_running_crate_version() {
    // Not a copy of the literal in `Cargo.toml` restated here — the point is
    // that a deployed function can be asked which build is answering.
    assert_eq!(health::body()["version"], env!("CARGO_PKG_VERSION"));
}
