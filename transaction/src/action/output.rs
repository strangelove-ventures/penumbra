use std::convert::{TryFrom, TryInto};

use anyhow::Error;
use bytes::Bytes;
use penumbra_crypto::{
    balance,
    proofs::transparent::OutputProof,
    symmetric::{OvkWrappedKey, WrappedMemoKey},
    Note, NotePayload,
};
use penumbra_proto::{core::transaction::v1alpha1 as pb, Protobuf};

use crate::{view::action_view::OutputView, ActionView, TransactionPerspective};

use super::IsAction;

#[derive(Clone, Debug)]
pub struct Output {
    pub body: Body,
    pub proof: OutputProof,
}

impl IsAction for Output {
    fn balance_commitment(&self) -> balance::Commitment {
        self.body.balance_commitment
    }

    fn view_from_perspective(&self, txp: &TransactionPerspective) -> anyhow::Result<ActionView> {
        // Get payload key for note commitment of note payload

        let note_commitment = self.body.note_payload.note_commitment;

        // Get payload key for note commitment of swap NFT.
        let payload_key = txp
            .payload_keys
            .get(&note_commitment)
            .ok_or_else(|| anyhow::anyhow!("corresponding payload key not found"))?;

        // Decrypt note

        let decrypted_note =
            Note::decrypt_with_payload_key(&self.body.note_payload.encrypted_note, payload_key)?;
        // If memo has not been decrypted yet
        // * Decrypt wrapped_memo_key

        let decrypted_memo_key = self.body.wrapped_memo_key.decrypt_outgoing(payload_key)?;

        Ok(ActionView::Output(OutputView {
            decrypted_note,
            decrypted_memo_key,
        }))
    }
}

#[derive(Clone, Debug)]
pub struct Body {
    pub note_payload: NotePayload,
    pub balance_commitment: balance::Commitment,
    pub ovk_wrapped_key: OvkWrappedKey,
    pub wrapped_memo_key: WrappedMemoKey,
}

impl Protobuf<pb::Output> for Output {}

impl From<Output> for pb::Output {
    fn from(output: Output) -> Self {
        let proof: Vec<u8> = output.proof.into();
        pb::Output {
            body: Some(output.body.into()),
            proof: proof.into(),
        }
    }
}

impl TryFrom<pb::Output> for Output {
    type Error = Error;

    fn try_from(proto: pb::Output) -> anyhow::Result<Self, Self::Error> {
        Ok(Output {
            body: proto
                .body
                .ok_or_else(|| anyhow::anyhow!("missing output body"))?
                .try_into()?,
            proof: proto.proof[..]
                .try_into()
                .map_err(|_| anyhow::anyhow!("output body malformed"))?,
        })
    }
}

impl Protobuf<pb::OutputBody> for Body {}

impl From<Body> for pb::OutputBody {
    fn from(output: Body) -> Self {
        pb::OutputBody {
            note_payload: Some(output.note_payload.into()),
            balance_commitment: Some(output.balance_commitment.into()),
            wrapped_memo_key: Bytes::copy_from_slice(&output.wrapped_memo_key.0),
            ovk_wrapped_key: Bytes::copy_from_slice(&output.ovk_wrapped_key.0),
        }
    }
}

impl TryFrom<pb::OutputBody> for Body {
    type Error = Error;

    fn try_from(proto: pb::OutputBody) -> anyhow::Result<Self, Self::Error> {
        let note_payload = proto
            .note_payload
            .ok_or_else(|| anyhow::anyhow!("missing note payload"))?
            .try_into()
            .map_err(|e: Error| e.context("output body malformed"))?;

        let wrapped_memo_key = proto.wrapped_memo_key[..]
            .try_into()
            .map_err(|_| anyhow::anyhow!("output malformed"))?;

        let ovk_wrapped_key: OvkWrappedKey = proto.ovk_wrapped_key[..]
            .try_into()
            .map_err(|_| anyhow::anyhow!("output malformed"))?;

        let balance_commitment = proto
            .balance_commitment
            .ok_or_else(|| anyhow::anyhow!("missing value commitment"))?
            .try_into()?;

        Ok(Body {
            note_payload,
            wrapped_memo_key,
            ovk_wrapped_key,
            balance_commitment,
        })
    }
}
