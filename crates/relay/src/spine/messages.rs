use alloy_primitives::B256;
use flux_utils::ArrayStr;
use helix_common::SubmissionTrace;
// Re-export as also used as spine message.
pub use helix_common::api::builder_api::TopBidUpdate;
use helix_tcp_types::Status;
use helix_types::BlsPublicKeyBytes;
use http::StatusCode;
use tracing::error;

use crate::{
    api::builder::error::BuilderApiError,
    auctioneer::{InternalBidSubmissionHeader, SubmissionRef},
};

#[derive(Debug, Clone, Copy)]
pub struct NewBidSubmission {
    pub payload_offset: usize,
    pub submission_ref: SubmissionRef,
    pub header: InternalBidSubmissionHeader,
    pub trace: SubmissionTrace,
    pub expected_pubkey: Option<BlsPublicKeyBytes>,
    pub http_submission_ix: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct SubmissionResultWithRef {
    pub sub_ref: SubmissionRef,
    pub tcp_status: Status,
    pub http_status: StatusCode,
    pub error_msg: ArrayStr<256>,
    pub should_report: bool,
}

impl SubmissionResultWithRef {
    pub fn new(sub_ref: SubmissionRef, result: Result<(), BuilderApiError>) -> Self {
        if let Err(e) = &result {
            if e.should_report() {
                let status = "error";
                let (error_type, error, error_message) = match e {
                    BuilderApiError::BidValidation(val_err) => (
                        "validation",
                        val_err.error_variant(),
                        val_err.error_details(),
                    ),
                    BuilderApiError::BlockSimulation(sim_err) => (
                        "simulation",
                        sim_err.error_variant(),
                        sim_err.error_details(),
                    ),
                    _ => ("other", "", String::new()),
                };
                error!(status, error_type, error, error_message);
            }
        }
        match result {
            Ok(()) => Self {
                sub_ref,
                tcp_status: Status::Okay,
                http_status: StatusCode::OK,
                error_msg: ArrayStr::default(),
                should_report: false,
            },
            Err(ref e) => {
                let tcp_status = match e {
                    BuilderApiError::DatabaseError(_) | BuilderApiError::InternalError => {
                        Status::InternalError
                    }
                    _ => Status::InvalidRequest,
                };
                Self {
                    sub_ref,
                    tcp_status,
                    http_status: e.http_status(),
                    error_msg: ArrayStr::from_str_truncate(&e.to_string()),
                    should_report: e.should_report(),
                }
            }
        }
    }
}

// references position in SharedVector<BidSubmission>
#[derive(Debug, Clone, Copy)]
pub struct DecodedSubmission {
    pub ix: usize,
}

/// Auctioneer → SimulatorTile: spine signal for a new sim/merge request or slot transition.
#[derive(Debug, Clone, Copy)]
pub struct ToSimMsg {
    pub kind: ToSimKind,
    /// Index into `SharedVector<SimInboundPayload>` (unused for `NewSlot`).
    pub ix: usize,
    /// Slot number; only meaningful for `NewSlot`.
    pub bid_slot: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToSimKind {
    /// SimRequest or MergeRequest stored at `ix`.
    Request,
    NewSlot,
}

/// SimulatorTile → Auctioneer: index into `SharedVector<SimOutboundPayload>`.
#[derive(Debug, Clone, Copy)]
pub struct FromSimMsg {
    pub ix: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum BidEvent {
    Live,
}

#[derive(Debug, Clone, Copy)]
pub struct BidUpdate {
    pub block_hash: B256,
    pub event: BidEvent,
}
