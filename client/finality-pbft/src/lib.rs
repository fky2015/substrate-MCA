use std::{marker::PhantomData, pin::Pin, sync::Arc, time::Duration};

use authorities::SharedAuthoritySet;
use aux_schema::PersistentData;
use communication::{Network as NetworkT, NetworkBridge};
use environment::{Environment};
use finality_grandpa::{
	leader::{self, Error as PbftError, VoterSet},
	BlockNumberOps,
};
use futures::{future, Future, Sink, SinkExt, Stream, TryStreamExt};
use import::PbftBlockImport;
use log::{debug, info};
use parity_scale_codec::Decode;
use parking_lot::RwLock;
use prometheus_endpoint::{PrometheusError, Registry};
use sc_client_api::{
	AuxStore, Backend, BlockchainEvents, CallExecutor, ExecutionStrategy, ExecutorProvider,
	Finalizer, HeaderBackend, LockImportRun, StorageProvider, TransactionFor,
};
use sc_consensus::BlockImport;
use sc_telemetry::{telemetry, TelemetryHandle, CONSENSUS_DEBUG, CONSENSUS_INFO};
use sc_utils::mpsc::{tracing_unbounded, TracingUnboundedReceiver};
use sp_api::ProvideRuntimeApi;
use sp_blockchain::{Error as ClientError, HeaderMetadata};
use sp_consensus::SelectChain;
use sp_finality_pbft::{AuthorityId, AuthorityList, AuthoritySignature, PbftApi, SetId};
use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
use sp_runtime::{
	generic::BlockId,
	traits::{Block as BlockT, Header as HeaderT, NumberFor, Zero},
};
use crate::environment::VoterSetState;

pub(crate) mod communication;
pub(crate) mod import;

pub(crate) mod authorities;
pub(crate) mod aux_schema;
pub(crate) mod environment;
pub(crate) mod until_imported;

use until_imported::UntilGlobalMessageBlocksImported;


/// A global communication input stream for commits and catch up messages. Not
/// exposed publicly, used internally to simplify types in the communication
/// layer.
type GlobalCommunication = leader::GlobalMessage<AuthorityId>;

pub type GlobalMessage = leader::GlobalMessage<AuthorityId>;

/// Link between the block importer and the background voter.
pub struct LinkHalf<Block: BlockT, C, SC> {
	client: Arc<C>,
	select_chain: SC,
	persistent_data: PersistentData<Block>,
	voter_commands_rx: TracingUnboundedReceiver<VoterCommand<Block::Hash, NumberFor<Block>>>,
	telemetry: Option<TelemetryHandle>,
}

impl<Block: BlockT, C, SC> LinkHalf<Block, C, SC> {
	/// Get the shared authority set.
	pub fn shared_authority_set(&self) -> &SharedAuthoritySet<Block::Hash, NumberFor<Block>> {
		&self.persistent_data.authority_set
	}
}

/// Provider for the Grandpa authority set configured on the genesis block.
pub trait GenesisAuthoritySetProvider<Block: BlockT> {
	/// Get the authority set at the genesis block.
	fn get(&self) -> Result<AuthorityList, ClientError>;
}

impl<Block: BlockT, E> GenesisAuthoritySetProvider<Block>
	for Arc<dyn ExecutorProvider<Block, Executor = E>>
where
	E: CallExecutor<Block>,
{
	fn get(&self) -> Result<AuthorityList, ClientError> {
		// This implementation uses the Grandpa runtime API instead of reading directly from the
		// `GRANDPA_AUTHORITIES_KEY` as the data may have been migrated since the genesis block of
		// the chain, whereas the runtime API is backwards compatible.
		self.executor()
			.call(
				&BlockId::Number(Zero::zero()),
				"GrandpaApi_grandpa_authorities",
				&[],
				ExecutionStrategy::NativeElseWasm,
				None,
			)
			.and_then(|call_result| {
				Decode::decode(&mut &call_result[..]).map_err(|err| {
					ClientError::CallResultDecode(
						"failed to decode GRANDPA authorities set proof",
						err,
					)
				})
			})
	}
}

/// Make block importer and link half necessary to tie the background voter
/// to it.
pub fn block_import<BE, Block: BlockT, Client, SC>(
	client: Arc<Client>,
	genesis_authorities_provider: &dyn GenesisAuthoritySetProvider<Block>,
	select_chain: SC,
	telemetry: Option<TelemetryHandle>,
) -> Result<(PbftBlockImport<BE, Block, Client, SC>, LinkHalf<Block, Client, SC>), ClientError>
where
	SC: SelectChain<Block>,
	BE: Backend<Block> + 'static,
	Client: ClientForPbft<Block, BE> + 'static,
{
	block_import_with_authority_set_hard_forks(
		client,
		genesis_authorities_provider,
		select_chain,
		Default::default(),
		telemetry,
	)
}

/// A descriptor for an authority set hard fork. These are authority set changes
/// that are not signalled by the runtime and instead are defined off-chain
/// (hence the hard fork).
pub struct AuthoritySetHardFork<Block: BlockT> {
	/// The new authority set id.
	pub set_id: SetId,
	/// The block hash and number at which the hard fork should be applied.
	pub block: (Block::Hash, NumberFor<Block>),
	/// The authorities in the new set.
	pub authorities: AuthorityList,
	/// The latest block number that was finalized before this authority set
	/// hard fork. When defined, the authority set change will be forced, i.e.
	/// the node won't wait for the block above to be finalized before enacting
	/// the change, and the given finalized number will be used as a base for
	/// voting.
	pub last_finalized: Option<NumberFor<Block>>,
}
/// Make block importer and link half necessary to tie the background voter to
/// it. A vector of authority set hard forks can be passed, any authority set
/// change signaled at the given block (either already signalled or in a further
/// block when importing it) will be replaced by a standard change with the
/// given static authorities.
pub fn block_import_with_authority_set_hard_forks<BE, Block: BlockT, Client, SC>(
	client: Arc<Client>,
	genesis_authorities_provider: &dyn GenesisAuthoritySetProvider<Block>,
	select_chain: SC,
	authority_set_hard_forks: Vec<AuthoritySetHardFork<Block>>,
	telemetry: Option<TelemetryHandle>,
) -> Result<(PbftBlockImport<BE, Block, Client, SC>, LinkHalf<Block, Client, SC>), ClientError>
where
	SC: SelectChain<Block>,
	BE: Backend<Block> + 'static,
	Client: ClientForPbft<Block, BE> + 'static,
{
	let chain_info = client.info();
	let genesis_hash = chain_info.genesis_hash;

	let persistent_data =
		aux_schema::load_persistent(&*client, genesis_hash, <NumberFor<Block>>::zero(), {
			let telemetry = telemetry.clone();
			move || {
				let authorities = genesis_authorities_provider.get()?;
				telemetry!(
					telemetry;
					CONSENSUS_DEBUG;
					"afg.loading_authorities";
					"authorities_len" => ?authorities.len()
				);
				Ok(authorities)
			}
		})?;

	let (voter_commands_tx, voter_commands_rx) = tracing_unbounded("mpsc_grandpa_voter_command");

	// let (justification_sender, justification_stream) = GrandpaJustificationStream::channel();

	// create pending change objects with 0 delay for each authority set hard fork.
	let authority_set_hard_forks = authority_set_hard_forks
		.into_iter()
		.map(|fork| {
			let delay_kind = if let Some(last_finalized) = fork.last_finalized {
				authorities::DelayKind::Best { median_last_finalized: last_finalized }
			} else {
				authorities::DelayKind::Finalized
			};

			(
				fork.set_id,
				authorities::PendingChange {
					next_authorities: fork.authorities,
					delay: Zero::zero(),
					canon_hash: fork.block.0,
					canon_height: fork.block.1,
					delay_kind,
				},
			)
		})
		.collect();

	Ok((
		PbftBlockImport::new(
			client.clone(),
			select_chain.clone(),
			persistent_data.authority_set.clone(),
			voter_commands_tx,
			authority_set_hard_forks,
			telemetry.clone(),
		),
		LinkHalf { client, select_chain, persistent_data, voter_commands_rx, telemetry },
	))
}

fn global_communication<BE, Block: BlockT, C, N>(
	set_id: SetId,
	voters: &Arc<VoterSet<AuthorityId>>,
	client: Arc<C>,
	network: &NetworkBridge<Block, N>,
	keystore: Option<&SyncCryptoStorePtr>,
	metrics: Option<until_imported::Metrics>,
) -> (
	impl Stream<Item = Result<GlobalCommunication, CommandOrError<Block::Hash, NumberFor<Block>>>>,
	impl Sink<GlobalCommunication, Error = CommandOrError<Block::Hash, NumberFor<Block>>>,
)
where
	BE: Backend<Block> + 'static,
	C: ClientForPbft<Block, BE> + 'static,
	N: NetworkT<Block>,
	NumberFor<Block>: BlockNumberOps,
{
	let is_voter = local_authority_id(voters, keystore).is_some();

	// verification stream
	let (global_in, global_out) =
		network.global_communication(communication::SetId(set_id), voters.clone(), is_voter);

	// block commit and catch up messages until relevant blocks are imported.
	let global_in = UntilGlobalMessageBlocksImported::new(
		client.import_notification_stream(),
		network.clone(),
		client.clone(),
		global_in,
		"global",
		metrics,
	);

	let global_in = global_in.map_err(CommandOrError::from);
	let global_out = global_out.sink_map_err(CommandOrError::from);

	(global_in, global_out)
}

struct Metrics {
	environment: environment::Metrics,
	// until_imported: until_imported::Metrics,
}

impl Metrics {
	fn register(registry: &Registry) -> Result<Self, PrometheusError> {
		Ok(Metrics {
			environment: environment::Metrics::register(registry)?,
			// until_imported: until_imported::Metrics::register(registry)?,
		})
	}
}

/// Futures that powers the voter.
#[must_use]
struct VoterWork<B, Block: BlockT, C, N: NetworkT<Block>, SC> {
	voter: Pin<
		Box<dyn Future<Output = Result<(), CommandOrError<Block::Hash, NumberFor<Block>>>> + Send>,
	>,
	shared_voter_state: SharedVoterState<Block>,
	env: Arc<Environment<B, Block, C, N, SC>>,
	voter_commands_rx: TracingUnboundedReceiver<VoterCommand<Block::Hash, NumberFor<Block>>>,
	network: NetworkBridge<Block, N>,
	telemetry: Option<TelemetryHandle>,
	/// Prometheus metrics.
	metrics: Option<Metrics>,
}

impl<B, Block, C, N, SC> VoterWork<B, Block, C, N, SC>
where
	Block: BlockT,
	B: Backend<Block> + 'static,
	C: ClientForPbft<Block, B> + 'static,
	C::Api: PbftApi<Block>,
	N: NetworkT<Block> + Sync,
	NumberFor<Block>: BlockNumberOps,
	SC: SelectChain<Block> + 'static,
{
	fn new(
		client: Arc<C>,
		config: Config,
		network: NetworkBridge<Block, N>,
		select_chain: SC,
		persistent_data: PersistentData<Block>,
		voter_commands_rx: TracingUnboundedReceiver<VoterCommand<Block::Hash, NumberFor<Block>>>,
		prometheus_registry: Option<prometheus_endpoint::Registry>,
		shared_voter_state: SharedVoterState<Block>,
		// justification_sender: GrandpaJustificationSender<Block>,
		telemetry: Option<TelemetryHandle>,
	) -> Self {
		let metrics = match prometheus_registry.as_ref().map(Metrics::register) {
			Some(Ok(metrics)) => Some(metrics),
			Some(Err(e)) => {
				debug!(target: "afg", "Failed to register metrics: {:?}", e);
				None
			},
			None => None,
		};

		let voters = persistent_data.authority_set.current_authorities();
		let env = Arc::new(Environment {
			client,
			select_chain,
			voters: Arc::new(voters),
			config,
			network: network.clone(),
			set_id: persistent_data.authority_set.set_id(),
			authority_set: persistent_data.authority_set.clone(),
			voter_set_state: persistent_data.set_state,
			metrics: metrics.as_ref().map(|m| m.environment.clone()),
			// justification_sender: Some(justification_sender),
			telemetry: telemetry.clone(),
			_phantom: PhantomData,
		});

		let mut work = VoterWork {
			// `voter` is set to a temporary value and replaced below when
			// calling `rebuild_voter`.
			voter: Box::pin(future::pending()),
			shared_voter_state,
			env,
			voter_commands_rx,
			network,
			telemetry,
			metrics,
		};
		work.rebuild_voter();
		work
	}

	/// Rebuilds the `self.voter` field using the current authority set
	/// state. This method should be called when we know that the authority set
	/// has changed (e.g. as signalled by a voter command).
	fn rebuild_voter(&mut self) {
		debug!(target: "afg", "{}: Starting new voter with set ID {}", self.env.config.name(), self.env.set_id);

		let maybe_authority_id =
			local_authority_id(&self.env.voters, self.env.config.keystore.as_ref());
		let authority_id = maybe_authority_id.map_or("<unknown>".into(), |s| s.to_string());

		telemetry!(
			self.telemetry;
			CONSENSUS_DEBUG;
			"afg.starting_new_voter";
			"name" => ?self.env.config.name(),
			"set_id" => ?self.env.set_id,
			"authority_id" => authority_id,
		);

		let chain_info = self.env.client.info();

		let authorities = self.env.voters.iter().map(|(id, _)| id.to_string()).collect::<Vec<_>>();

		let authorities = serde_json::to_string(&authorities).expect(
			"authorities is always at least an empty vector; elements are always of type string; qed.",
		);

		telemetry!(
			self.telemetry;
			CONSENSUS_INFO;
			"afg.authority_set";
			"number" => ?chain_info.finalized_number,
			"hash" => ?chain_info.finalized_hash,
			"authority_id" => authority_id,
			"authority_set_id" => ?self.env.set_id,
			"authorities" => authorities,
		);

		match &*self.env.voter_set_state.read() {
			VoterSetState::Live { completed_views, .. } => {
				let last_finalized = (chain_info.finalized_hash, chain_info.finalized_number);

				let global_comms = global_communication(
					self.env.set_id,
					&self.env.voters,
					self.env.client.clone(),
					&self.env.network,
					self.env.config.keystore.as_ref(),
					self.metrics.as_ref().map(|m| m.until_imported.clone()),
				);

				let last_completed_view = completed_views.last();

				let voter = finality_grandpa::leader::voter::Voter::new(
					self.env.clone(),
					(*self.env.voters).clone(),
					global_comms,
					last_completed_view.number,
					last_completed_view.votes.clone(),
					last_completed_view.base,
					last_finalized,
				);

				// Repoint shared_voter_state so that the RPC endpoint can query the state
				if self.shared_voter_state.reset(voter.voter_state()).is_none() {
					info!(target: "afg",
						"Timed out trying to update shared GRANDPA voter state. \
						RPC endpoints may return stale data."
					);
				}

				self.voter = Box::pin(voter);
			},
			VoterSetState::Paused { .. } => self.voter = Box::pin(future::pending()),
		};
	}

	fn handle_voter_command(
		&mut self,
		command: VoterCommand<Block::Hash, NumberFor<Block>>,
	) -> Result<(), Error> {
		match command {
			VoterCommand::ChangeAuthorities(new) => {
				let voters: Vec<String> =
					new.authorities.iter().map(move |(a, _)| format!("{}", a)).collect();
				telemetry!(
					self.telemetry;
					CONSENSUS_INFO;
					"afg.voter_command_change_authorities";
					"number" => ?new.canon_number,
					"hash" => ?new.canon_hash,
					"voters" => ?voters,
					"set_id" => ?new.set_id,
				);

				self.env.update_voter_set_state(|_| {
					// start the new authority set using the block where the
					// set changed (not where the signal happened!) as the base.
					let set_state = VoterSetState::live(
						new.set_id,
						&*self.env.authority_set.inner(),
						(new.canon_hash, new.canon_number),
					);

					aux_schema::write_voter_set_state(&*self.env.client, &set_state)?;
					Ok(Some(set_state))
				})?;

				let voters = Arc::new(VoterSet::new(new.authorities.into_iter()).expect(
					"new authorities come from pending change; pending change comes from \
					 `AuthoritySet`; `AuthoritySet` validates authorities is non-empty and \
					 weights are non-zero; qed.",
				));

				self.env = Arc::new(Environment {
					voters,
					set_id: new.set_id,
					voter_set_state: self.env.voter_set_state.clone(),
					client: self.env.client.clone(),
					select_chain: self.env.select_chain.clone(),
					config: self.env.config.clone(),
					authority_set: self.env.authority_set.clone(),
					network: self.env.network.clone(),
					metrics: self.env.metrics.clone(),
					// justification_sender: self.env.justification_sender.clone(),
					telemetry: self.telemetry.clone(),
					_phantom: PhantomData,
				});

				self.rebuild_voter();
				Ok(())
			},
			VoterCommand::Pause(reason) => {
				info!(target: "afg", "Pausing old validator set: {}", reason);

				// not racing because old voter is shut down.
				self.env.update_voter_set_state(|voter_set_state| {
					let completed_views = voter_set_state.completed_views();
					let set_state = VoterSetState::Paused { completed_views };

					aux_schema::write_voter_set_state(&*self.env.client, &set_state)?;
					Ok(Some(set_state))
				})?;

				self.rebuild_voter();
				Ok(())
			},
		}
	}
}

/// A trait that includes all the client functionalities grandpa requires.
/// Ideally this would be a trait alias, we're not there yet.
/// tracking issue <https://github.com/rust-lang/rust/issues/41517>
pub trait ClientForPbft<Block, BE>:
	LockImportRun<Block, BE>
	+ Finalizer<Block, BE>
	+ AuxStore
	+ HeaderMetadata<Block, Error = sp_blockchain::Error>
	+ HeaderBackend<Block>
	+ BlockchainEvents<Block>
	+ ProvideRuntimeApi<Block>
	+ ExecutorProvider<Block>
	+ BlockImport<Block, Transaction = TransactionFor<BE, Block>, Error = sp_consensus::Error>
	+ StorageProvider<Block, BE>
where
	BE: Backend<Block>,
	Block: BlockT,
{
}

impl<Block, BE, T> ClientForPbft<Block, BE> for T
where
	Block: BlockT,
	BE: Backend<Block>,
	T: LockImportRun<Block, BE>
		+ Finalizer<Block, BE>
		+ AuxStore
		+ HeaderMetadata<Block, Error = sp_blockchain::Error>
		+ HeaderBackend<Block>
		+ BlockchainEvents<Block>
		+ ProvideRuntimeApi<Block>
		+ ExecutorProvider<Block>
		+ BlockImport<Block, Transaction = TransactionFor<BE, Block>, Error = sp_consensus::Error>
		+ StorageProvider<Block, BE>,
{
}

/// Configuration for the PBFT service.
#[derive(Clone)]
pub struct Config {
	/// The expected duration for a message to be gossiped across the network.
	pub gossip_duration: Duration,
	/// The role of the local node (i.e. authority, full-node or light).
	pub local_role: sc_network::config::Role,
	/// Some local identifier of the voter.
	pub name: Option<String>,
	/// The keystore that manages the keys of this node.
	pub keystore: Option<SyncCryptoStorePtr>,
	/// TelemetryHandle instance.
	pub telemetry: Option<TelemetryHandle>,
	/// Chain specific PBFT protocol name. See [`crate::protocol_standard_name`].
	pub protocol_name: std::borrow::Cow<'static, str>,
}

impl Config {
	fn name(&self) -> &str {
		self.name.as_deref().unwrap_or("<unknown>")
	}
}

pub type SignedMessage<Block> = leader::SignedMessage<
	NumberFor<Block>,
	<Block as BlockT>::Hash,
	AuthoritySignature,
	AuthorityId,
>;

/// Shared voter state for querying.
pub struct SharedVoterState<Block: BlockT> {
	inner: Arc<
		RwLock<
			Option<
				Box<
					dyn leader::voter::VoterState<<Block as BlockT>::Hash, AuthorityId>
						+ Sync
						+ Send,
				>,
			>,
		>,
	>,
}

/// A new authority set along with the canonical block it changed at.
#[derive(Debug)]
pub(crate) struct NewAuthoritySet<H, N> {
	pub(crate) canon_number: N,
	pub(crate) canon_hash: H,
	pub(crate) set_id: SetId,
	pub(crate) authorities: AuthorityList,
}

/// Commands issued to the voter.
#[derive(Debug)]
pub(crate) enum VoterCommand<H, N> {
	/// Pause the voter for given reason.
	Pause(String),
	/// New authorities.
	ChangeAuthorities(NewAuthoritySet<H, N>),
}

/// Signals either an early exit of a voter or an error.
#[derive(Debug)]
pub(crate) enum CommandOrError<H, N> {
	/// An error occurred.
	Error(Error),
	/// A command to the voter.
	VoterCommand(VoterCommand<H, N>),
}

impl<H, N> From<Error> for CommandOrError<H, N> {
	fn from(e: Error) -> Self {
		CommandOrError::Error(e)
	}
}

impl<H, N> From<ClientError> for CommandOrError<H, N> {
	fn from(e: ClientError) -> Self {
		CommandOrError::Error(Error::Client(e))
	}
}

impl<H, N> From<finality_grandpa::leader::Error> for CommandOrError<H, N> {
	fn from(e: finality_grandpa::leader::Error) -> Self {
		CommandOrError::Error(Error::from(e))
	}
}

impl<H, N> From<VoterCommand<H, N>> for CommandOrError<H, N> {
	fn from(e: VoterCommand<H, N>) -> Self {
		CommandOrError::VoterCommand(e)
	}
}

use std::fmt;
impl<H: fmt::Debug, N: fmt::Debug> ::std::error::Error for CommandOrError<H, N> {}

impl<H, N> fmt::Display for CommandOrError<H, N> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match *self {
			CommandOrError::Error(ref e) => write!(f, "{}", e),
			CommandOrError::VoterCommand(ref cmd) => write!(f, "{}", cmd),
		}
	}
}

/// Errors that can occur while voting in PBFT.
#[derive(Debug, thiserror::Error)]
pub enum Error {
	/// An error within pbft.
	#[error("pbft error: {0}")]
	Pbft(PbftError),

	/// A network error.
	#[error("network error: {0}")]
	Network(String),

	/// A blockchain error.
	#[error("blockchain error: {0}")]
	Blockchain(String),

	/// Could not complete a round on disk.
	#[error("could not complete a round on disk: {0}")]
	Client(ClientError),

	/// Could not sign outgoing message
	#[error("could not sign outgoing message: {0}")]
	Signing(String),

	/// An invariant has been violated (e.g. not finalizing pending change blocks in-order)
	#[error("safety invariant has been violated: {0}")]
	Safety(String),

	/// A timer failed to fire.
	#[error("a timer failed to fire: {0}")]
	Timer(std::io::Error),

	/// A runtime api request failed.
	#[error("runtime API request failed: {0}")]
	RuntimeApi(sp_api::ApiError),
}

impl From<PbftError> for Error {
	fn from(e: PbftError) -> Self {
		Error::Pbft(e)
	}
}

impl From<ClientError> for Error {
	fn from(e: ClientError) -> Self {
		Error::Client(e)
	}
}

/// Something which can determine if a block is known.
pub(crate) trait BlockStatus<Block: BlockT> {
	/// Return `Ok(Some(number))` or `Ok(None)` depending on whether the block
	/// is definitely known and has been imported.
	/// If an unexpected error occurs, return that.
	fn block_number(&self, hash: Block::Hash) -> Result<Option<NumberFor<Block>>, Error>;
}

impl<Block: BlockT, Client> BlockStatus<Block> for Arc<Client>
where
	Client: HeaderBackend<Block>,
	NumberFor<Block>: BlockNumberOps,
{
	fn block_number(&self, hash: Block::Hash) -> Result<Option<NumberFor<Block>>, Error> {
		self.block_number_from_id(&BlockId::Hash(hash))
			.map_err(|e| Error::Blockchain(e.to_string()))
	}
}

/// Something that one can ask to do a block sync request.
pub(crate) trait BlockSyncRequester<Block: BlockT> {
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

impl<Block, Network> BlockSyncRequester<Block> for NetworkBridge<Block, Network>
where
	Block: BlockT,
	Network: NetworkT<Block>,
{
	fn set_sync_fork_request(
		&self,
		peers: Vec<sc_network::PeerId>,
		hash: Block::Hash,
		number: NumberFor<Block>,
	) {
		NetworkBridge::set_sync_fork_request(self, peers, hash, number)
	}
}

/// Checks if this node has any available keys in the keystore for any authority id in the given
/// voter set.  Returns the authority id for which keys are available, or `None` if no keys are
/// available.
fn local_authority_id(
	voters: &VoterSet<AuthorityId>,
	keystore: Option<&SyncCryptoStorePtr>,
) -> Option<AuthorityId> {
	keystore.and_then(|keystore| {
		voters
			.iter()
			.find(|(p, _)| {
				SyncCryptoStore::has_keys(&**keystore, &[(p.to_raw_vec(), AuthorityId::ID)])
			})
			.map(|(p, _)| p.clone())
	})
}
