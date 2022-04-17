use std::sync::Arc;

use parity_scale_codec::{Decode, Encode};
use parking_lot::Mutex;
use sc_network_gossip::{GossipEngine, Network as GossipNetwork};
use sc_telemetry::TelemetryHandle;
use sc_utils::mpsc::TracingUnboundedReceiver;
use sp_api::NumberFor;
use sp_finality_pbft::{SetId as SetIdNumber, ViewNumber};
use sp_runtime::traits::Block as BlockT;

use self::gossip::{GossipValidator, PeerReport};

mod gossip;

mod periodic;

/// A handle to the network.
///
/// Something that provides both the capabilities needed for the `gossip_network::Network` trait as
/// well as the ability to set a fork sync request for a particular block.
pub trait Network<Block: BlockT>: GossipNetwork<Block> + Clone + Send + 'static {
	/// Notifies the sync service to try and sync the given block from the given
	/// peers.
	///
	/// If the given vector of peers is empty then the underlying implementation
	/// should make a best effort to fetch the block from any peers it is
	/// connected to (NOTE: this assumption will change in the future #3629).
	fn set_sync_fork_request(
		&self,
		peers: Vec<sc_network::PeerId>,
		hash: Block::Hash,
		number: NumberFor<Block>,
	);
}

/// Bridge between the underlying network service, gossiping consensus messages and Grandpa
pub(crate) struct NetworkBridge<B: BlockT, N: Network<B>> {
	service: N,
	gossip_engine: Arc<Mutex<GossipEngine<B>>>,
	validator: Arc<GossipValidator<B>>,

	/// Sender side of the neighbor packet channel.
	///
	/// Packets sent into this channel are processed by the `NeighborPacketWorker` and passed on to
	/// the underlying `GossipEngine`.
	neighbor_sender: periodic::NeighborPacketSender<B>,

	/// `NeighborPacketWorker` processing packets sent through the `NeighborPacketSender`.
	// `NetworkBridge` is required to be cloneable, thus one needs to be able to clone its
	// children, thus one has to wrap `neighbor_packet_worker` with an `Arc` `Mutex`.
	neighbor_packet_worker: Arc<Mutex<periodic::NeighborPacketWorker<B>>>,

	/// Receiver side of the peer report stream populated by the gossip validator, forwarded to the
	/// gossip engine.
	// `NetworkBridge` is required to be cloneable, thus one needs to be able to clone its
	// children, thus one has to wrap gossip_validator_report_stream with an `Arc` `Mutex`. Given
	// that it is just an `UnboundedReceiver`, one could also switch to a
	// multi-producer-*multi*-consumer channel implementation.
	gossip_validator_report_stream: Arc<Mutex<TracingUnboundedReceiver<PeerReport>>>,

	telemetry: Option<TelemetryHandle>,
}

/// Type-safe wrapper around a round number.
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Encode, Decode)]
pub struct View(pub ViewNumber);

/// Type-safe wrapper around a set ID.
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Encode, Decode)]
pub struct SetId(pub SetIdNumber);
