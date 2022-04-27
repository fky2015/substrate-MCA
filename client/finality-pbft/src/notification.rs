use sc_utils::notification::{NotificationSender, NotificationStream, TracingKeyStr};

use crate::justification::PbftJustification;

/// The sending half of the Pbft justification channel(s).
///
/// Used to send notifications about justifications generated
/// at the end of a Pbft round.
pub type PbftJustificationSender<Block> = NotificationSender<PbftJustification<Block>>;

/// The receiving half of the Pbft justification channel.
///
/// Used to receive notifications about justifications generated
/// at the end of a Pbft round.
/// The `PbftJustificationStream` entity stores the `SharedJustificationSenders`
/// so it can be used to add more subscriptions.
pub type PbftJustificationStream<Block> =
	NotificationStream<PbftJustification<Block>, PbftJustificationsTracingKey>;

/// Provides tracing key for GRANDPA justifications stream.
#[derive(Clone)]
pub struct PbftJustificationsTracingKey;
impl TracingKeyStr for PbftJustificationsTracingKey {
	const TRACING_KEY: &'static str = "mpsc_pbft_justification_notification_stream";
}
