//! CancellationActor — tracks the active LLM request and supports cancellation.
//!
//! When a new user message arrives, the previous in-flight request can be
//! cancelled via a oneshot channel.
//!
//! ## Messages
//!
//! - `RegisterRequest` — register a new active request with a cancel sender.
//! - `CancelCurrent` — cancel the active request.
//! - `DeregisterRequest` — remove a completed/cancelled request.

use actix::prelude::*;
use plugin_interface::*;
use tokio::sync::oneshot;

// ── Messages ─────────────────────────────────────────────────────────────────

#[derive(Message)]
#[rtype(result = "()")]
pub struct RegisterRequest {
    pub request_id: String,
    pub cancel_tx: oneshot::Sender<()>,
}

#[derive(Message)]
#[rtype(result = "bool")]
pub struct CancelCurrent;

#[derive(Message)]
#[rtype(result = "()")]
pub struct DeregisterRequest {
    pub request_id: String,
}

// ── Actor ────────────────────────────────────────────────────────────────────

pub struct CancellationActor {
    /// (request_id, cancel_tx)
    active: Option<(String, oneshot::Sender<()>)>,
}

impl CancellationActor {
    pub fn new() -> Self {
        Self { active: None }
    }
}

impl Actor for CancellationActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        log::info!("[CancellationActor] started");
    }
}

impl Handler<RegisterRequest> for CancellationActor {
    type Result = ();

    fn handle(&mut self, msg: RegisterRequest, _ctx: &mut Self::Context) {
        // If there's already an active request, cancel it.
        if let Some((prev_id, cancel_tx)) = self.active.take() {
            log::info!(
                "[Cancellation] replacing request {prev_id} with {}",
                msg.request_id
            );
            let _ = cancel_tx.send(());
        }
        self.active = Some((msg.request_id, msg.cancel_tx));
    }
}

impl Handler<CancelCurrent> for CancellationActor {
    type Result = bool;

    fn handle(&mut self, _msg: CancelCurrent, _ctx: &mut Self::Context) -> bool {
        if let Some((request_id, cancel_tx)) = self.active.take() {
            log::info!("[Cancellation] cancelling {request_id}");
            let _ = cancel_tx.send(());
            true
        } else {
            false
        }
    }
}

impl Handler<DeregisterRequest> for CancellationActor {
    type Result = ();

    fn handle(&mut self, msg: DeregisterRequest, _ctx: &mut Self::Context) {
        // Only remove if it's still the same request_id (not replaced by a newer one).
        if let Some((rid, _)) = &self.active {
            if *rid == msg.request_id {
                self.active = None;
                log::debug!("[Cancellation] deregistered {}", msg.request_id);
            }
        }
    }
}
