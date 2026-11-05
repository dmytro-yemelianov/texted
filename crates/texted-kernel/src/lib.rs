//! QNX-inspired In-Process Microkernel IPC Bus
//!
//! Provides two fundamental QNX primitives:
//! 1. **Synchronous Request-Reply Rendezvous (`MsgSend` / `MsgReceive` / `MsgReply`)**:
//!    The sender blocks until the receiving server processes the request and sends back a reply.
//! 2. **Asynchronous Pulses (`MsgSendPulse`)**:
//!    Non-blocking, fixed-size signals (priority, 8-bit code, 64-bit value) for high-priority notifications.

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub type ChannelId = u16;
pub type ConnectionId = u16;

pub const CH_BUFFER: ChannelId = 1;
pub const CH_LAYOUT: ChannelId = 2;
pub const CH_RENDER: ChannelId = 3;
pub const CH_INPUT: ChannelId = 4;
pub const CH_IO: ChannelId = 5;

/// Non-blocking, fixed-size Pulse (analogous to QNX `pulse_t`)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Pulse {
    pub priority: u8,
    pub code: i8,
    pub value: u64,
}

impl Pulse {
    pub fn new(priority: u8, code: i8, value: u64) -> Self {
        Self {
            priority,
            code,
            value,
        }
    }
}

// Well-known pulse codes
pub const PULSE_SYS_FRAME_TICK: i8 = -1;
pub const PULSE_SYS_DIRTY_RECT: i8 = -2;
pub const PULSE_KEY_INPUT: i8 = 10;
pub const PULSE_BUFFER_DIRTY: i8 = 20;

/// Synchronous Rendezvous Request-Reply envelope
pub struct Request<Req, Resp> {
    pub client_coid: ConnectionId,
    pub tx_id: u64,
    pub payload: Req,
    reply_sender: crossbeam_channel::Sender<Resp>,
}

impl<Req, Resp> Request<Req, Resp> {
    /// Server replies to the waiting client, unblocking it.
    pub fn reply(self, response: Resp) {
        let _ = self.reply_sender.send(response);
    }
}

/// Incoming event for a server channel (either a high-priority pulse or a synchronous request)
pub enum ChannelEvent<Req, Resp> {
    Pulse(Pulse),
    Message(Request<Req, Resp>),
}

/// Channel end used by a server to receive messages and pulses
pub struct ChannelServer<Req, Resp> {
    channel_id: ChannelId,
    pulse_rx: Receiver<Pulse>,
    msg_rx: Receiver<Request<Req, Resp>>,
}

impl<Req, Resp> ChannelServer<Req, Resp> {
    pub fn channel_id(&self) -> ChannelId {
        self.channel_id
    }

    /// Receives the next available event, prioritizing pulses over request messages.
    pub fn receive(&self) -> Result<ChannelEvent<Req, Resp>, crossbeam_channel::RecvError> {
        // Check pulses first without blocking
        if let Ok(pulse) = self.pulse_rx.try_recv() {
            return Ok(ChannelEvent::Pulse(pulse));
        }

        // Select between pulse and message
        crossbeam_channel::select! {
            recv(self.pulse_rx) -> pulse => {
                pulse.map(ChannelEvent::Pulse)
            }
            recv(self.msg_rx) -> msg => {
                msg.map(ChannelEvent::Message)
            }
        }
    }

    /// Try to receive without blocking
    pub fn try_receive(&self) -> Option<ChannelEvent<Req, Resp>> {
        if let Ok(pulse) = self.pulse_rx.try_recv() {
            Some(ChannelEvent::Pulse(pulse))
        } else if let Ok(msg) = self.msg_rx.try_recv() {
            Some(ChannelEvent::Message(msg))
        } else {
            None
        }
    }
}

/// Channel handle used by clients to send pulses and messages
#[derive(Clone)]
pub struct ChannelClient<Req, Resp> {
    client_coid: ConnectionId,
    target_chid: ChannelId,
    pulse_tx: Sender<Pulse>,
    msg_tx: Sender<Request<Req, Resp>>,
    next_tx: Arc<AtomicU64>,
}

impl<Req, Resp> ChannelClient<Req, Resp> {
    pub fn connection_id(&self) -> ConnectionId {
        self.client_coid
    }

    pub fn target_channel_id(&self) -> ChannelId {
        self.target_chid
    }

    /// Asynchronous Pulse: non-blocking signal (QNX MsgSendPulse)
    pub fn send_pulse(&self, pulse: Pulse) -> Result<(), crossbeam_channel::SendError<Pulse>> {
        self.pulse_tx.send(pulse)
    }

    /// Synchronous Rendezvous (QNX MsgSend):
    /// Sends `request` to the server and blocks until the server replies with `Resp`.
    pub fn send(&self, request: Req) -> Result<Resp, crossbeam_channel::RecvError> {
        let (reply_tx, reply_rx) = bounded(1);
        let tx_id = self.next_tx.fetch_add(1, Ordering::Relaxed);
        let envelope = Request {
            client_coid: self.client_coid,
            tx_id,
            payload: request,
            reply_sender: reply_tx,
        };

        if self.msg_tx.send(envelope).is_err() {
            return Err(crossbeam_channel::RecvError);
        }

        reply_rx.recv()
    }
}

/// Channel factory for typed messages
pub struct ChannelPair<Req, Resp> {
    pub server: ChannelServer<Req, Resp>,
    pulse_tx: Sender<Pulse>,
    msg_tx: Sender<Request<Req, Resp>>,
    next_coid: AtomicU64,
}

impl<Req, Resp> ChannelPair<Req, Resp> {
    pub fn new(channel_id: ChannelId) -> Self {
        let (pulse_tx, pulse_rx) = unbounded();
        let (msg_tx, msg_rx) = unbounded();
        Self {
            server: ChannelServer {
                channel_id,
                pulse_rx,
                msg_rx,
            },
            pulse_tx,
            msg_tx,
            next_coid: AtomicU64::new(1),
        }
    }

    /// Create a client connection to this channel
    pub fn connect(&self) -> ChannelClient<Req, Resp> {
        let coid = self.next_coid.fetch_add(1, Ordering::Relaxed) as ConnectionId;
        ChannelClient {
            client_coid: coid,
            target_chid: self.server.channel_id,
            pulse_tx: self.pulse_tx.clone(),
            msg_tx: self.msg_tx.clone(),
            next_tx: Arc::new(AtomicU64::new(1)),
        }
    }
}

/// The Microkernel Registry managing active channels and supervisor tracking
#[derive(Default)]
pub struct MicrokernelRegistry {
    channels: RwLock<HashMap<ChannelId, &'static str>>,
}

impl MicrokernelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_channel(&self, id: ChannelId, name: &'static str) {
        self.channels.write().insert(id, name);
    }

    pub fn channel_name(&self, id: ChannelId) -> Option<&'static str> {
        self.channels.read().get(&id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_sync_rendezvous_msg_send_reply() {
        let pair: ChannelPair<String, usize> = ChannelPair::new(CH_BUFFER);
        let client = pair.connect();
        let server = pair.server;

        let handle = thread::spawn(move || {
            match server.receive().expect("Failed to receive") {
                ChannelEvent::Message(req) => {
                    assert_eq!(req.payload, "hello microkernel");
                    let len = req.payload.len();
                    req.reply(len);
                }
                ChannelEvent::Pulse(_) => panic!("Expected message"),
            }
        });

        let response = client.send("hello microkernel".to_string()).unwrap();
        assert_eq!(response, 17);
        handle.join().unwrap();
    }

    #[test]
    fn test_pulse_priority_over_messages() {
        let pair: ChannelPair<u32, u32> = ChannelPair::new(CH_INPUT);
        let client = pair.connect();
        let server = pair.server;

        // Send a message first
        let client_clone = client.clone();
        let sender_thread = thread::spawn(move || {
            let res = client_clone.send(42).unwrap();
            assert_eq!(res, 84);
        });

        // Send a pulse immediately after
        let pulse = Pulse::new(0, PULSE_KEY_INPUT, 999);
        client.send_pulse(pulse).unwrap();

        // Server should receive the pulse first because pulses take precedence
        match server.receive().unwrap() {
            ChannelEvent::Pulse(p) => {
                assert_eq!(p.code, PULSE_KEY_INPUT);
                assert_eq!(p.value, 999);
            }
            ChannelEvent::Message(_) => panic!("Expected pulse priority"),
        }

        // Then server receives the message
        match server.receive().unwrap() {
            ChannelEvent::Message(req) => {
                assert_eq!(req.payload, 42);
                let val = req.payload;
                req.reply(val * 2);
            }
            ChannelEvent::Pulse(_) => panic!("Expected message"),
        }

        sender_thread.join().unwrap();
    }
}
