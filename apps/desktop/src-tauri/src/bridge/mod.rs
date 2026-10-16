//! The daemon bridge: a shared call connection plus forwarded stream subscriptions.

mod connection;
mod subscriptions;

pub use connection::{call_long, Connection};
pub use subscriptions::{
    EndedPayload, NotificationEmitter, NotificationPayload, Subscribed, Subscriptions, ENDED_EVENT,
    NOTIFICATION_EVENT,
};

/// Bridge state held by the app for the session.
#[derive(Default)]
pub struct Bridge {
    pub connection: Connection,
    pub subscriptions: Subscriptions,
}

impl Bridge {
    /// Drop the call connection and every subscription (drive change, daemon stop, exit).
    pub fn reset(&self) {
        self.connection.disconnect();
        self.subscriptions.clear();
    }
}

#[cfg(test)]
mod live_tests;
