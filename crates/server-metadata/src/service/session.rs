//! Bounded connection observers and activity policy, without a transport/runtime dependency.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, Weak};

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::protocol::session::{EventsRequest, Heartbeat, SessionEventKind};

const PRESENCE_MS: i64 = 180_000;
const MAX_PENDING_EVENTS: usize = 64;
const MAX_PENDING_BYTES: usize = 1024 * 1024;

/// Safe subscription validation or delivery failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// Invalid heartbeat or subscription parameters.
    #[error("invalid session parameters")]
    Invalid,
    /// The requested producer has not been implemented.
    #[error("unsupported session event")]
    Unsupported,
    /// Delivery closed or exhausted its bounded pending queue.
    #[error("session event delivery closed")]
    Closed,
}

/// Transport-owned bounded delivery callback. Payload includes its subscription identity.
pub type EventSink = Arc<dyn Fn(SessionEventKind, Value) -> Result<(), SessionError> + Send + Sync>;

#[derive(Debug, Default)]
struct Presence {
    activity_ms: Option<i64>,
    visible: bool,
    focused_agent: Option<String>,
}

#[derive(Default)]
struct State {
    connections: BTreeMap<String, Weak<Mutex<Presence>>>,
    listeners: Vec<Weak<Listener>>,
}

/// Shared in-memory events and presence. Durable business state stays with its owning capability.
#[derive(Clone, Default)]
pub struct SessionEvents(Arc<Mutex<State>>);

impl fmt::Debug for SessionEvents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionEvents { .. }")
    }
}

/// Presence lifetime owned by one authenticated connection.
#[derive(Debug)]
pub struct SessionConnection {
    id: String,
    presence: Arc<Mutex<Presence>>,
    events: SessionEvents,
}

struct Listener {
    id: String,
    connection: String,
    presence: Weak<Mutex<Presence>>,
    kinds: BTreeSet<SessionEventKind>,
    notifications: bool,
    sink: EventSink,
    delivery: Mutex<Delivery>,
}

#[derive(Default)]
struct Delivery {
    active: bool,
    closed: bool,
    bytes: usize,
    pending: VecDeque<(SessionEventKind, Value)>,
}

/// An observer starts paused; activate only after its RPC response enters the outbound queue.
pub struct SessionSubscription(Arc<Listener>);

impl fmt::Debug for SessionSubscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SessionSubscription")
            .field(&self.0.id)
            .finish()
    }
}

impl SessionEvents {
    /// Register presence for a new connection. Dropping it releases focus suppression.
    #[must_use]
    pub fn connect(&self) -> SessionConnection {
        let id = Uuid::new_v4().to_string();
        let presence = Arc::new(Mutex::new(Presence::default()));
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .connections
            .retain(|_, value| value.strong_count() != 0);
        state
            .connections
            .insert(id.clone(), Arc::downgrade(&presence));
        SessionConnection {
            id,
            presence,
            events: self.clone(),
        }
    }

    /// Publish after the owning service commits a change. Slow/closed observers cannot fail it.
    /// For attention, `payload.agentId` selects the focus target; notification is elected once
    /// across present connections and duplicate subscriptions. Every observer still gets state.
    pub fn publish(&self, kind: SessionEventKind, payload: &Value) {
        self.publish_at(kind, payload, Utc::now().timestamp_millis());
    }

    fn publish_at(&self, kind: SessionEventKind, payload: &Value, now: i64) {
        if !payload.is_object() {
            return;
        }
        let (listeners, presence) = {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state
                .listeners
                .retain(|listener| listener.strong_count() != 0);
            state
                .connections
                .retain(|_, presence| presence.strong_count() != 0);
            let listeners: Vec<_> = state
                .listeners
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|listener| {
                    listener.kinds.contains(&kind) && listener.presence.strong_count() != 0
                })
                .collect();
            let presence: Vec<_> = state
                .connections
                .iter()
                .filter(|_| kind == SessionEventKind::AgentAttention)
                .filter_map(|(id, value)| value.upgrade().map(|value| (id.clone(), value)))
                .collect();
            (listeners, presence)
        };
        let mut recipient = None;
        let mut latest = i64::MIN;
        let mut focused = false;
        if kind == SessionEventKind::AgentAttention {
            let candidates: BTreeSet<_> = listeners
                .iter()
                .filter(|listener| listener.notifications)
                .map(|listener| listener.connection.as_str())
                .collect();
            for (id, presence) in presence {
                let presence = presence
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let Some(activity) = presence.activity_ms else {
                    continue;
                };
                if now.saturating_sub(activity) > PRESENCE_MS {
                    continue;
                }
                focused |= presence.visible
                    && presence.focused_agent.is_some()
                    && presence.focused_agent.as_deref() == payload["agentId"].as_str();
                if activity > latest && candidates.contains(id.as_str()) {
                    latest = activity;
                    recipient = Some(id);
                }
            }
        }
        let mut notified = false;
        for listener in listeners {
            let mut value = payload.clone();
            value["subscriptionId"] = Value::String(listener.id.clone());
            if kind == SessionEventKind::AgentAttention {
                let should_notify = !focused
                    && !notified
                    && listener.notifications
                    && recipient.as_deref() == Some(listener.connection.as_str());
                value["shouldNotify"] = Value::Bool(should_notify);
                notified |= should_notify;
            }
            if listener.deliver(kind, value).is_err() {
                tracing::debug!("session event observer closed");
            }
        }
    }
}

impl SessionConnection {
    /// Return this connection's opaque identity.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Validate and replace activity. Future timestamps are clamped at receipt time.
    ///
    /// # Errors
    /// Rejects invalid timestamps or oversized/control-containing focus identities.
    pub fn heartbeat(&self, heartbeat: Heartbeat) -> Result<(), SessionError> {
        let activity = parse_time(&heartbeat.last_activity_at)?;
        if let Some(timestamp) = &heartbeat.app_visibility_changed_at {
            parse_time(timestamp)?;
        }
        for id in [&heartbeat.focused_agent_id, &heartbeat.focused_terminal_id]
            .into_iter()
            .flatten()
        {
            if id.len() > 128 || id.chars().any(char::is_control) {
                return Err(SessionError::Invalid);
            }
        }
        let mut presence = self.presence.lock().map_err(|_| SessionError::Closed)?;
        presence.activity_ms = Some(activity.min(Utc::now().timestamp_millis()));
        presence.visible = heartbeat.app_visible;
        presence.focused_agent = heartbeat.focused_agent_id.filter(|id| !id.is_empty());
        Ok(())
    }

    /// Prepare an independently owned subscription. The API applies its shared connection budget.
    ///
    /// # Errors
    /// Rejects oversized requests, unknown categories, or unavailable event producers.
    pub fn subscribe(
        &self,
        request: EventsRequest,
        sink: EventSink,
    ) -> Result<SessionSubscription, SessionError> {
        if request.events.len() > 32 {
            return Err(SessionError::Invalid);
        }
        let kinds = request
            .events
            .into_iter()
            .map(|event| {
                serde_json::from_value(Value::String(event)).map_err(|_| SessionError::Unsupported)
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let listener = Arc::new(Listener {
            id: Uuid::new_v4().to_string(),
            connection: self.id.clone(),
            presence: Arc::downgrade(&self.presence),
            kinds,
            notifications: request.notifications,
            sink,
            delivery: Mutex::new(Delivery::default()),
        });
        let mut state = self.events.0.lock().map_err(|_| SessionError::Closed)?;
        state
            .listeners
            .retain(|listener| listener.strong_count() != 0);
        state.listeners.push(Arc::downgrade(&listener));
        Ok(SessionSubscription(listener))
    }
}

impl Listener {
    fn deliver(&self, kind: SessionEventKind, payload: Value) -> Result<(), SessionError> {
        let mut delivery = self.delivery.lock().map_err(|_| SessionError::Closed)?;
        if delivery.closed {
            return Err(SessionError::Closed);
        }
        if delivery.active {
            let result = (self.sink)(kind, payload);
            delivery.closed = result.is_err();
            return result;
        }
        let bytes = serde_json::to_vec(&payload)
            .map_err(|_| SessionError::Invalid)?
            .len();
        if delivery.pending.len() >= MAX_PENDING_EVENTS
            || delivery.bytes.saturating_add(bytes) > MAX_PENDING_BYTES
        {
            delivery.closed = true;
            delivery.pending.clear();
            return Err(SessionError::Closed);
        }
        delivery.bytes += bytes;
        delivery.pending.push_back((kind, payload));
        Ok(())
    }
}

impl SessionSubscription {
    /// Return the connection-owned release identity.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.0.id
    }

    /// Flush bounded pending events in order after the response has been enqueued.
    ///
    /// # Errors
    /// Returns `Closed` when the observer overflowed or the transport rejects delivery.
    pub fn activate(&self) -> Result<(), SessionError> {
        let mut delivery = self.0.delivery.lock().map_err(|_| SessionError::Closed)?;
        if delivery.closed {
            return Err(SessionError::Closed);
        }
        while let Some((kind, payload)) = delivery.pending.pop_front() {
            if (self.0.sink)(kind, payload).is_err() {
                delivery.closed = true;
                delivery.pending.clear();
                return Err(SessionError::Closed);
            }
        }
        delivery.bytes = 0;
        delivery.active = true;
        Ok(())
    }
}

impl Drop for SessionSubscription {
    fn drop(&mut self) {
        let mut delivery = self
            .0
            .delivery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        delivery.closed = true;
        delivery.pending.clear();
    }
}

fn parse_time(value: &str) -> Result<i64, SessionError> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.timestamp_millis())
        .map_err(|_| SessionError::Invalid)
}

#[cfg(test)]
mod tests;
