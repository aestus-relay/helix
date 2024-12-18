pub mod broadcast_cancellation;
pub mod broadcast_get_payload;
pub mod broadcast_header;
pub mod broadcast_payload;

use broadcast_cancellation::BroadcastCancellationParams;
pub use broadcast_get_payload::*;
pub use broadcast_header::*;
pub use broadcast_payload::*;

#[derive(Debug, Clone)]
pub enum GossipedMessage {
    Header(Box<BroadcastHeaderParams>),
    Payload(Box<BroadcastPayloadParams>),
    GetPayload(Box<BroadcastGetPayloadParams>),
    Cancellation(Box<BroadcastCancellationParams>),
}
