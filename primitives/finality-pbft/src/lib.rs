#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(feature = "std")]
use serde::Serialize;

use codec::{Codec, Decode, Encode, Input};
use scale_info::TypeInfo;

#[cfg(feature = "std")]
use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
use sp_runtime::{traits::NumberFor, ConsensusEngineId, RuntimeDebug};
use sp_std::{borrow::Cow, vec::Vec};

#[cfg(feature = "std")]
use log::debug;

use grandpa::leader;

/// Key type for PBFT module
pub const KEY_TYPE: sp_core::crypto::KeyTypeId = sp_application_crypto::KeyTypeId(*b"pbft");

mod app {
	use sp_application_crypto::{app_crypto, ed25519};
	app_crypto!(ed25519, super::KEY_TYPE);
}

sp_application_crypto::with_pair! {
	/// The pbft crypto scheme defined via the keypair type.
	pub type AuthorityPair = app::Pair;
}

/// Identify of a PBFT authority.
pub type AuthorityId = app::Public;

/// Signature for a PBFT authority.
pub type AuthoritySignature = app::Signature;

/// The `ConsensusEngineId` of PBFT.
pub const PBFT_ENGINE_ID: ConsensusEngineId = *b"PBFT";

/// The storage key for the current set of weighted PBFT authorities.
/// The value stored is an encoded VersionedAuthorityList.
pub const PBFT_AUTHORITIES_KEY: &'static [u8] = b":pbft_authorities";

/// The index of an authority.
pub type AuthorityIndex = u64;

/// The monotonic identifier of a PBFT set of authorities.
pub type SetId = u64;

/// The view indicator.
pub type ViewNumber = u64;

/// A list of Grandpa authorities with associated weights.
pub type AuthorityList = Vec<AuthorityId>;

// TODO: A scheduled change of authority set.
// #[cfg_attr(feature = "std", derive(Serialize))]
// #[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug)]
// pub struct ScheduledChange<N> {
// 	/// The new authorities after the change, along with their respective weights.
// 	pub next_authorities: AuthorityList,
// 	/// The number of blocks to delay.
// 	pub delay: N,
// }

/// Encode round message localized to a given round and set id.
pub fn localized_payload<E: Encode>(view: u64, set_id: SetId, message: &E) -> Vec<u8> {
	let mut buf = Vec::new();
	localized_payload_with_buffer(view, set_id, message, &mut buf);
	buf
}

/// Encode round message localized to a given round and set id using the given
/// buffer. The given buffer will be cleared and the resulting encoded payload
/// will always be written to the start of the buffer.
pub fn localized_payload_with_buffer<E: Encode>(
	view: u64,
	set_id: SetId,
	message: &E,
	buf: &mut Vec<u8>,
) {
	buf.clear();
	(message, view, set_id).encode_to(buf)
}

/// Check a message signature by encoding the message as a localized payload and
/// verifying the provided signature using the expected authority id.
pub fn check_message_signature<H, N>(
	message: &leader::Message<H, N>,
	id: &AuthorityId,
	signature: &AuthoritySignature,
	view: u64,
	set_id: SetId,
) -> bool
where
	H: Encode,
	N: Encode,
{
	check_message_signature_with_buffer(message, id, signature, view, set_id, &mut Vec::new())
}

/// Check a message signature by encoding the message as a localized payload and
/// verifying the provided signature using the expected authority id.
/// The encoding necessary to verify the signature will be done using the given
/// buffer, the original content of the buffer will be cleared.
pub fn check_message_signature_with_buffer<H, N>(
	message: &leader::Message<H, N>,
	id: &AuthorityId,
	signature: &AuthoritySignature,
	view: u64,
	set_id: SetId,
	buf: &mut Vec<u8>,
) -> bool
where
	H: Encode,
	N: Encode,
{
	use sp_application_crypto::RuntimeAppPublic;

	localized_payload_with_buffer(view, set_id, message, buf);

	let valid = id.verify(&buf, signature);

	if !valid {
		#[cfg(feature = "std")]
		debug!(target: "afg", "Bad signature on message from {:?}", id);
	}

	valid
}

sp_api::decl_runtime_apis! {
	/// APIs for integrating the PBFT finality gadget into runtimes.
	/// This should be implemented on the runtime side.
	///
	/// This is primarily used for negotiating authority-set changes for the
	/// gadget. GRANDPA uses a signaling model of changing authority sets:
	/// changes should be signaled with a delay of N blocks, and then automatically
	/// applied in the runtime after those N blocks have passed.
	///
	/// The consensus protocol will coordinate the handoff externally.
	#[api_version(3)]
	pub trait PbftApi {
		/// Get the current GRANDPA authorities and weights. This should not change except
		/// for when changes are scheduled and the corresponding delay has passed.
		///
		/// When called at block B, it will return the set of authorities that should be
		/// used to finalize descendants of this block (B+1, B+2, ...). The block B itself
		/// is finalized by the authorities from block B-1.
		fn grandpa_authorities() -> AuthorityList;
		/// Get current GRANDPA authority set id.
		fn current_set_id() -> SetId;
	}
}
