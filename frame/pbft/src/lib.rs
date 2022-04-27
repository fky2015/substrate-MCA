#![cfg_attr(not(feature = "std"), no_std)]

pub use sp_finality_pbft as fp_primitives;


use sp_std::prelude::*;
use codec::{self as codec, Decode, Encode, MaxEncodedLen};
pub use fp_primitives::{AuthorityId, AuthorityList, AuthorityWeight, VersionedAuthorityList};
use fp_primitives::{
	ConsensusLog, EquivocationProof, ScheduledChange, SetId, GRANDPA_AUTHORITIES_KEY,
	GRANDPA_ENGINE_ID,
};

use frame_support::{
	dispatch::DispatchResultWithPostInfo,
	pallet_prelude::Get,
	storage,
	traits::{KeyOwnerProofSystem, OneSessionHandler, StorageVersion},
	weights::{Pays, Weight},
	WeakBoundedVec,
};
use sp_runtime::{generic::DigestItem, traits::Zero, DispatchResult, KeyTypeId};
use sp_session::{GetSessionNumber, GetValidatorCount};
use sp_staking::SessionIndex;

pub use pallet::*;

use scale_info::TypeInfo;

#[frame_support::pallet]
pub mod pallet {
    use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::storage_version(STORAGE_VERSION)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The event type of this module.
		type Event: From<Event>
			+ Into<<Self as frame_system::Config>::Event>
			+ IsType<<Self as frame_system::Config>::Event>;

		/// The function call.
		type Call: From<Call<Self>>;

		/// The proof of key ownership, used for validating equivocation reports
		/// The proof must include the session index and validator count of the
		/// session at which the equivocation occurred.
		type KeyOwnerProof: Parameter + GetSessionNumber + GetValidatorCount;

		/// The identification of a key owner, used when reporting equivocations.
		type KeyOwnerIdentification: Parameter;

		/// A system for proving ownership of keys, i.e. that a given key was part
		/// of a validator set, needed for validating equivocation reports.
		type KeyOwnerProofSystem: KeyOwnerProofSystem<
			(KeyTypeId, AuthorityId),
			Proof = Self::KeyOwnerProof,
			IdentificationTuple = Self::KeyOwnerIdentification,
		>;

		/// The equivocation handling subsystem, defines methods to report an
		/// offence (after the equivocation has been validated) and for submitting a
		/// transaction to report an equivocation (from an offchain context).
		/// NOTE: when enabling equivocation handling (i.e. this type isn't set to
		/// `()`) you must use this pallet's `ValidateUnsigned` in the runtime
		/// definition.
		type HandleEquivocation: HandleEquivocation<Self>;

		/// Weights for this pallet.
		type WeightInfo: WeightInfo;

		/// Max Authorities in use
		#[pallet::constant]
		type MaxAuthorities: Get<u32>;
	}
}
