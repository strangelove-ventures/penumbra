use std::convert::{TryFrom, TryInto};

use anyhow::Error;
use bytes::Bytes;
use penumbra_crypto::{
    balance,
    proofs::transparent::SpendProof,
    rdsa::{Signature, SpendAuth, VerificationKey},
    Nullifier,
};
use penumbra_proto::{core::transaction::v1alpha1 as transaction, Protobuf};

use crate::{view::action_view::SpendView, ActionView, TransactionPerspective};

use super::IsAction;

#[derive(Clone, Debug)]
pub struct Spend {
    pub body: Body,
    pub auth_sig: Signature<SpendAuth>,
    pub proof: SpendProof,
}

impl IsAction for Spend {
    fn balance_commitment(&self) -> balance::Commitment {
        self.body.balance_commitment
    }

    fn view_from_perspective(&self, txp: &TransactionPerspective) -> anyhow::Result<ActionView> {
        let decrypted_note = txp
            .spend_nullifiers
            .get(&self.body.nullifier)
            .ok_or_else(|| anyhow::anyhow!("no note found for nullifier"))?
            .to_owned();
        Ok(ActionView::Spend(SpendView { decrypted_note }))
    }
}

impl Protobuf<transaction::Spend> for Spend {}

impl From<Spend> for transaction::Spend {
    fn from(msg: Spend) -> Self {
        let proof: Vec<u8> = msg.proof.into();
        transaction::Spend {
            body: Some(msg.body.into()),
            auth_sig: Some(msg.auth_sig.into()),
            proof: proof.into(),
        }
    }
}

impl TryFrom<transaction::Spend> for Spend {
    type Error = Error;

    fn try_from(proto: transaction::Spend) -> anyhow::Result<Self, Self::Error> {
        let body = proto
            .body
            .ok_or_else(|| anyhow::anyhow!("spend body malformed"))?
            .try_into()?;
        let auth_sig = proto
            .auth_sig
            .ok_or_else(|| anyhow::anyhow!("spend body malformed"))?
            .try_into()?;

        let proof = (proto.proof[..])
            .try_into()
            .map_err(|_| anyhow::anyhow!("spend body malformed"))?;

        Ok(Spend {
            body,
            auth_sig,
            proof,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Body {
    pub balance_commitment: balance::Commitment,
    pub nullifier: Nullifier,
    pub rk: VerificationKey<SpendAuth>,
}

impl Protobuf<transaction::SpendBody> for Body {}

impl From<Body> for transaction::SpendBody {
    fn from(msg: Body) -> Self {
        let nullifier_bytes: [u8; 32] = msg.nullifier.into();
        let rk_bytes: [u8; 32] = msg.rk.into();
        transaction::SpendBody {
            balance_commitment: Some(msg.balance_commitment.into()),
            nullifier: Bytes::copy_from_slice(&nullifier_bytes),
            rk: Bytes::copy_from_slice(&rk_bytes),
        }
    }
}

impl TryFrom<transaction::SpendBody> for Body {
    type Error = Error;

    fn try_from(proto: transaction::SpendBody) -> anyhow::Result<Self, Self::Error> {
        let balance_commitment: balance::Commitment = proto
            .balance_commitment
            .ok_or_else(|| anyhow::anyhow!("missing value commitment"))?
            .try_into()?;

        let nullifier = (proto.nullifier[..])
            .try_into()
            .map_err(|_| anyhow::anyhow!("spend body malformed"))?;

        let rk_bytes: [u8; 32] = (proto.rk[..])
            .try_into()
            .map_err(|_| anyhow::anyhow!("spend body malformed"))?;
        let rk = rk_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("spend body malformed"))?;

        Ok(Body {
            balance_commitment,
            nullifier,
            rk,
        })
    }
}
