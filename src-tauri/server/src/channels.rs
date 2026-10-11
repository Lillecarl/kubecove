use kubecove_lib::runtime::{Channel, ChannelClosed};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    ops::Bound::{Excluded, Unbounded},
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

// Match src/features/resource-detail/log-helpers.ts's retained log line limit.
const RETAINED_MESSAGES: usize = 1_000;

#[derive(Default)]
struct PendingChannel {
    messages: VecDeque<Value>,
    dropped: usize,
}

#[cfg(test)]
#[path = "channels_tests.rs"]
mod tests;

#[derive(Default)]
struct Pending {
    channels: BTreeMap<u64, PendingChannel>,
    last_channel: Option<u64>,
    closed: bool,
}

#[derive(Default)]
pub struct ChannelHub {
    pending: Mutex<Pending>,
    pub ready: Notify,
}

impl ChannelHub {
    pub fn channel<T: Serialize + Send + 'static>(self: &Arc<Self>, id: u64) -> Channel<T> {
        self.pending
            .lock()
            .expect("channel hub lock")
            .channels
            .entry(id)
            .or_default();
        let hub = Arc::clone(self);
        Channel::new(move |message| {
            let message = serde_json::to_value(message).map_err(|_| ChannelClosed)?;
            let mut pending = hub.pending.lock().map_err(|_| ChannelClosed)?;
            if pending.closed {
                return Err(ChannelClosed);
            }
            let channel = pending.channels.get_mut(&id).ok_or(ChannelClosed)?;
            if channel.messages.len() == RETAINED_MESSAGES {
                channel.messages.pop_front();
                channel.dropped = channel.dropped.saturating_add(1);
            }
            channel.messages.push_back(message);
            drop(pending);
            hub.ready.notify_one();
            Ok(())
        })
    }

    pub fn pop(&self) -> Option<Value> {
        let mut pending = self.pending.lock().expect("channel hub lock");
        let has_output = |(_, channel): &(&u64, &PendingChannel)| {
            channel.dropped != 0 || !channel.messages.is_empty()
        };
        let next = pending.last_channel.and_then(|last| {
            pending
                .channels
                .range((Excluded(last), Unbounded))
                .find(has_output)
        });
        let id = *next.or_else(|| pending.channels.iter().find(has_output))?.0;
        pending.last_channel = Some(id);
        let channel = pending
            .channels
            .get_mut(&id)
            .expect("selected pending channel");
        if channel.dropped != 0 {
            let dropped = std::mem::take(&mut channel.dropped);
            return Some(json!({"type": "channel_gap", "channelId": id, "dropped": dropped}));
        }
        let message = channel.messages.pop_front()?;
        Some(json!({"type": "channel", "channelId": id, "message": message}))
    }

    pub fn remove(&self, id: u64) {
        self.pending
            .lock()
            .expect("channel hub lock")
            .channels
            .remove(&id);
    }

    pub fn close(&self) {
        let mut pending = self.pending.lock().expect("channel hub lock");
        pending.closed = true;
        pending.channels.clear();
        drop(pending);
        self.ready.notify_waiters();
    }
}
