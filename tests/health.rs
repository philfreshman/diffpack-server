use diffpack_server::health;

#[test]
fn health_reports_the_service_as_ok() {
    let body = health::body();

    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "diffpack-server");
}

#[test]
fn health_reports_the_running_crate_version() {
    assert_eq!(health::body()["version"], env!("CARGO_PKG_VERSION"));
}
