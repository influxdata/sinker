#![deny(rustdoc::broken_intra_doc_links, rustdoc::bare_urls, rust_2018_idioms)]

use clap::{Parser, Subcommand};
use kube::CustomResourceExt;

use sinker::controller;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    #[derive(Clone, Parser)]
    #[clap(version)]
    struct Args {
        /// The tracing filter used for logs
        #[clap(long, env = "SINKER_LOG", default_value = "sinker=info,warn")]
        log_level: kubert::LogFilter,

        /// The logging format
        #[clap(long, default_value = "plain")]
        log_format: kubert::LogFormat,

        #[clap(flatten)]
        client: kubert::ClientArgs,

        #[clap(flatten)]
        admin: kubert::AdminArgs,

        #[command(subcommand)]
        command: Commands,
    }

    #[derive(Clone, Subcommand)]
    enum Commands {
        /// Run the controller
        Run,

        /// Generates k8s manifests
        Manifests,
    }

    let Args {
        log_level,
        log_format,
        client,
        admin,
        command,
    } = Args::parse();

    match &command {
        Commands::Manifests => {
            println!(
                "{}",
                serde_yaml::to_string(&sinker::resources::ResourceSync::crd()).unwrap()
            );
        }
        Commands::Run => {
            let rt = kubert::Runtime::builder()
                .with_log(log_level, log_format)
                .with_admin(admin)
                .with_client(client)
                .build()
                .await?;

            let controller = controller::run(rt.client());

            // Both runtimes implements graceful shutdown, so poll until both are done
            tokio::join!(controller, rt.run()).1?;
        }
    }

    Ok(())
}
