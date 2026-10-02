mod disk;
mod server;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let option = |name: &str, default: &str| {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map(|pair| pair[1].clone())
            .unwrap_or_else(|| default.to_string())
    };
    server::serve(server::ServerConfig {
        data_dir: option("--data-dir", "data").into(),
        listen: option("--listen", "127.0.0.1:3187").parse()?,
        title: "kconfigwtf".to_string(),
    })
    .await
}
