use fork_tree::ForkTree;
use parity_scale_codec::{Decode, Encode};
use sc_consensus::shared_data::SharedData;
use sp_finality_pbft::AuthorityList;

/// A shared authority set.
#[derive(Clone)]
pub struct SharedAuthoritySet<H, N> {
	inner: SharedData<AuthoritySet<H, N>>,
}

/// Tracks historical authority set changes. We store the block numbers for the last block
/// of each authority set, once they have been finalized. These blocks are guaranteed to
/// have a justification unless they were triggered by a forced change.
#[derive(Debug, Encode, Decode, Clone, PartialEq)]
pub struct AuthoritySetChanges<N>(Vec<(u64, N)>);

/// A set of authorities.
#[derive(Debug, Clone, Encode, Decode, PartialEq)]
pub struct AuthoritySet<H, N> {
	/// The current active authorities.
	pub(crate) current_authorities: AuthorityList,
	/// The current set id.
	pub(crate) set_id: u64,
	/// Tree of pending standard changes across forks. Standard changes are
	/// enacted on finality and must be enacted (i.e. finalized) in-order across
	/// a given branch
	pub(crate) pending_standard_changes: ForkTree<H, N, PendingChange<H, N>>,
	/// Pending forced changes across different forks (at most one per fork).
	/// Forced changes are enacted on block depth (not finality), for this
	/// reason only one forced change should exist per fork. When trying to
	/// apply forced changes we keep track of any pending standard changes that
	/// they may depend on, this is done by making sure that any pending change
	/// that is an ancestor of the forced changed and its effective block number
	/// is lower than the last finalized block (as signaled in the forced
	/// change) must be applied beforehand.
	pending_forced_changes: Vec<PendingChange<H, N>>,
	/// Track at which blocks the set id changed. This is useful when we need to prove finality
	/// for a given block since we can figure out what set the block belongs to and when the
	/// set started/ended.
	pub(crate) authority_set_changes: AuthoritySetChanges<N>,
}
/// Kinds of delays for pending changes.
#[derive(Debug, Clone, Encode, Decode, PartialEq)]
pub enum DelayKind<N> {
	/// Depth in finalized chain.
	Finalized,
	/// Depth in best chain. The median last finalized block is calculated at the time the
	/// change was signaled.
	Best { median_last_finalized: N },
}

/// A pending change to the authority set.
///
/// This will be applied when the announcing block is at some depth within
/// the finalized or unfinalized chain.
#[derive(Debug, Clone, Encode, PartialEq)]
pub struct PendingChange<H, N> {
	/// The new authorities and weights to apply.
	pub(crate) next_authorities: AuthorityList,
	/// How deep in the chain the announcing block must be
	/// before the change is applied.
	pub(crate) delay: N,
	/// The announcing block's height.
	pub(crate) canon_height: N,
	/// The announcing block's hash.
	pub(crate) canon_hash: H,
	/// The delay kind.
	pub(crate) delay_kind: DelayKind<N>,
}
