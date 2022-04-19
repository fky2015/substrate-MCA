use std::{
	collections::{HashSet, VecDeque},
	time::Instant,
};

use ahash::{AHashMap, AHashSet};
use parity_scale_codec::{Decode, Encode};
use sc_network::{ObservedRole, PeerId, ReputationChange};
use sc_telemetry::TelemetryHandle;
use sc_utils::mpsc::TracingUnboundedSender;
use sp_api::NumberFor;
use sp_finality_pbft::AuthorityId;
use sp_runtime::traits::Block as BlockT;

use crate::{environment, SignedMessage};

use super::{SetId, View};

/// A view of protocal state.
struct PeerView<N> {
	view: View,             // the current view we are at.
	set_id: SetId,          // the current voter set id.
	last_commit: Option<N>, // commit-finalized block height, if any.
}

#[derive(Debug)]
struct PeerInfo<N> {
	view: PeerView<N>,
	roles: ObservedRole,
}
/// The peers we're connected to in gossip.
struct Peers<N> {
	inner: AHashMap<PeerId, PeerInfo<N>>,
	/// The randomly picked set of `LUCKY_PEERS` we'll gossip to in the first stage of round
	/// gossiping.
	first_stage_peers: AHashSet<PeerId>,
	/// The randomly picked set of peers we'll gossip to in the second stage of gossiping if
	/// the first stage didn't allow us to spread the voting data enough to conclude the round.
	/// This set should have size `sqrt(connected_peers)`.
	second_stage_peers: HashSet<PeerId>,
	/// The randomly picked set of `LUCKY_PEERS` light clients we'll gossip commit messages to.
	lucky_light_peers: HashSet<PeerId>,
}
/// Tracks gossip topics that we are keeping messages for. We keep topics of:
///
/// - the last `KEEP_RECENT_ROUNDS` complete GRANDPA rounds,
///
/// - the topic for the current and next round,
///
/// - and a global topic for commit and catch-up messages.
struct KeepTopics<B: BlockT> {
	current_set: SetId,
	rounds: VecDeque<(View, SetId)>,
	reverse_map: AHashMap<B::Hash, (Option<View>, SetId)>,
}

/// A local view of protocol state. Similar to `View` but we additionally track
/// the round and set id at which the last commit was observed, and the instant
/// at which the current round started.
struct LocalView<N> {
	view: View,
	set_id: SetId,
	last_commit: Option<(N, View, SetId)>,
	round_start: Instant,
}

/// Configuration for the round catch-up mechanism.
enum CatchUpConfig {
	/// Catch requests are enabled, our node will issue them whenever it sees a
	/// neighbor packet for a round further than `CATCH_UP_THRESHOLD`. If
	/// `only_from_authorities` is set, the node will only send catch-up
	/// requests to other authorities it is connected to. This is useful if the
	/// GRANDPA observer protocol is live on the network, in which case full
	/// nodes (non-authorities) don't have the necessary round data to answer
	/// catch-up requests.
	Enabled { only_from_authorities: bool },
	/// Catch-up requests are disabled, our node will never issue them. This is
	/// useful for the GRANDPA observer mode, where we are only interested in
	/// commit messages and don't need to follow the full round protocol.
	Disabled,
}

/// A catch up request for a given round (or any further round) localized by set id.
#[derive(Clone, Debug, Encode, Decode)]
pub(super) struct CatchUpRequestMessage {
	/// The view that we want to catch up to.
	pub(super) view: View,
	/// The voter set ID this message is from.
	pub(super) set_id: SetId,
}
/// State of catch up request handling.
#[derive(Debug)]
enum PendingCatchUp {
	/// No pending catch up requests.
	None,
	/// Pending catch up request which has not been answered yet.
	Requesting { who: PeerId, request: CatchUpRequestMessage, instant: Instant },
	/// Pending catch up request that was answered and is being processed.
	Processing { instant: Instant },
}

struct Inner<Block: BlockT> {
	local_view: Option<LocalView<NumberFor<Block>>>,
	peers: Peers<NumberFor<Block>>,
	live_topics: KeepTopics<Block>,
	authorities: Vec<AuthorityId>,
	config: crate::Config,
	next_rebroadcast: Instant,
	pending_catch_up: PendingCatchUp,
	catch_up_config: CatchUpConfig,
}

/// A validator for PBFT gossip messages.
pub(super) struct GossipValidator<Block: BlockT> {
	inner: parking_lot::RwLock<Inner<Block>>,
	set_state: environment::SharedVoterSetState<Block>,
	report_sender: TracingUnboundedSender<PeerReport>,
	// metrics: Option<Metrics>,
	telemetry: Option<TelemetryHandle>,
}

/// Grandpa gossip message type.
/// This is the root type that gets encoded and sent on the network.
#[derive(Debug, Encode, Decode)]
pub(super) enum GossipMessage<Block: BlockT> {
	/// PBFT message with view and set info.
	Message(Message<Block>),
	// FIXME: add Global Message (FKY)
}

/// Network level message with topic information.
#[derive(Debug, Encode, Decode)]
pub(super) struct Message<Block: BlockT> {
	/// The view this message is from.
	pub(super) view: View,
	/// The voter set ID this message is from.
	pub(super) set_id: SetId,
	/// The message itself.
	pub(super) message: SignedMessage<Block>,
}

/// V1 neighbor packet. Neighbor packets are sent from nodes to their peers
/// and are not repropagated. These contain information about the node's state.
#[derive(Debug, Encode, Decode, Clone)]
pub(super) struct NeighborPacket<N> {
	/// The round the node is currently at.
	pub(super) view: View,
	/// The set ID the node is currently at.
	pub(super) set_id: SetId,
	/// The highest finalizing commit observed.
	pub(super) commit_finalized_height: N,
}

/// Report specifying a reputation change for a given peer.
pub(super) struct PeerReport {
	pub who: PeerId,
	pub cost_benefit: ReputationChange,
}
