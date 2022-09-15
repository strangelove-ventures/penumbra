use anyhow::Result;
use comfy_table::{presets, Table};
use penumbra_crypto::FullViewingKey;
use penumbra_view::ViewClient;


/// Queries the chain for a transaction by hash.
#[derive(Debug, clap::Args)]
pub struct TxCmd {
    /// The hex-formatted transaction hash to query.
    hash: String,
}

impl TxCmd {
    pub fn needs_sync(&self) -> bool {
        true
    }
    pub async fn exec<V: ViewClient>(&self, fvk: &FullViewingKey, view: &mut V) -> Result<()> {
        
        // Initialize the table
        let mut table = Table::new();
        table.load_preset(presets::NOTHING);
        table.set_header(vec!["Action Type", "Net Change"]);

        // Retrieve TransactionView

        // Iterate over the actions in the transaction & display as appropriate

        // Print table of actions and their change to the balance
        println!("{}", table);

        // Print total change for entire tx

        Ok(())
    }
}
