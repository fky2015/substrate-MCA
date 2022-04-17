/// QUESTION: what's the difference between `voters` and `authority_set`?
use std::{
	collections::{BTreeMap, HashMap},
	sync::Arc, pin::Pin, marker::PhantomData,
};

use crate::{
	authorities::SharedAuthoritySet, communication::Network as NetworkT, ClientForPbft, Config,
	SignedMessage, CommandOrError,
};
use finality_grandpa::{
	leader::{self, voter::report::ViewState, VoterSet},
	BlockNumberOps,
};
use futures::{Sink, Stream, Future};
use parity_scale_codec::{Decode, Encode};
use parking_lot::RwLock;
use prometheus_endpoint::{register, Counter, Gauge, PrometheusError, U64};
use sc_client_api::backend::Backend as BackendT;
use sc_telemetry::TelemetryHandle;
use sp_consensus::SelectChain as SelectChainT;
use sp_finality_pbft::{AuthorityId, SetId, ViewNumber, AuthoritySignature, PbftApi};
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, NumberFor};

/// The environment we run PBFT in.
pub(crate) struct Environment<Backend, Block: BlockT, C, N: NetworkT<Block>, SC> {
	pub(crate) client: Arc<C>,
	pub(crate) select_chain: SC,
	pub(crate) voters: Arc<VoterSet<AuthorityId>>,
	pub(crate) config: Config,
	pub(crate) authority_set: SharedAuthoritySet<Block::Hash, NumberFor<Block>>,
	pub(crate) network: crate::communication::NetworkBridge<Block, N>,
	pub(crate) set_id: SetId,
	pub(crate) voter_set_state: SharedVoterSetState<Block>,
	pub(crate) metrics: Option<Metrics>,
	// pub(crate) justification_sender: Option<GrandpaJustificationSender<Block>>,
	pub(crate) telemetry: Option<TelemetryHandle>,
	pub(crate) _phantom: PhantomData<Backend>,
}

impl<B, Block, C, N, SC> leader::voter::Environment for Environment<B, Block, C, N, SC>
where
	Block: BlockT,
	B: BackendT<Block>,
	C: ClientForPbft<Block, B> + 'static,
	C::Api: PbftApi<Block>,
	N: NetworkT<Block>,
	SC: SelectChainT<Block> + 'static,
	NumberFor<Block>: BlockNumberOps,
{
	type Timer = Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>>;


	type Id = AuthorityId;

	type Signature = AuthoritySignature;

	type In = Pin<
		Box<
			dyn Stream<
    // FIXME:
					Item = Result<
						::finality_grandpa::leader::SignedMessage<
							Block::Hash,
							NumberFor<Block>,
							Self::Signature,
							Self::Id,
						>,
						Self::Error,
					>,
				> + Send,
		>,
	>;


	type Out = Pin<
		Box<
			dyn Sink<
    // FIXME:
					::finality_grandpa::leader::Message<Block::Hash, NumberFor<Block>>,
					Error = Self::Error,
				> + Send,
		>,
	>;

	type Error = CommandOrError<Block::Hash, NumberFor<Block>>;

	type Hash = Block::Hash;

	type Number = NumberFor<Block>;

	fn voter_data(&self) -> leader::voter::communicate::VoterData<Self::Id> {
		todo!()
	}

	fn round_data(
		&self,
		view: u64,
	) -> leader::voter::communicate::RoundData<Self::Id, Self::Timer, Self::In, Self::Out> {
		todo!()
	}

	fn preprepare(&self, view: u64) -> Option<(Self::Hash, Self::Number)> {
		todo!()
	}

	fn finalize_block(
		&self,
		hash: Self::Hash,
		number: Self::Number,
		// commit: Message::Commit,
	) -> bool {
		todo!()
	}
}

/// Whether we've voted already during a prior run of the program.
#[derive(Clone, Debug, Decode, Encode, PartialEq)]
pub enum HasVoted<Block: BlockT> {
	/// Has not voted already in this round.
	No,
	/// Has voted in this round.
	Yes(AuthorityId, leader::Message<NumberFor<Block>, <Block as BlockT>::Hash>),
}

/// A map with voter status information for currently live rounds,
/// which votes have we cast and what are they.
pub type CurrentViews<Block> = BTreeMap<ViewNumber, HasVoted<Block>>;

/// Data about a completed round. The set of votes that is stored must be
/// minimal, i.e. at most one equivocation is stored per voter.
#[derive(Debug, Clone, Decode, Encode, PartialEq)]
pub struct CompletedView<Block: BlockT> {
	/// The view number.
	pub number: ViewNumber,
	/// The view state (prevote ghost, estimate, finalized, etc.)
	pub state: ViewState<Block::Hash, NumberFor<Block>>,
	/// The target block base used for voting in the round.
	pub base: (Block::Hash, NumberFor<Block>),
	/// All the votes observed in the round.
	pub votes: Vec<SignedMessage<Block>>,
}

// Data about last completed rounds within a single voter set. Stores
// NUM_LAST_COMPLETED_ROUNDS and always contains data about at least one round
// (genesis).
#[derive(Debug, Clone, PartialEq)]
pub struct CompletedViews<Block: BlockT> {
	rounds: Vec<CompletedView<Block>>,
	set_id: SetId,
	voters: Vec<AuthorityId>,
}

/// The state of the current voter set, whether it is currently active or not
/// and information related to the previously completed rounds. Current round
/// voting status is used when restarting the voter, i.e. it will re-use the
/// previous votes for a given round if appropriate (same round and same local
/// key).
#[derive(Debug, Decode, Encode, PartialEq)]
pub enum VoterSetState<Block: BlockT> {
	/// The voter is live, i.e. participating in rounds.
	Live {
		/// The previously completed rounds.
		completed_rounds: CompletedViews<Block>,
		/// Voter status for the currently live rounds.
		current_rounds: CurrentViews<Block>,
	},
	/// The voter is paused, i.e. not casting or importing any votes.
	Paused {
		/// The previously completed rounds.
		completed_rounds: CompletedViews<Block>,
	},
}

/// A voter set state meant to be shared safely across multiple owners.
#[derive(Clone)]
pub struct SharedVoterSetState<Block: BlockT> {
	/// The inner shared `VoterSetState`.
	inner: Arc<RwLock<VoterSetState<Block>>>,
	/// A tracker for the rounds that we are actively participating on (i.e. voting)
	/// and the authority id under which we are doing it.
	voting: Arc<RwLock<HashMap<ViewNumber, AuthorityId>>>,
}

impl<Block: BlockT> From<VoterSetState<Block>> for SharedVoterSetState<Block> {
	fn from(set_state: VoterSetState<Block>) -> Self {
		SharedVoterSetState::new(set_state)
	}
}

/// Prometheus metrics for PBFT.
#[derive(Clone)]
pub(crate) struct Metrics {
	finality_pbft_view: Gauge<U64>,
	finality_pbft_prepares: Counter<U64>,
	finality_pbft_commits: Counter<U64>,
}

impl Metrics {
	pub(crate) fn register(
		registry: &prometheus_endpoint::Registry,
	) -> Result<Self, PrometheusError> {
		Ok(Self {
			finality_pbft_view: register(
				Gauge::new("substrate_finality_pbft_round", "Highest completed PBFT round.")?,
				registry,
			)?,
			finality_pbft_prepares: register(
				Counter::new(
					"substrate_finality_pbft_prepares_total",
					"Total number of PBFT prevotes cast locally.",
				)?,
				registry,
			)?,
			finality_pbft_commits: register(
				Counter::new(
					"substrate_finality_pbft_commits_total",
					"Total number of GRANDPA commits cast locally.",
				)?,
				registry,
			)?,
		})
	}
}
