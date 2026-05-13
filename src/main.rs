use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "coati", version, about, long_about = None)]
struct Cli {}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let _cli = Cli::parse();
}
