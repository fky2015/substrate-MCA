//! Schema for stuff in the aux-db.

use std::fmt::Debug;

use finality_grandpa::leader::view_round::State as ViewState;
use log::{info, warn};
use parity_scale_codec::{Decode, Encode};

use fork_tree::ForkTree;
use sc_client_api::backend::AuxStore;
use sp_blockchain::{Error as ClientError, Result as ClientResult};
use sp_finality_pbft::{AuthorityList, SetId, ViewNumber};
use sp_runtime::traits::{Block as BlockT, NumberFor};

use crate::{
	authorities::{AuthoritySet, SharedAuthoritySet},
	environment::{SharedVoterSetState, VoterSetState},
};

const VERSION_KEY: &[u8] = b"pbft_schema_version";
const SET_STATE_KEY: &[u8] = b"pbft_completed_round";
const CONCLUDED_ROUNDS: &[u8] = b"pbft_concluded_rounds";
const AUTHORITY_SET_KEY: &[u8] = b"pbft_voters";
const BEST_JUSTIFICATION: &[u8] = b"pbft_best_justification";

const CURRENT_VERSION: u32 = 3;

pub(crate) fn load_decode<B: AuxStore, T: Decode>(
	backend: &B,
	key: &[u8],
) -> ClientResult<Option<T>> {
	match backend.get_aux(key)? {
		None => Ok(None),
		Some(t) => T::decode(&mut &t[..])
			.map_err(|e| ClientError::Backend(format!("GRANDPA DB is corrupted: {}", e)))
			.map(Some),
	}
}
/// Persistent data kept between runs.
pub(crate) struct PersistentData<Block: BlockT> {
	pub(crate) authority_set: SharedAuthoritySet<Block::Hash, NumberFor<Block>>,
	pub(crate) set_state: SharedVoterSetState<Block>,
}
/// Load or initialize persistent data from backend.
pub(crate) fn load_persistent<Block: BlockT, B, G>(
	backend: &B,
	genesis_hash: Block::Hash,
	genesis_number: NumberFor<Block>,
	genesis_authorities: G,
) -> ClientResult<PersistentData<Block>>
where
	B: AuxStore,
	G: FnOnce() -> ClientResult<AuthorityList>,
{
	let version: Option<u32> = load_decode(backend, VERSION_KEY)?;

	let make_genesis_round = move || ViewState::genesis((genesis_hash, genesis_number));

	match version {
		Some(3) => {
			if let Some(set) = load_decode::<_, AuthoritySet<Block::Hash, NumberFor<Block>>>(
				backend,
				AUTHORITY_SET_KEY,
			)? {
				let set_state =
					match load_decode::<_, VoterSetState<Block>>(backend, SET_STATE_KEY)? {
						Some(state) => state,
						None => {
							let state = make_genesis_round();
							let base = state.prevote_ghost
							.expect("state is for completed round; completed rounds must have a prevote ghost; qed.");

							VoterSetState::live(set.set_id, &set, base)
						},
					};

				return Ok(PersistentData { authority_set: set.into(), set_state: set_state.into() })
			}
		},
		Some(other) =>
			return Err(ClientError::Backend(format!("Unsupported GRANDPA DB version: {:?}", other))),
	}

	// genesis.
	info!(target: "afg", "👴 Loading GRANDPA authority set \
		from genesis on what appears to be first startup.");

	let genesis_authorities = genesis_authorities()?;
	let genesis_set = AuthoritySet::genesis(genesis_authorities)
		.expect("genesis authorities is non-empty; all weights are non-zero; qed.");
	let state = make_genesis_round();
	let base = state
		.prevote_ghost
		.expect("state is for completed round; completed rounds must have a prevote ghost; qed.");

	let genesis_state = VoterSetState::live(0, &genesis_set, base);

	backend.insert_aux(
		&[
			(AUTHORITY_SET_KEY, genesis_set.encode().as_slice()),
			(SET_STATE_KEY, genesis_state.encode().as_slice()),
		],
		&[],
	)?;

	Ok(PersistentData { authority_set: genesis_set.into(), set_state: genesis_state.into() })
}

/// Write voter set state.
pub(crate) fn write_voter_set_state<Block: BlockT, B: AuxStore>(
	backend: &B,
	state: &VoterSetState<Block>,
) -> ClientResult<()> {
	backend.insert_aux(&[(SET_STATE_KEY, state.encode().as_slice())], &[])
}