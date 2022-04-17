// TODO: Or say, currently lacked features:
//  - authority change & hard fork logic
//  - justification
use sc_client_api::{Backend, TransactionFor};
use sc_consensus::{BlockCheckParams, BlockImport, BlockImportParams, ImportResult};
use sc_telemetry::TelemetryHandle;
use sc_utils::mpsc::TracingUnboundedSender;
use sp_api::NumberFor;
use sp_blockchain::well_known_cache_keys;
use sp_consensus::Error as ConsensusError;
use sp_finality_pbft::{PbftApi, SetId};
use sp_runtime::{
	generic::BlockId,
	traits::{Block as BlockT, Header as HeaderT},
};
use std::{collections::HashMap, marker::PhantomData, sync::Arc};

use crate::{ClientForPbft, authorities::{SharedAuthoritySet, PendingChange}, VoterCommand};

/// A block-import handler for PBFT.
///
/// TODO: This scans each imported block for signals of changing authority set.
/// If the block being imported enacts an authority set change then:
/// - If the current authority set is still live: we import the block
/// - Otherwise, the block must include a valid justification.
///
/// When using PBFT, the block import worker should be using this block import
/// object.
#[derive(Clone)]
pub struct PbftBlockImport<Backend, Block: BlockT, Client, SC> {
	inner: Arc<Client>,
	select_chain: SC,
	authority_set: SharedAuthoritySet<Block::Hash, NumberFor<Block>>,
	send_voter_commands: TracingUnboundedSender<VoterCommand<Block::Hash, NumberFor<Block>>>,
	authority_set_hard_forks: HashMap<Block::Hash, PendingChange<Block::Hash, NumberFor<Block>>>,
	telemetry: Option<TelemetryHandle>,
	_phantom: PhantomData<Backend>,
}

// impl<Backend, Block: BlockT, Client, SC: Clone> Clone
// 	for PbftBlockImport<Backend, Block, Client, SC>
// {
// 	fn clone(&self) -> Self {
// 		PbftBlockImport {
// 			inner: self.inner.clone(),
// 			select_chain: self.select_chain.clone(),
// 			authority_set: self.authority_set.clone(),
// 			send_voter_commands: self.send_voter_commands.clone(),
// 			authority_set_hard_forks: self.authority_set_hard_forks.clone(),
// 			telemetry: self.telemetry.clone(),
// 			_phantom: PhantomData,
// 		}
// 	}
// }
//
impl<Backend, Block: BlockT, Client, SC> PbftBlockImport<Backend, Block, Client, SC> {
	pub fn new(
		inner: Arc<Client>,
		select_chain: SC,
		authority_set: SharedAuthoritySet<Block::Hash, NumberFor<Block>>,
		send_voter_commands: TracingUnboundedSender<VoterCommand<Block::Hash, NumberFor<Block>>>,
		authority_set_hard_forks: Vec<(SetId, PendingChange<Block::Hash, NumberFor<Block>>)>,
		telemetry: Option<TelemetryHandle>,
	) -> Self {
		// check for and apply any forced authority set hard fork that applies
		// to the *current* authority set.
		if let Some((_, change)) = authority_set_hard_forks
			.iter()
			.find(|(set_id, _)| *set_id == authority_set.set_id())
		{
			authority_set.inner().current_authorities = change.next_authorities.clone();
		}

		// index authority set hard forks by block hash so that they can be used
		// by any node syncing the chain and importing a block hard fork
		// authority set changes.
		let authority_set_hard_forks = authority_set_hard_forks
			.into_iter()
			.map(|(_, change)| (change.canon_hash, change))
			.collect::<HashMap<_, _>>();

		// check for and apply any forced authority set hard fork that apply to
		// any *pending* standard changes, checking by the block hash at which
		// they were announced.
		{
			let mut authority_set = authority_set.inner();

			authority_set.pending_standard_changes =
				authority_set.pending_standard_changes.clone().map(&mut |hash, _, original| {
					authority_set_hard_forks.get(&hash).cloned().unwrap_or(original)
				});
		}

		Self {
			inner,
			select_chain,
			authority_set,
			send_voter_commands,
			authority_set_hard_forks,
			telemetry,
			_phantom: PhantomData,
		}
	}
}

// impl<Backend, Block: BlockT, Client> Clone for PbftBlockImport<Backend, Block, Client> {
// 	fn clone(&self) -> Self {
// 		PbftBlockImport {
// 			inner: self.inner.clone(),
// 			authority_set: self.authority_set.clone(),

// 		}
// 	}
// }

#[async_trait::async_trait]
impl<BE, Block: BlockT, Client, SC> BlockImport<Block> for PbftBlockImport<BE, Block, Client, SC>
where
	NumberFor<Block>: finality_grandpa::BlockNumberOps,
	BE: Backend<Block>,
    // TODO: Why this bound: `sc_client_api::Backend<Block>`
	Client: ClientForPbft<Block, BE> + sc_client_api::Backend<Block>,
	Client::Api: PbftApi<Block>,
	for<'a> &'a Client:
		BlockImport<Block, Error = ConsensusError, Transaction = TransactionFor<Client, Block>>,
	TransactionFor<Client, Block>: 'static,
	SC: Send,
{
	type Error = ConsensusError;
	type Transaction = TransactionFor<Client, Block>;

	async fn import_block(
		&mut self,
		mut block: BlockImportParams<Block, Self::Transaction>,
		new_cache: HashMap<well_known_cache_keys::Id, Vec<u8>>,
	) -> Result<ImportResult, Self::Error> {
		Ok(())
	}

	async fn check_block(
		&mut self,
		block: BlockCheckParams<Block>,
	) -> Result<ImportResult, Self::Error> {
		self.inner.check_block(block).await
	}
}
