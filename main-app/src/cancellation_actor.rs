//! CancellationActor — tracks active LLM requests and supports cancellation.
//!
//! When a new user message arrives for the same `chat_id`, the previous
//! in-flight request can be cancelled via a oneshot channel.
//!
//! ## Messages
//!
//! - `RegisterRequest` — register a new active request with a cancel sender.
//! - `CancelChat` — cancel all active requests for a chat_id.
//! - `DeregisterRequest` — remove a completed/cancelled request.

use actix::prelude::*;
use plugin_interface::*;
use std::collections::HashMap;
use tokio::sync::oneshot;

// ── Messages ─────────────────────────────────────────────────────────────────

#[derive(Message)]
#[rtype(result = "()")]
pub struct RegisterRequest {
    pub chat_id: i64,
    pub request_id: String,
    pub cancel_tx: oneshot::Sender<()>,
}

#[derive(Message)]
#[rtype(result = "bool")]
pub struct CancelChat {
    pub chat_id: i64,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct DeregisterRequest {
    pub chat_id: i64,
    pub request_id: String,
}

#[derive(Message)]
#[rtype(result = "Vec<String>")]
pub struct ListActiveRequests;

// ── Actor ────────────────────────────────────────────────────────────────────

pub struct CancellationActor {
    /// chat_id → (request_id, cancel_tx)
    active: HashMap<i64, (String, oneshot::Sender<()>)>,
}

impl CancellationActor {
    pub fn new() -> Self {
        Self { active: HashMap::new() }
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
        // If there's already an active request for this chat_id, cancel it.
        if let Some((prev_id, cancel_tx)) = self.active.remove(&msg.chat_id) {
            log::info!("[Cancellation] replacing request {prev_id} with {} for chat_id={}", msg.request_id, msg.chat_id);
            let _ = cancel_tx.send(());
        }
        self.active.insert(msg.chat_id, (msg.request_id, msg.cancel_tx));
    }
}

impl Handler<CancelChat> for CancellationActor {
    type Result = bool;

    fn handle(&mut self, msg: CancelChat, _ctx: &mut Self::Context) -> bool {
        if let Some((request_id, cancel_tx)) = self.active.remove(&msg.chat_id) {
            log::info!("[Cancellation] cancelling {request_id} for chat_id={}", msg.chat_id);
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
        if let Some((rid, _)) = self.active.get(&msg.chat_id) {
            if *rid == msg.request_id {
                self.active.remove(&msg.chat_id);
                log::debug!("[Cancellation] deregistered {} for chat_id={}", msg.request_id, msg.chat_id);
            }
        }
    }
}

impl Handler<ListActiveRequests> for CancellationActor {
    type Result = MessageResult<ListActiveRequests>;

    fn handle(&mut self, _: ListActiveRequests, _ctx: &mut Self::Context) -> Self::Result {
        MessageResult(self.active.iter().map(|(chat_id, (rid, _))| {
            format!("chat_id={} request_id={}", chat_id, rid)
        }).collect())
    }
}
