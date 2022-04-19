use std::sync::Arc;

use finality_grandpa::leader::voter;
use futures::{future, StreamExt};
use log::{debug, trace};
use parity_scale_codec::{Decode, Encode};
use parking_lot::Mutex;
use prometheus_endpoint::Registry;
use sc_network_gossip::{GossipEngine, Network as GossipNetwork};
use sc_telemetry::{telemetry, TelemetryHandle};
use sc_utils::mpsc::TracingUnboundedReceiver;
use sp_finality_pbft::{AuthorityId, SetId as SetIdNumber, ViewNumber};
use sp_keystore::SyncCryptoStorePtr;
use sp_runtime::traits::{Block as BlockT, Hash as HashT, Header as HeaderT, NumberFor};

use self::gossip::{GossipValidator, PeerReport};

mod gossip;

mod periodic;

pub mod pbft_protocol_name {
	use sc_chain_spec::ChainSpec;

	pub(crate) const NAME: &'static str = "/pbft/1";

	/// Name of the notifications protocol used by GRANDPA.
	///
	/// Must be registered towards the networking in order for GRANDPA to properly function.
	pub fn standard_name<Hash: std::fmt::Display>(
		genesis_hash: &Hash,
		chain_spec: &Box<dyn ChainSpec>,
	) -> std::borrow::Cow<'static, str> {
		let chain_prefix = match chain_spec.fork_id() {
			Some(fork_id) => format!("/{}/{}", genesis_hash, fork_id),
			None => format!("/{}", genesis_hash),
		};
		format!("{}{}", chain_prefix, NAME).into()
	}
}

/// A type that ties together our local authority id and a keystore where it is
/// available for signing.
pub struct LocalIdKeystore((AuthorityId, SyncCryptoStorePtr));

impl LocalIdKeystore {
	/// Returns a reference to our local authority id.
	fn local_id(&self) -> &AuthorityId {
		&(self.0).0
	}

	/// Returns a reference to the keystore.
	fn keystore(&self) -> SyncCryptoStorePtr {
		(self.0).1.clone()
	}
}

impl From<(AuthorityId, SyncCryptoStorePtr)> for LocalIdKeystore {
	fn from(inner: (AuthorityId, SyncCryptoStorePtr)) -> LocalIdKeystore {
		LocalIdKeystore(inner)
	}
}

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

/// Create a unique topic for a view and set-id combo.
pub(crate) fn view_topic<B: BlockT>(view: ViewNumber, set_id: SetIdNumber) -> B::Hash {
	<<B::Header as HeaderT>::Hashing as HashT>::hash(format!("{}-{}", set_id, view).as_bytes())
}

/// Create a unique topic for global messages on a set ID.
pub(crate) fn global_topic<B: BlockT>(set_id: SetIdNumber) -> B::Hash {
	<<B::Header as HeaderT>::Hashing as HashT>::hash(format!("{}-GLOBAL", set_id).as_bytes())
}

/// Bridge between the underlying network service, gossiping consensus messages and Grandpa
#[derive(Clone)]
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

impl<B: BlockT, N: Network<B>> Unpin for NetworkBridge<B, N> {}

impl<B: BlockT, N: Network<B>> NetworkBridge<B, N> {
	/// Create a new network bridge.
	pub fn new(
		service: N,
		config: crate::Config,
		set_state: crate::environment::SharedVoterSetState<B>,
		prometheus_registry: Option<&Registry>,
		telemetry: Option<TelemetryHandle>,
	) -> Self {
		// Get Protocol name.
		let protocal = config.protocol_name.clone();
		// Create GossipValidator.
		let (validator, report_stream) = Arc::new(GossipValidator::new(
			config,
			set_state.clone(),
			prometheus_registry,
			telemetry.clone(),
		));

		// Create GossipEngine.
		let gossip_engine = Arc::new(Mutex::new(GossipEngine::new(
			service.clone(),
			protocal,
			validator.clone(),
			prometheus_registry,
		)));

		// TODO: modify this, FKY.
		{
			// register all previous votes with the gossip service so that they're
			// available to peers potentially stuck on a previous view.
			let completed = set_state.read().completed_views();
			let (set_id, voters) = completed.set_info();
			validator.note_set(SetId(set_id), voters.to_vec(), |_, _| {});
			for view in completed.iter() {
				let topic = view_topic::<B>(view.number, set_id);

				// we need to note the view with the gossip validator otherwise
				// messages will be ignored.
				validator.note_view(View(view.number), |_, _| {});

				for signed in view.votes.iter() {
					let message = gossip::GossipMessage::Vote(gossip::VoteMessage::<B> {
						message: signed.clone(),
						round: View(view.number),
						set_id: SetId(set_id),
					});

					gossip_engine.lock().register_gossip_message(topic, message.encode());
				}

				trace!(target: "afg",
					"Registered {} messages for topic {:?} (view: {}, set_id: {})",
					view.votes.len(),
					topic,
					view.number,
					set_id,
				);
			}
		}

		let (neighbor_packet_worker, neighbor_packet_sender) =
			periodic::NeighborPacketWorker::new();

		NetworkBridge {
			service,
			gossip_engine,
			validator,
			neighbor_sender: neighbor_packet_sender,
			neighbor_packet_worker: Arc::new(Mutex::new(neighbor_packet_worker)),
			gossip_validator_report_stream: Arc::new(Mutex::new(report_stream)),
			telemetry,
		}
	}
	/// Note the beginning of a new view to the `GossipValidator`.
	pub(crate) fn note_view(&self, round: Round, set_id: SetId, voters: &VoterSet<AuthorityId>) {
		// is a no-op if currently in that set.
		self.validator.note_set(
			set_id,
			voters.iter().map(|(v, _)| v.clone()).collect(),
			|to, neighbor| self.neighbor_sender.send(to, neighbor),
		);

		self.validator
			.note_view(round, |to, neighbor| self.neighbor_sender.send(to, neighbor));
	}

	/// Get a stream of signature-checked round messages from the network as well as a sink for
	/// round messages to the network all within the current set.
	pub(crate) fn round_communication(
		&self,
		keystore: Option<LocalIdKeystore>,
		view: View,
		set_id: SetId,
		voters: Arc<VoterSet<AuthorityId>>,
		has_voted: HasVoted<B>,
	) -> (impl Stream<Item = SignedMessage<B>> + Unpin, OutgoingMessages<B>) {
		self.note_view(view, set_id, &*voters);

		let keystore = keystore.and_then(|ks| {
			let id = ks.local_id();
			if voters.contains(id) {
				Some(ks)
			} else {
				None
			}
		});

		let topic = view_topic::<B>(round.0, set_id.0);
		let telemetry = self.telemetry.clone();
		let incoming =
			self.gossip_engine.lock().messages_for(topic).filter_map(move |notification| {
				let decoded = GossipMessage::<B>::decode(&mut &notification.message[..]);

				match decoded {
					Err(ref e) => {
						debug!(target: "afg", "Skipping malformed message {:?}: {}", notification, e);
						future::ready(None)
					},
					Ok(GossipMessage::Vote(msg)) => {
						// check signature.
						if !voters.contains(&msg.message.id) {
							debug!(target: "afg", "Skipping message from unknown voter {}", msg.message.id);
							return future::ready(None)
						}

						if voters.len().get() <= TELEMETRY_VOTERS_LIMIT {
							match &msg.message.message {
								PrimaryPropose(propose) => {
									telemetry!(
										telemetry;
										CONSENSUS_INFO;
										"afg.received_propose";
										"voter" => ?format!("{}", msg.message.id),
										"target_number" => ?propose.target_number,
										"target_hash" => ?propose.target_hash,
									);
								},
								Prevote(prevote) => {
									telemetry!(
										telemetry;
										CONSENSUS_INFO;
										"afg.received_prevote";
										"voter" => ?format!("{}", msg.message.id),
										"target_number" => ?prevote.target_number,
										"target_hash" => ?prevote.target_hash,
									);
								},
								Precommit(precommit) => {
									telemetry!(
										telemetry;
										CONSENSUS_INFO;
										"afg.received_precommit";
										"voter" => ?format!("{}", msg.message.id),
										"target_number" => ?precommit.target_number,
										"target_hash" => ?precommit.target_hash,
									);
								},
							};
						}

						future::ready(Some(msg.message))
					},
					_ => {
						debug!(target: "afg", "Skipping unknown message type");
						future::ready(None)
					},
				}
			});

		let (tx, out_rx) = mpsc::channel(0);
		let outgoing = OutgoingMessages::<B> {
			keystore,
			round: round.0,
			set_id: set_id.0,
			network: self.gossip_engine.clone(),
			sender: tx,
			has_voted,
			telemetry: self.telemetry.clone(),
		};

		// Combine incoming votes from external GRANDPA nodes with outgoing
		// votes from our own GRANDPA voter to have a single
		// vote-import-pipeline.
		let incoming = stream::select(incoming, out_rx);

		(incoming, outgoing)
	}

	/// Set up the global communication streams.
	pub(crate) fn global_communication(
		&self,
		set_id: SetId,
		voters: Arc<VoterSet<AuthorityId>>,
		is_voter: bool,
	) -> (
		impl Stream<Item = CommunicationIn<B>>,
		impl Sink<CommunicationOutH<B, B::Hash>, Error = Error> + Unpin,
	) {
		self.validator.note_set(
			set_id,
			voters.iter().map(|(v, _)| v.clone()).collect(),
			|to, neighbor| self.neighbor_sender.send(to, neighbor),
		);

		let topic = global_topic::<B>(set_id.0);
		let incoming = incoming_global(
			self.gossip_engine.clone(),
			topic,
			voters,
			self.validator.clone(),
			self.neighbor_sender.clone(),
			self.telemetry.clone(),
		);

		let outgoing = CommitsOut::<B>::new(
			self.gossip_engine.clone(),
			set_id.0,
			is_voter,
			self.validator.clone(),
			self.neighbor_sender.clone(),
			self.telemetry.clone(),
		);

		let outgoing = outgoing.with(|out| {
			let voter::CommunicationOut::Commit(round, commit) = out;
			future::ok((round, commit))
		});

		(incoming, outgoing)
	}

	/// Notifies the sync service to try and sync the given block from the given
	/// peers.
	///
	/// If the given vector of peers is empty then the underlying implementation
	/// should make a best effort to fetch the block from any peers it is
	/// connected to (NOTE: this assumption will change in the future #3629).
	pub(crate) fn set_sync_fork_request(
		&self,
		peers: Vec<sc_network::PeerId>,
		hash: B::Hash,
		number: NumberFor<B>,
	) {
		Network::set_sync_fork_request(&self.service, peers, hash, number)
	}
}

/// Type-safe wrapper around a round number.
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Encode, Decode)]
pub struct View(pub ViewNumber);

/// Type-safe wrapper around a set ID.
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Encode, Decode)]
pub struct SetId(pub SetIdNumber);
