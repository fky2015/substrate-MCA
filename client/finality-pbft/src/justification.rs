use std::{
	collections::{HashMap, HashSet},
	sync::Arc,
};

use finality_grandpa::leader::{self, Error as PbftError, VoterSet};
use parity_scale_codec::{Decode, Encode};
use sp_blockchain::{Error as ClientError, HeaderBackend};
use sp_finality_pbft::AuthorityId;
use sp_runtime::{
	generic::BlockId,
	traits::{Block as BlockT, Header as HeaderT, NumberFor},
};

use crate::{AuthorityList, Error, FinalizedCommit};

/// A GRANDPA justification for block finality, it includes a commit message and
/// an ancestry proof including all headers routing all commit target blocks
/// to the commit target block. Due to the current voting strategy the commit
/// targets should be the same as the commit target, since honest voters don't
/// vote past authority set change blocks.
///
/// This is meant to be stored in the db and passed around the network to other
/// nodes, and are used by syncing nodes to prove authority set handoffs.
#[derive(Clone, Encode, Decode, PartialEq, Eq, Debug)]
pub struct PbftJustification<Block: BlockT> {
	view: u64,
	pub(crate) f_commit: FinalizedCommit<Block>,
}

impl<Block: BlockT> PbftJustification<Block> {
	/// Create a GRANDPA justification from the given commit. This method
	/// assumes the commit is valid and well-formed.
	pub fn from_commit<C>(
		client: &Arc<C>,
		view: u64,
		f_commit: FinalizedCommit<Block>,
	) -> Result<PbftJustification<Block>, Error>
	where
		C: HeaderBackend<Block>,
	{
		Ok(PbftJustification { view, f_commit })
	}

	/// Decode a GRANDPA justification and validate the commit and the votes'
	/// ancestry proofs finalize the given block.
	pub fn decode_and_verify_finalizes(
		encoded: &[u8],
		finalized_target: (Block::Hash, NumberFor<Block>),
		set_id: u64,
		voters: &VoterSet<AuthorityId>,
	) -> Result<PbftJustification<Block>, ClientError>
	where
		NumberFor<Block>: leader::BlockNumberOps,
	{
		let justification = PbftJustification::<Block>::decode(&mut &*encoded)
			.map_err(|_| ClientError::JustificationDecode)?;

		if (justification.f_commit.target_hash, justification.f_commit.target_number)
			!= finalized_target
		{
			let msg = "invalid commit target in pbft justification".to_string();
			Err(ClientError::BadJustification(msg))
		} else {
			justification.verify_with_voter_set(set_id, voters).map(|_| justification)
		}
	}

	/// Validate the commit and the votes' ancestry proofs.
	pub fn verify(&self, set_id: u64, authorities: &AuthorityList) -> Result<(), ClientError>
	where
		NumberFor<Block>: leader::BlockNumberOps,
	{
		let voters = VoterSet::new(authorities.to_vec())
			.ok_or(ClientError::Consensus(sp_consensus::Error::InvalidAuthoritiesSet))?;

		self.verify_with_voter_set(set_id, &voters)
	}

	/// Validate the commit and the votes' ancestry proofs.
	pub(crate) fn verify_with_voter_set(
		&self,
		set_id: u64,
		voters: &VoterSet<AuthorityId>,
	) -> Result<(), ClientError>
	where
		NumberFor<Block>: leader::BlockNumberOps,
	{
		let mut buf = Vec::new();
		for signed in self.f_commit.commits.iter() {
			if !sp_finality_pbft::check_message_signature_with_buffer(
				&leader::Message::Commit(signed.commit.clone()),
				&signed.id,
				&signed.signature,
				self.view,
				set_id,
				&mut buf,
			) {
				return Err(ClientError::BadJustification(
					// FIXME:
					"invalid signature for commit in pbft justification".to_string(),
				));
			}

			if self.f_commit.target_hash == signed.commit.target_hash {
				continue;
			}

			return Err(ClientError::BadJustification(
				// FIXME:
				"invalid commit ancestry proof in pbft justification".to_string(),
			));
		}

		Ok(())
	}

	/// The target block number and hash that this justifications proves finality for.
	pub fn target(&self) -> (NumberFor<Block>, Block::Hash) {
		(self.f_commit.target_number, self.f_commit.target_hash)
	}
}
