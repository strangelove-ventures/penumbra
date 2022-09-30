// Many of the IBC message types are enums, where the number of variants differs
// depending on the build configuration, meaning that the fallthrough case gets
// marked as unreachable only when not building in test configuration.
#![allow(unreachable_patterns)]

mod channel;
mod client;
mod connection;
pub(crate) mod state_key;

use crate::ibc::ibc_handler::AppRouter;
use crate::ibc::transfer::ICS20Transfer;
use crate::{Component, Context};
use anyhow::Result;
use async_trait::async_trait;
use client::Ics2Client;
use ibc::core::ics24_host::identifier::PortId;
use penumbra_chain::{genesis, View as _};
use penumbra_proto::core::ibc::v1alpha1 as ibc_pb;
use penumbra_storage::State;
use penumbra_transaction::{Action, Transaction};
use tendermint::abci;
use tracing::instrument;

pub struct IBCComponent {
    client: client::Ics2Client,
    connection: connection::ConnectionComponent,
    channel: channel::ICS4Channel,

    state: State,
}

impl IBCComponent {
    #[instrument(name = "ibc", skip(state))]
    pub async fn new(state: State) -> Self {
        let client = Ics2Client::new(state.clone()).await;
        let connection = connection::ConnectionComponent::new(state.clone()).await;

        let mut router = AppRouter::new();
        let transfer = ICS20Transfer::new(state.clone());
        router.bind(PortId::transfer(), Box::new(transfer));

        let channel = channel::ICS4Channel::new(state.clone(), Box::new(router)).await;

        Self {
            channel,
            client,
            connection,

            state: state.clone(),
        }
    }

    #[instrument(name = "ibc", skip(self, ctx))]
    pub async fn stateful_ics20_withdrawal_check(
        &self,
        ctx: Context,
        withdrawal: Action::ICS20Withdrawal,
    ) -> Result<()> {
        // check that withdrawal timeout timestamp and height are not in the past
        let block_time = self.get_block_timestamp().await?;
        let block_height = self.get_block_height().await?;

        if block_time > withdrawal.timeout_timestamp {
            return Err(anyhow::anyhow!(
                "withdrawal timeout timestamp is in the past"
            ));
        }
        if block_height > withdrawal.timeout_height {
            return Err(anyhow::anyhow!("withdrawal timeout height is in the past"));
        }

        // NOTE: the `value` is verified to originate from a well-formed spend in the balance
        // commitment.
        //

        Ok(())
    }
}

#[async_trait]
impl Component for IBCComponent {
    #[instrument(name = "ibc", skip(self, app_state))]
    async fn init_chain(&mut self, app_state: &genesis::AppState) {
        self.client.init_chain(app_state).await;
        self.connection.init_chain(app_state).await;
        self.channel.init_chain(app_state).await;
    }

    #[instrument(name = "ibc", skip(self, begin_block, ctx))]
    async fn begin_block(&mut self, ctx: Context, begin_block: &abci::request::BeginBlock) {
        self.client.begin_block(ctx.clone(), begin_block).await;
        self.connection.begin_block(ctx.clone(), begin_block).await;
        self.channel.begin_block(ctx.clone(), begin_block).await;
    }

    #[instrument(name = "ibc", skip(tx, ctx))]
    fn check_tx_stateless(ctx: Context, tx: &Transaction) -> Result<()> {
        for action in tx.transaction_body.actions.iter() {
            match action {
                Action::ICS20Withdrawal(withdrawal) => {
                    // check that the destination chain address and chain ID are well-formed
                    withdrawal.validate()?;
                }
                _ => {}
            }
        }

        client::Ics2Client::check_tx_stateless(ctx.clone(), tx)?;
        connection::ConnectionComponent::check_tx_stateless(ctx.clone(), tx)?;
        channel::ICS4Channel::check_tx_stateless(ctx, tx)?;

        Ok(())
    }

    #[instrument(name = "ibc", skip(self, ctx, tx))]
    async fn check_tx_stateful(&self, ctx: Context, tx: &Transaction) -> Result<()> {
        if tx.ibc_actions().count() > 0 && !self.state.get_chain_params().await?.ibc_enabled {
            return Err(anyhow::anyhow!(
                "transaction contains IBC actions, but IBC is not enabled"
            ));
        }

        for action in tx.transaction_body.actions.iter() {
            match action {
                Action::ICS20Withdrawal(withdrawal) => {
                    self.stateful_ics20_withdrawal_check(ctx.clone(), withdrawal)
                        .await?;
                }
                _ => {}
            }
        }

        self.client.check_tx_stateful(ctx.clone(), tx).await?;
        self.connection.check_tx_stateful(ctx.clone(), tx).await?;
        self.channel.check_tx_stateful(ctx.clone(), tx).await?;

        Ok(())
    }

    #[instrument(name = "ibc", skip(self, ctx, tx))]
    async fn execute_tx(&mut self, ctx: Context, tx: &Transaction) {
        for action in tx.transaction_body.actions.iter() {
            match action {
                Action::ICS20Withdrawal(withdrawal) => {
                    // Create an appropriate FungibleTokenPacketData from the action contents;
                    let packet_data: ibc_pb::FungibleTokenPacketData = withdrawal.clone().into();

                    // Commit the FTPD to penumbra using the correct namespace

                    // Record the FTPD as not acknowledged yet, to allow us to handle timeouts
                }
                _ => {}
            }
        }
        self.client.execute_tx(ctx.clone(), tx).await;
        self.connection.execute_tx(ctx.clone(), tx).await;
        self.channel.execute_tx(ctx.clone(), tx).await;
    }

    #[instrument(name = "ibc", skip(self, ctx, end_block))]
    async fn end_block(&mut self, ctx: Context, end_block: &abci::request::EndBlock) {
        self.client.end_block(ctx.clone(), end_block).await;
        self.connection.end_block(ctx.clone(), end_block).await;
        self.channel.end_block(ctx.clone(), end_block).await;
    }
}
