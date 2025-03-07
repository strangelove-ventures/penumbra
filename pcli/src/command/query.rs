use anyhow::Result;

mod shielded_pool;
use shielded_pool::ShieldedPool;
mod tx;
use tx::Tx;
mod chain;
use chain::ChainCmd;
mod dex;
use dex::DexCmd;
mod governance;
use governance::GovernanceCmd;
mod validator;
pub(super) use validator::ValidatorCmd;

use crate::App;

#[derive(Debug, clap::Subcommand)]
pub enum QueryCmd {
    /// Queries an arbitrary key.
    Key {
        /// The key to query.
        key: String,
    },
    /// Queries shielded pool data.
    #[clap(subcommand)]
    ShieldedPool(ShieldedPool),
    /// Queries a transaction by hash.
    Tx(Tx),
    /// Queries information about the chain.
    #[clap(subcommand)]
    Chain(ChainCmd),
    /// Queries information about validators.
    #[clap(subcommand)]
    Validator(ValidatorCmd),
    /// Queries information about governance proposals.
    #[clap(subcommand)]
    Governance(GovernanceCmd),
    /// Queries information about the decentralized exchange.
    #[clap(subcommand)]
    Dex(DexCmd),
}

impl QueryCmd {
    pub async fn exec(&self, app: &mut App) -> Result<()> {
        // Special-case: this is a Tendermint query
        if let QueryCmd::Tx(tx) = self {
            return tx.exec(app).await;
        }

        if let QueryCmd::Chain(chain) = self {
            return chain.exec(app).await;
        }

        if let QueryCmd::Validator(validator) = self {
            return validator.exec(app).await;
        }

        if let QueryCmd::Dex(dex) = self {
            return dex.exec(app).await;
        }

        if let QueryCmd::Governance(governance) = self {
            return governance.exec(app).await;
        }

        let key = match self {
            QueryCmd::Tx(_)
            | QueryCmd::Chain(_)
            | QueryCmd::Validator(_)
            | QueryCmd::Dex(_)
            | QueryCmd::Governance(_) => {
                unreachable!("query handled in guard");
            }
            QueryCmd::ShieldedPool(p) => p.key().as_bytes().to_vec(),
            QueryCmd::Key { key } => key.as_bytes().to_vec(),
        };

        let mut client = app.specific_client().await?;
        let req = penumbra_proto::client::v1alpha1::KeyValueRequest {
            key,
            ..Default::default()
        };

        tracing::debug!(?req);

        let rsp = client.key_value(req).await?.into_inner();

        self.display_value(&rsp.value)?;
        Ok(())
    }

    fn display_value(&self, bytes: &[u8]) -> Result<()> {
        match self {
            QueryCmd::Key { .. } => {
                println!("{}", hex::encode(bytes));
            }
            QueryCmd::ShieldedPool(sp) => sp.display_value(bytes)?,
            QueryCmd::Tx { .. }
            | QueryCmd::Chain { .. }
            | QueryCmd::Validator { .. }
            | QueryCmd::Dex { .. }
            | QueryCmd::Governance { .. } => {
                unreachable!("query is special cased")
            }
        }

        Ok(())
    }
}
