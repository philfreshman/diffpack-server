use tower::Layer;
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

#[tokio::main]
async fn main() -> Result<(), Error> {
    run(VercelLayer.layer(diffpack_server::router::router())).await
}
