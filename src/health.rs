use serde_json::{json, Value};

pub fn body() -> Value {
    json!({
        "status": "ok",
        "service": "diffpack-server",
        "version": env!("CARGO_PKG_VERSION"),
    })
}
