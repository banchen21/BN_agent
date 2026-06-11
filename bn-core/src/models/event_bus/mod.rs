//! 事件总线 Actor — 基于 actix 的同步事件分发

use actix::prelude::*;
use plugin_core::AgentEvent;
use std::sync::Arc;

type EventCallback = Arc<dyn Fn(&AgentEvent) -> bool + Send + Sync>;
//                                              ^^^^
//  true  = 继续传播给下一个回调
//  false = 拦截，停止传播

pub struct EventBus {
    callbacks: Vec<EventCallback>,
}

impl EventBus {
    pub fn new() -> Self {
        Self { callbacks: Vec::new() }
    }
}

impl Actor for EventBus {
    type Context = Context<Self>;
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct RegisterCallback(pub EventCallback);

#[derive(Message)]
#[rtype(result = "()")]
pub struct EmitEvent(pub AgentEvent);

impl Handler<RegisterCallback> for EventBus {
    type Result = ();
    fn handle(&mut self, msg: RegisterCallback, _: &mut Self::Context) {
        self.callbacks.push(msg.0);
        tracing::info!("已注册回调 (共 {} 个)", self.callbacks.len());
    }
}

impl Handler<EmitEvent> for EventBus {
    type Result = ();
    fn handle(&mut self, msg: EmitEvent, _: &mut Self::Context) {
        for cb in &self.callbacks {
            if !cb(&msg.0) {
                // 回调返回 false → 拦截，停止传播
                break;
            }
        }
    }
}

/// 桥接插件系统的 EventEmitter 实现
pub struct BusEmitter {
    addr: Addr<EventBus>,
}

impl BusEmitter {
    pub fn new(addr: Addr<EventBus>) -> Self {
        Self { addr }
    }
}

impl plugin_core::EventEmitter for BusEmitter {
    fn emit(&self, event: AgentEvent) {
        self.addr.do_send(EmitEvent(event));
    }
}
