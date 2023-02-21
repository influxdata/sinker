#![deny(rustdoc::broken_intra_doc_links, rustdoc::bare_urls, rust_2018_idioms)]

use anyhow::Result;
use clap::Parser;

#[derive(Clone, Parser)]
#[clap(version)]
struct Args {
    /// The tracing filter used for logs
    #[clap(long, env = "SINKER_LOG", default_value = "watch_pods=info,warn")]
    log_level: kubert::LogFilter,

    /// The logging format
    #[clap(long, default_value = "plain")]
    log_format: kubert::LogFormat,

    #[clap(flatten)]
    client: kubert::ClientArgs,

    #[clap(flatten)]
    admin: kubert::AdminArgs,
}

#[tokio::main]
async fn main() -> Result<()> {
    let Args {
        log_level,
        log_format,
        client,
        admin,
    } = Args::parse();

    let _rt = kubert::Runtime::builder()
        .with_log(log_level, log_format)
        .with_admin(admin)
        .with_client(client)
        .build()
        .await?;
    tracing::debug!("wow");
    println!("Hello, world!");

    Ok(())
}
