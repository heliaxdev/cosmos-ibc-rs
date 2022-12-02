use crate::prelude::*;

use bytes::Buf;
use ibc_proto::google::protobuf::Any;
use ibc_proto::ibc::lightclients::tendermint::v1::Misbehaviour as RawMisbehaviour;
use ibc_proto::protobuf::Protobuf;
use prost::Message;
use tendermint::Hash;
use tendermint_light_client_verifier::operations::{
    CommitValidator, Hasher, ProdCommitValidator, ProdHasher,
};
use tendermint_light_client_verifier::types::UntrustedBlockState;
use tendermint_light_client_verifier::Verdict;

use crate::clients::ics07_tendermint::error::{Error, IntoResult};
use crate::clients::ics07_tendermint::header::Header;
use crate::core::ics02_client::error::ClientError;
use crate::core::ics24_host::identifier::{ChainId, ClientId};
use crate::Height;

pub const TENDERMINT_MISBEHAVIOUR_TYPE_URL: &str = "/ibc.lightclients.tendermint.v1.Misbehaviour";

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Misbehaviour {
    client_id: ClientId,
    header1: Header,
    header2: Header,
}

impl Misbehaviour {
    pub fn new(client_id: ClientId, header1: Header, header2: Header) -> Result<Self, Error> {
        if header1.signed_header.header.chain_id != header2.signed_header.header.chain_id {
            return Err(Error::InvalidRawMisbehaviour {
                reason: "headers must have identical chain_ids".to_owned(),
            });
        }

        if header1.height() < header2.height() {
            return Err(Error::InvalidRawMisbehaviour {
                reason: format!(
                    "headers1 height is less than header2 height ({} < {})",
                    header1.height(),
                    header2.height()
                ),
            });
        }

        let untrusted_state_1 = header1.as_untrusted_block_state();
        let untrusted_state_2 = header2.as_untrusted_block_state();

        Self::verify_cur_validator_sets(&untrusted_state_1)?;
        Self::verify_cur_validator_sets(&untrusted_state_2)?;
        Self::verify_next_validator_sets(&untrusted_state_1)?;
        Self::verify_next_validator_sets(&untrusted_state_2)?;

        Self::verify_header_commit(&untrusted_state_1)?;
        Self::verify_header_commit(&untrusted_state_2)?;
        Self::valid_commit(&untrusted_state_1)?;
        Self::valid_commit(&untrusted_state_2)?;

        Ok(Self {
            client_id,
            header1,
            header2,
        })
    }

    pub fn client_id(&self) -> &ClientId {
        &self.client_id
    }

    pub fn header1(&self) -> &Header {
        &self.header1
    }

    pub fn header2(&self) -> &Header {
        &self.header2
    }

    pub fn chain_id_matches(&self, chain_id: &ChainId) -> bool {
        assert_eq!(
            self.header1.signed_header.header.chain_id, self.header2.signed_header.header.chain_id,
            "this is enforced by the ctor"
        );

        self.header1.signed_header.header.chain_id.as_str() == chain_id.as_str()
    }

    fn verify_cur_validator_sets(untrusted_state: &UntrustedBlockState<'_>) -> Result<(), Error> {
        let hasher = ProdHasher {};
        let validators_hash = hasher.hash_validator_set(untrusted_state.validators);
        let header_validators_hash = untrusted_state.signed_header.header.validators_hash;
        Self::verify_validator_sets(validators_hash, header_validators_hash)
    }

    fn verify_next_validator_sets(untrusted_state: &UntrustedBlockState<'_>) -> Result<(), Error> {
        let hasher = ProdHasher {};
        match untrusted_state.next_validators {
            Some(untrusted_next_validators) => {
                let validators_hash = hasher.hash_validator_set(untrusted_next_validators);
                let header_validators_hash =
                    untrusted_state.signed_header.header.next_validators_hash;
                Self::verify_validator_sets(validators_hash, header_validators_hash)
            }
            None => Ok(()),
        }
    }

    fn verify_validator_sets(
        validators_hash: Hash,
        header_validators_hash: Hash,
    ) -> Result<(), Error> {
        if header_validators_hash == validators_hash {
            Ok(())
        } else {
            Err(Error::Validation {
                reason: format!(
                    "invalid validator set: header_validators_hash {}, validators_hash {}",
                    header_validators_hash, validators_hash
                ),
            })
        }
    }

    fn verify_header_commit(untrusted_state: &UntrustedBlockState<'_>) -> Result<(), Error> {
        let hasher = ProdHasher {};
        let header_hash = hasher.hash_header(&untrusted_state.signed_header.header);
        let commit_hash = untrusted_state.signed_header.commit.block_id.hash;
        if header_hash == commit_hash {
            Ok(())
        } else {
            Err(Error::Validation {
                reason: format!(
                    "invalid header commit: header_hash {}, commit_hash {}",
                    header_hash, commit_hash
                ),
            })
        }
    }

    fn valid_commit(untrusted_state: &UntrustedBlockState<'_>) -> Result<(), Error> {
        let hasher = ProdHasher {};
        let commit_validator = ProdCommitValidator::new(hasher);
        let signed_header = untrusted_state.signed_header;
        let validators = untrusted_state.validators;

        let result: Verdict = commit_validator.validate(signed_header, validators).into();
        result.into_result()?;
        let result: Verdict = commit_validator
            .validate_full(signed_header, validators)
            .into();
        result.into_result()?;

        Ok(())
    }
}

impl crate::core::ics02_client::misbehaviour::Misbehaviour for Misbehaviour {
    fn client_id(&self) -> &ClientId {
        &self.client_id
    }

    fn height(&self) -> Height {
        self.header1.height()
    }
}

impl Protobuf<RawMisbehaviour> for Misbehaviour {}

impl TryFrom<RawMisbehaviour> for Misbehaviour {
    type Error = Error;

    fn try_from(raw: RawMisbehaviour) -> Result<Self, Self::Error> {
        let client_id = raw
            .client_id
            .parse()
            .map_err(|_| Error::InvalidRawClientId {
                client_id: raw.client_id.clone(),
            })?;
        let header1: Header = raw
            .header_1
            .ok_or_else(|| Error::InvalidRawMisbehaviour {
                reason: "missing header1".into(),
            })?
            .try_into()?;
        let header2: Header = raw
            .header_2
            .ok_or_else(|| Error::InvalidRawMisbehaviour {
                reason: "missing header2".into(),
            })?
            .try_into()?;

        Self::new(client_id, header1, header2)
    }
}

impl From<Misbehaviour> for RawMisbehaviour {
    fn from(value: Misbehaviour) -> Self {
        RawMisbehaviour {
            client_id: value.client_id.to_string(),
            header_1: Some(value.header1.into()),
            header_2: Some(value.header2.into()),
        }
    }
}

impl Protobuf<Any> for Misbehaviour {}

impl TryFrom<Any> for Misbehaviour {
    type Error = ClientError;

    fn try_from(raw: Any) -> Result<Self, ClientError> {
        use core::ops::Deref;

        fn decode_misbehaviour<B: Buf>(buf: B) -> Result<Misbehaviour, Error> {
            RawMisbehaviour::decode(buf)
                .map_err(Error::Decode)?
                .try_into()
        }

        match raw.type_url.as_str() {
            TENDERMINT_MISBEHAVIOUR_TYPE_URL => {
                decode_misbehaviour(raw.value.deref()).map_err(Into::into)
            }
            _ => Err(ClientError::UnknownMisbehaviourType {
                misbehaviour_type: raw.type_url,
            }),
        }
    }
}

impl From<Misbehaviour> for Any {
    fn from(misbehaviour: Misbehaviour) -> Self {
        Any {
            type_url: TENDERMINT_MISBEHAVIOUR_TYPE_URL.to_string(),
            value: Protobuf::<RawMisbehaviour>::encode_vec(&misbehaviour)
                .expect("encoding to `Any` from `TmMisbehaviour`"),
        }
    }
}

pub fn decode_misbehaviour<B: Buf>(buf: B) -> Result<Misbehaviour, Error> {
    RawMisbehaviour::decode(buf)
        .map_err(Error::Decode)?
        .try_into()
}

impl core::fmt::Display for Misbehaviour {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result<(), core::fmt::Error> {
        write!(
            f,
            "{} h1: {}-{} h2: {}-{}",
            self.client_id,
            self.header1.height(),
            self.header1.trusted_height,
            self.header2.height(),
            self.header2.trusted_height,
        )
    }
}
