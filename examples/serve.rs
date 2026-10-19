use diffpack_server::mcp::Diffpack;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use tokio::net::TcpListener;

const ANY_FREE_LOOPBACK_PORT: &str = "127.0.0.1:0";

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let ctx = Ctx::fixture(fixtures_dir()?);
    let app = router::router_with(
        move || Ok(Diffpack::with_ctx(ctx.clone())),
        router::allowed_origins_from_env(),
    );

    let listener = TcpListener::bind(ANY_FREE_LOOPBACK_PORT).await?;
    println!("http://{}", listener.local_addr()?);
    axum::serve(listener, app).await
}

fn fixtures_dir() -> std::io::Result<String> {
    std::env::args().nth(1).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "usage: serve <fixtures-dir>",
        )
    })
}
