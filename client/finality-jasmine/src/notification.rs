use sc_utils::notification::{NotificationSender, NotificationStream, TracingKeyStr};

use crate::justification::JasmineJustification;

/// The sending half of the Jasmine justification channel(s).
///
/// Used to send notifications about justifications generated
/// at the end of a Jasmine round.
pub type JasmineJustificationSender<Block> = NotificationSender<JasmineJustification<Block>>;

/// The receiving half of the Jasmine justification channel.
///
/// Used to receive notifications about justifications generated
/// at the end of a Jasmine round.
/// The `JasmineJustificationStream` entity stores the `SharedJustificationSenders`
/// so it can be used to add more subscriptions.
pub type JasmineJustificationStream<Block> =
	NotificationStream<JasmineJustification<Block>, JasmineJustificationsTracingKey>;

/// Provides tracing key for GRANDPA justifications stream.
#[derive(Clone)]
pub struct JasmineJustificationsTracingKey;
impl TracingKeyStr for JasmineJustificationsTracingKey {
	const TRACING_KEY: &'static str = "mpsc_jasmine_justification_notification_stream";
}
