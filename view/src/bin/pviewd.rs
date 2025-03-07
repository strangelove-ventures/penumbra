#![recursion_limit = "256"]
#![allow(clippy::clone_on_copy)]
use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use penumbra_crypto::FullViewingKey;
use penumbra_proto::client::v1alpha1::oblivious_query_client::ObliviousQueryClient;
use penumbra_proto::client::v1alpha1::ChainParamsRequest;
use penumbra_proto::view::v1alpha1::view_protocol_server::ViewProtocolServer;
use penumbra_view::ViewService;
use std::env;
use std::str::FromStr;
use tonic::transport::Server;

#[derive(Debug, Parser)]
#[clap(
    name = "pviewd",
    about = "The Penumbra view daemon.",
    version = env!("VERGEN_GIT_SEMVER"),
)]
struct Opt {
    /// Command to run.
    #[clap(subcommand)]
    cmd: Command,
    /// The path used to store the state database.
    #[clap(short, long, default_value = "pviewd-db.sqlite")]
    sqlite_path: Utf8PathBuf,
    /// The address of the pd+tendermint node.
    #[clap(short, long, default_value = "testnet.penumbra.zone")]
    node: String,
    /// The port to use to speak to tendermint's RPC server.
    #[clap(long, default_value = "26657")]
    tendermint_port: u16,
    /// The port to use to speak to pd's gRPC server.
    #[clap(long, default_value = "8080")]
    pd_port: u16,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize the view service with a full viewing key.
    Init {
        /// The full viewing key to initialize the view service with.
        full_viewing_key: String,
    },
    /// Start the view service.
    Start {
        /// Bind the view service to this host.
        #[clap(long, default_value = "127.0.0.1")]
        host: String,
        /// Bind the view gRPC server to this port.
        #[clap(long, default_value = "8081")]
        view_port: u16,
    },
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let opt = Opt::parse();

    match opt.cmd {
        Command::Init { full_viewing_key } => {
            let mut client =
                ObliviousQueryClient::connect(format!("http://{}:{}", opt.node, opt.pd_port))
                    .await?;

            let params = client
                .chain_parameters(tonic::Request::new(ChainParamsRequest {
                    chain_id: String::new(),
                }))
                .await?
                .into_inner()
                .try_into()?;

            penumbra_view::Storage::initialize(
                opt.sqlite_path.as_path(),
                FullViewingKey::from_str(full_viewing_key.as_ref())
                    .context("The provided string is not a valid FullViewingKey")?,
                params,
            )
            .await?;
            Ok(())
        }
        Command::Start { host, view_port } => {
            tracing::info!(?opt.sqlite_path, ?host, ?view_port, ?opt.node, ?opt.tendermint_port, ?opt.pd_port, "starting pviewd");

            let storage = penumbra_view::Storage::load(opt.sqlite_path).await?;

            let service =
                ViewService::new(storage, opt.node, opt.pd_port, opt.tendermint_port).await?;

            tokio::spawn(
                Server::builder()
                    .add_service(ViewProtocolServer::new(service))
                    .serve(
                        format!("{}:{}", host, view_port)
                            .parse()
                            .expect("this is a valid address"),
                    ),
            )
            .await??;

            Ok(())
        }
    }
}
