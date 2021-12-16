use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    pin::Pin,
    str::FromStr,
    sync::Arc,
};

use anyhow::{Context, Result};
use futures::stream::{self, Stream, StreamExt};
use penumbra_crypto::{
    merkle::{self, NoteCommitmentTree, TreeExt},
    Address, Nullifier,
};
use penumbra_proto::{
    light_wallet::{CompactBlock, StateFragment},
    thin_wallet::{Asset, TransactionDetail},
};
use penumbra_stake::{FundingStream, Validator};
use sqlx::{postgres::PgPoolOptions, query, query_as, Pool, Postgres};
use tendermint::block;
use tracing::instrument;

use crate::{
    db::{self, schema},
    genesis, PendingBlock,
};

#[derive(Debug, Clone)]
pub struct State {
    pool: Pool<Postgres>,
}

impl State {
    /// Connect to the database with the given `uri`.
    #[instrument]
    pub async fn connect(uri: &str) -> Result<Self> {
        tracing::info!("connecting to postgres");
        let pool = PgPoolOptions::new().max_connections(4).connect(uri).await?;
        tracing::info!("building tables");
        db::init_tables(&pool).await?;
        Ok(State { pool })
    }

    pub async fn commit_block(&self, block: PendingBlock) -> Result<()> {
        let mut dbtx = self.pool.begin().await?;

        let nct_anchor = block.note_commitment_tree.root2();
        // TODO: work out what other stuff to put in apphashes
        let app_hash = nct_anchor.to_bytes();
        let height = block.height.expect("height must be set");

        let nct_bytes = bincode::serialize(&block.note_commitment_tree)?;

        query!(
            r#"
INSERT INTO blobs (id, data) VALUES ('nct', $1)
ON CONFLICT (id) DO UPDATE SET data = $1
"#,
            &nct_bytes[..]
        )
        .execute(&mut dbtx)
        .await?;

        query!(
            "INSERT INTO blocks (height, nct_anchor, app_hash) VALUES ($1, $2, $3)",
            height,
            &nct_anchor.to_bytes()[..],
            &app_hash[..]
        )
        .execute(&mut dbtx)
        .await?;

        // TODO: this could be batched / use prepared statements
        for (note_commitment, positioned_note) in block.notes.into_iter() {
            query!(
                r#"
                INSERT INTO notes (
                    note_commitment,
                    ephemeral_key,
                    encrypted_note,
                    transaction_id,
                    position,
                    height
                ) VALUES ($1, $2, $3, $4, $5, $6)"#,
                &<[u8; 32]>::from(note_commitment)[..],
                &positioned_note.data.ephemeral_key.0[..],
                &positioned_note.data.encrypted_note[..],
                &positioned_note.data.transaction_id[..],
                positioned_note.position as i64,
                height
            )
            .execute(&mut dbtx)
            .await?;
        }

        for nullifier in block.spent_nullifiers.into_iter() {
            query!(
                "INSERT INTO nullifiers VALUES ($1, $2)",
                &<[u8; 32]>::from(nullifier)[..],
                height
            )
            .execute(&mut dbtx)
            .await?;
        }

        // Save any new assets found in the block to the asset registry.
        for (id, denom) in block.new_assets {
            query!(
                r#" INSERT INTO assets ( asset_id, denom) VALUES ($1, $2)"#,
                &id.to_bytes()[..],
                denom
            )
            .execute(&mut dbtx)
            .await?;
        }

        dbtx.commit().await.map_err(Into::into)
    }

    /// Retrieve a nullifier if it exists.
    pub async fn nullifier(&self, nullifier: Nullifier) -> Result<Option<schema::NullifiersRow>> {
        let mut conn = self.pool.acquire().await?;
        let nullifier_row = query!(
            r#"SELECT height FROM nullifiers WHERE nullifier = $1 LIMIT 1"#,
            &<[u8; 32]>::from(nullifier.clone())[..]
        )
        .fetch_optional(&mut conn)
        .await?
        .map(|row| schema::NullifiersRow {
            nullifier,
            height: row.height,
        });

        Ok(nullifier_row)
    }

    /// Retrieve the current note commitment tree.
    pub async fn note_commitment_tree(&self) -> Result<NoteCommitmentTree> {
        let mut conn = self.pool.acquire().await?;
        let note_commitment_tree = if let Some(schema::BlobsRow { data, .. }) = query_as!(
            schema::BlobsRow,
            "SELECT id, data FROM blobs WHERE id = 'nct';"
        )
        .fetch_optional(&mut conn)
        .await?
        {
            bincode::deserialize(&data).context("Could not parse saved note commitment tree")?
        } else {
            NoteCommitmentTree::new(0)
        };

        Ok(note_commitment_tree)
    }

    /// Retrieve the node genesis configuration.
    pub async fn genesis_configuration(&self) -> Result<genesis::AppState> {
        let mut conn = self.pool.acquire().await?;
        let genesis_config = if let Some(schema::BlobsRow { data, .. }) = query_as!(
            schema::BlobsRow,
            "SELECT id, data FROM blobs WHERE id = 'gc';"
        )
        .fetch_optional(&mut conn)
        .await?
        {
            serde_json::from_slice(&data).context("Could not parse saved genesis config")?
        } else {
            // This is only reached on the initial startup.
            // The default value here will be overridden by `InitChain`.
            Default::default()
        };

        Ok(genesis_config)
    }

    pub async fn set_genesis_configuration(
        &self,
        genesis_config: &genesis::AppState,
    ) -> Result<()> {
        let mut dbtx = self.pool.begin().await?;

        let gc_bytes = serde_json::to_vec(&genesis_config)?;

        // ON CONFLICT is excluded here so that an error is raised
        // if genesis config is attempted to be set more than once
        query!(
            r#"
INSERT INTO blobs (id, data) VALUES ('gc', $1)
"#,
            &gc_bytes[..]
        )
        .execute(&mut dbtx)
        .await?;

        dbtx.commit().await.map_err(Into::into)
    }

    /// Retrieve the latest block info, if any.
    pub async fn latest_block_info(&self) -> Result<Option<schema::BlocksRow>> {
        let mut conn = self.pool.acquire().await?;
        let latest = query_as!(
            schema::BlocksRow,
            r#"SELECT height, nct_anchor AS "nct_anchor: merkle::Root", app_hash FROM blocks ORDER BY height DESC LIMIT 1"#
        )
        .fetch_optional(&mut conn)
        .await?;

        Ok(latest)
    }

    // retrieve the `last` latest node commitment tree anchors from the database
    pub async fn recent_anchors(&self, last: usize) -> Result<VecDeque<merkle::Root>> {
        let mut conn = self.pool.acquire().await?;
        let anchor_rows = query!(
            r#"SELECT nct_anchor AS "nct_anchor: merkle::Root" FROM blocks ORDER BY height DESC LIMIT $1"#,
            last as i64,
        )
        .fetch_all(&mut conn)
        .await?;

        let mut nct_vec: VecDeque<merkle::Root> = VecDeque::new();
        for block in anchor_rows {
            nct_vec.push_back(block.nct_anchor)
        }

        Ok(nct_vec)
    }

    /// Retrieve the latest block height.
    pub async fn height(&self) -> Result<block::Height> {
        Ok(self
            .latest_block_info()
            .await?
            .map(|row| row.height)
            .unwrap_or(0)
            .try_into()
            .unwrap())
    }

    /// Retrieve the latest apphash.
    pub async fn app_hash(&self) -> Result<Vec<u8>> {
        Ok(self
            .latest_block_info()
            .await?
            .map(|row| row.app_hash)
            .unwrap_or_else(|| vec![0; 32]))
    }

    /// Retrieve the [`CompactBlock`] for the given height.
    ///
    /// If the block does not exist, the resulting `CompactBlock` will be empty.
    pub async fn compact_blocks<'a>(
        &self,
        start_height: i64,
        end_height: i64,
        conn_nullifiers: &'a mut sqlx::pool::PoolConnection<Postgres>,
        conn_fragments: &'a mut sqlx::pool::PoolConnection<Postgres>,
    ) -> Result<impl Stream<Item = CompactBlock> + 'a> {
        // let mut conn_nullifiers = self.pool.acquire().await?;
        // let mut conn_fragments = self.pool.acquire().await?;

        let nullifiers = query!(
            "SELECT height, nullifier 
                FROM nullifiers 
                WHERE height BETWEEN $1 AND $2
                ORDER BY height ASC",
            start_height,
            end_height
        )
        .fetch(conn_nullifiers)
        .peekable();

        let fragments = query!(
            "SELECT height, note_commitment, ephemeral_key, encrypted_note
                 FROM notes
                 WHERE height BETWEEN $1 AND $2
                 ORDER BY position ASC",
            start_height,
            end_height
        )
        .fetch(conn_fragments)
        .peekable();

        let nullifiers = Arc::new(RefCell::new(nullifiers));
        let fragments = Arc::new(RefCell::new(fragments));

        Ok(stream::iter(start_height..=end_height).then(move |height| {
            let nullifiers: Arc<RefCell<_>> = nullifiers.clone();
            let fragments: Arc<RefCell<_>> = fragments.clone();

            let mut block = CompactBlock {
                height: height as u32,
                nullifiers: vec![],
                fragments: vec![],
            };

            async move {
                let mut nullifiers = nullifiers.borrow_mut();
                let mut fragments = fragments.borrow_mut();

                while let Some(Ok(row)) = Pin::new(&mut *nullifiers).peek().await {
                    if row.height == height {
                        let row = Pin::new(&mut *nullifiers)
                            .next()
                            .await
                            .expect("we already peeked, so there is a next row")
                            .expect("we already peeked and confirmed there is no error");
                        block.nullifiers.push(row.nullifier.into());
                    } else {
                        break;
                    }
                }

                while let Some(Ok(row)) = Pin::new(&mut *fragments).peek().await {
                    if row.height == Some(height) {
                        let row = Pin::new(&mut *fragments)
                            .next()
                            .await
                            .expect("we already peeked, so there is a next row")
                            .expect("we already peeked and confirmed there is no error");
                        block.fragments.push(StateFragment {
                            note_commitment: row.note_commitment.into(),
                            ephemeral_key: row.ephemeral_key.into(),
                            encrypted_note: row.encrypted_note.into(),
                        });
                    } else {
                        break;
                    }
                }

                block
            }
        }))
    }

    /// Retreive the current validator set.
    ///
    pub async fn validators(&self) -> Result<BTreeMap<tendermint::PublicKey, Validator>> {
        let mut conn = self.pool.acquire().await?;

        let mut validators: BTreeMap<tendermint::PublicKey, Validator> = BTreeMap::new();

        let stored_validators = query!(r#"SELECT tm_pubkey, voting_power FROM validators"#)
            .fetch_all(&mut conn)
            .await?;
        for row in stored_validators.iter() {
            // NOTE: we store the validator's public key in the database as a json-encoded string,
            // because Tendermint pubkeys can be either ed25519 or secp256k1, and we want a
            // non-ambiguous encoding for the public key.
            let decoded_pubkey: tendermint::PublicKey = serde_json::from_slice(&row.tm_pubkey)?;

            let mut funding_streams: Vec<FundingStream> = Vec::new();
            let stored_funding_streams = query!(r#"SELECT tm_pubkey, address, rate_bps FROM validator_fundingstreams WHERE tm_pubkey = $1"#, row.tm_pubkey).fetch_all(&mut conn).await?;
            for f_row in stored_funding_streams {
                funding_streams.push(FundingStream {
                    address: Address::from_str(&f_row.address)?,
                    rate_bps: f_row.rate_bps.try_into()?,
                })
            }
            // NOTE: voting_power is stored in the psql database as a `bigint`, which maps to an
            // `i64` in sqlx. try_into uses the `TryFrom<i64>` implementation for voting power from
            // Tendermint, so will return an error if voting power is negative (and not silently
            // overflow).
            validators.insert(
                decoded_pubkey,
                Validator::new(
                    decoded_pubkey,
                    row.voting_power.try_into()?,
                    funding_streams,
                ),
            );
        }

        Ok(validators)
    }

    /// set the initial validator set, inserting each validator in `validators` into the state.
    pub async fn set_initial_validators(
        &self,
        validators: &BTreeMap<tendermint::PublicKey, Validator>,
    ) -> Result<()> {
        let mut conn = self.pool.begin().await?;

        // TODO: batching optimization
        for (tm_pubkey, val) in validators.iter() {
            let pubkey_str = serde_json::to_string(tm_pubkey)?;

            query!(
                "INSERT INTO validators (tm_pubkey, voting_power) VALUES ($1, $2)",
                pubkey_str.as_bytes(),
                i64::try_from(val.voting_power)?,
            )
            .execute(&mut conn)
            .await?;

            // TODO (optimization): batch insert?
            for stream in val.funding_streams.iter() {
                query!(
                "INSERT INTO validator_fundingstreams (tm_pubkey, address, rate_bps) VALUES ($1, $2, $3)",
                pubkey_str.as_bytes(),
                stream.address.to_string(),
                i64::try_from(stream.rate_bps)?,
                )
                .execute(&mut conn)
                .await?;
            }
        }

        conn.commit().await.map_err(Into::into)
    }

    /// Retrieve the [`TransactionDetail`] for a given note commitment.
    pub async fn transaction_by_note(&self, note_commitment: Vec<u8>) -> Result<TransactionDetail> {
        let mut conn = self.pool.acquire().await?;

        let row = query!(
            "SELECT transaction_id FROM notes WHERE note_commitment = $1",
            note_commitment
        )
        .fetch_one(&mut conn)
        .await?;
        Ok(TransactionDetail {
            id: row.transaction_id,
        })
    }

    /// Retrieve the [`Asset`] for a given asset ID.
    pub async fn asset_lookup(&self, asset_id: Vec<u8>) -> Result<Asset> {
        let mut conn = self.pool.acquire().await?;

        let asset = query!(
            "SELECT denom, asset_id FROM assets WHERE asset_id = $1",
            asset_id
        )
        .fetch_one(&mut conn)
        .await?;
        Ok(Asset {
            asset_denom: asset.denom,
            asset_id: asset.asset_id,
        })
    }

    /// Retrieves the entire Asset Registry.
    pub async fn asset_list(&self) -> Result<Vec<Asset>> {
        let mut conn = self.pool.acquire().await?;

        Ok(query!("SELECT denom, asset_id FROM assets")
            .fetch_all(&mut conn)
            .await?
            .into_iter()
            .map(|row| Asset {
                asset_denom: row.denom,
                asset_id: row.asset_id,
            })
            .collect())
    }
}
