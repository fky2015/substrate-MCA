#![cfg_attr(not(feature = "std"), no_std)]

pub use sp_finality_pbft as fp_primitives;

use codec::{self as codec, Decode, Encode, MaxEncodedLen};
pub use fp_primitives::{AuthorityId, AuthorityList};
use fp_primitives::{ConsensusLog, ScheduledChange, SetId, PBFT_AUTHORITIES_KEY, PBFT_ENGINE_ID};
use sp_std::prelude::*;

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

// pub use pallet::*;

use scale_info::TypeInfo;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		// /// The event type of this module.
		// type Event: From<Event>
		// 	+ Into<<Self as frame_system::Config>::Event>
		// 	+ IsType<<Self as frame_system::Config>::Event>;
		//
		// /// The function call.
		// type Call: From<Call<Self>>;

		/// Max Authorities in use
		#[pallet::constant]
		type MaxAuthorities: Get<u32>;
	}
}
