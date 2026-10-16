//! Method registry: maps `<domain>.<verb>` names to handlers.
//!
//! Domain modules register handlers at startup (see `methods/`). Handlers are
//! synchronous and run on the connection's reader thread; long-running work
//! should be exposed as a stream so the connection stays responsive.

use crate::{
    errors::RpcError,
    lifecycle::ShutdownSignal,
    rpc::{RESERVED_PREFIX, is_valid_method_name},
    stream::NotificationSink,
    transport::ConnectionShared,
};
use aidex_core::ServiceErrorCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fmt, sync::Arc};

/// A request/response handler.
pub type MethodHandler = dyn Fn(&CallContext, Value) -> Result<Value, RpcError> + Send + Sync;

/// A subscription handler. It returns the subscribe call's initial `data`
/// and may move the sink into a thread to push notifications.
pub type StreamHandler =
    dyn Fn(&CallContext, Value, NotificationSink) -> Result<Value, RpcError> + Send + Sync;

pub(crate) enum Route {
    Call(Box<MethodHandler>),
    Stream(Box<StreamHandler>),
}

/// Identity a client declares in its handshake. Informational only: it is
/// not authenticated beyond possession of the endpoint capability.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

impl ClientInfo {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }

    /// Names are 1-64 bytes of `[A-Za-z0-9._-]`; versions are 1-64 printable ASCII bytes.
    pub fn validate(&self) -> Result<(), String> {
        let name_ok = (1..=64).contains(&self.name.len())
            && self
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        let version_ok = (1..=64).contains(&self.version.len())
            && self.version.bytes().all(|b| b.is_ascii_graphic());
        match (name_ok, version_ok) {
            (true, true) => Ok(()),
            (false, _) => Err("client.name must be 1-64 characters of [A-Za-z0-9._-]".into()),
            (_, false) => Err("client.version must be 1-64 printable ASCII characters".into()),
        }
    }
}

/// Per-call context passed to every handler.
pub struct CallContext {
    connection_id: u64,
    client: Arc<ClientInfo>,
    connection: Option<Arc<ConnectionShared>>,
    shutdown: ShutdownSignal,
}

impl CallContext {
    pub(crate) fn new(
        connection_id: u64,
        client: Arc<ClientInfo>,
        connection: Arc<ConnectionShared>,
        shutdown: ShutdownSignal,
    ) -> Self {
        Self {
            connection_id,
            client,
            connection: Some(connection),
            shutdown,
        }
    }

    /// A context without a connection, for in-process callers and handler unit tests.
    pub fn detached(client: ClientInfo, shutdown: ShutdownSignal) -> Self {
        Self {
            connection_id: 0,
            client: Arc::new(client),
            connection: None,
            shutdown,
        }
    }

    pub fn connection_id(&self) -> u64 {
        self.connection_id
    }

    pub fn client(&self) -> &ClientInfo {
        &self.client
    }

    /// The connection is closing or the daemon is stopping; abandon work.
    pub fn is_cancelled(&self) -> bool {
        self.shutdown.is_requested()
            || self
                .connection
                .as_ref()
                .is_some_and(|connection| !connection.is_open())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterError {
    /// The name does not follow `<domain>.<verb>` (lowercase, digits, `_`).
    InvalidName(&'static str),
    /// `aidex.*` names belong to the transport.
    Reserved(&'static str),
    Duplicate(&'static str),
}

impl RouterError {
    pub fn code(&self) -> ServiceErrorCode {
        match self {
            Self::InvalidName(_) | Self::Reserved(_) => ServiceErrorCode::InvalidArgument,
            Self::Duplicate(_) => ServiceErrorCode::Conflict,
        }
    }
}

impl fmt::Display for RouterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(
                f,
                "method name `{name}` must follow `<domain>.<verb>` (lowercase ASCII, digits, `_`)"
            ),
            Self::Reserved(name) => {
                write!(f, "method name `{name}` uses the reserved `aidex.` prefix")
            }
            Self::Duplicate(name) => write!(f, "method `{name}` is already registered"),
        }
    }
}

impl std::error::Error for RouterError {}

/// The daemon's method table.
#[derive(Default)]
pub struct Router {
    routes: BTreeMap<&'static str, Route>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a request/response method.
    pub fn register<F>(&mut self, method: &'static str, handler: F) -> Result<(), RouterError>
    where
        F: Fn(&CallContext, Value) -> Result<Value, RpcError> + Send + Sync + 'static,
    {
        self.insert(method, Route::Call(Box::new(handler)))
    }

    /// Register a read clients poll in the background (a status, a list, a
    /// conversation view): what it reads through the vault does not count as
    /// vault use (`VaultService::passively`), so polling never holds the idle
    /// auto-lock off. Only a person's actions and executing runs keep the
    /// vault awake. A locked vault still refuses the read.
    pub fn register_passive<F>(
        &mut self,
        method: &'static str,
        handler: F,
    ) -> Result<(), RouterError>
    where
        F: Fn(&CallContext, Value) -> Result<Value, RpcError> + Send + Sync + 'static,
    {
        self.register(method, move |context, params| {
            aidex_vault::VaultService::passively(|| handler(context, params))
        })
    }

    /// Register a subscription method (convention: `<domain>.subscribe`).
    ///
    /// The transport answers the call with `{ "subscription": "<id>", "data": <handler result> }`
    /// and guarantees that answer is written before any notification from the sink.
    pub fn register_stream<F>(
        &mut self,
        method: &'static str,
        handler: F,
    ) -> Result<(), RouterError>
    where
        F: Fn(&CallContext, Value, NotificationSink) -> Result<Value, RpcError>
            + Send
            + Sync
            + 'static,
    {
        self.insert(method, Route::Stream(Box::new(handler)))
    }

    fn insert(&mut self, method: &'static str, route: Route) -> Result<(), RouterError> {
        if method.starts_with(RESERVED_PREFIX) {
            return Err(RouterError::Reserved(method));
        }
        if !is_valid_method_name(method) {
            return Err(RouterError::InvalidName(method));
        }
        if self.routes.contains_key(method) {
            return Err(RouterError::Duplicate(method));
        }
        self.routes.insert(method, route);
        Ok(())
    }

    pub fn contains(&self, method: &str) -> bool {
        self.routes.contains_key(method)
    }

    /// Registered method names in sorted order.
    pub fn methods(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.routes.keys().copied()
    }

    /// Invoke a request/response method directly (in-process callers and tests).
    pub fn call(
        &self,
        context: &CallContext,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        match self.routes.get(method) {
            Some(Route::Call(handler)) => handler(context, params),
            Some(Route::Stream(_)) => Err(RpcError::invalid_request(format!(
                "`{method}` is a stream method and needs a connection"
            ))),
            None => Err(RpcError::method_not_found(method)),
        }
    }

    pub(crate) fn route(&self, method: &str) -> Option<&Route> {
        self.routes.get(method)
    }
}

impl fmt::Debug for Router {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Router")
            .field("methods", &self.routes.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ErrorCode;
    use serde_json::json;

    #[test]
    fn registration_enforces_naming_reservation_and_uniqueness() {
        let mut router = Router::new();
        router
            .register("demo.echo", |_, params| Ok(params))
            .unwrap();
        assert_eq!(
            router.register("demo.echo", |_, p| Ok(p)),
            Err(RouterError::Duplicate("demo.echo"))
        );
        assert_eq!(
            router.register("aidex.handshake", |_, p| Ok(p)),
            Err(RouterError::Reserved("aidex.handshake"))
        );
        assert_eq!(
            router.register("Echo", |_, p| Ok(p)),
            Err(RouterError::InvalidName("Echo"))
        );
        router
            .register_stream("demo.subscribe", |_, _, _| Ok(Value::Null))
            .unwrap();
        assert_eq!(
            router.methods().collect::<Vec<_>>(),
            ["demo.echo", "demo.subscribe"]
        );
    }

    #[test]
    fn direct_calls_dispatch_and_report_missing_methods() {
        let mut router = Router::new();
        router
            .register("demo.echo", |context, params| {
                Ok(json!({ "client": context.client().name, "params": params }))
            })
            .unwrap();
        let context = CallContext::detached(ClientInfo::new("test", "1"), ShutdownSignal::new());
        assert_eq!(
            router.call(&context, "demo.echo", json!([1])).unwrap(),
            json!({ "client": "test", "params": [1] })
        );
        assert_eq!(
            router
                .call(&context, "demo.missing", Value::Null)
                .unwrap_err()
                .kind(),
            Some(ErrorCode::MethodNotFound)
        );
        assert!(!context.is_cancelled());
    }

    #[test]
    fn passive_methods_run_in_a_passive_vault_scope_and_others_do_not() {
        let mut router = Router::new();
        router
            .register_passive("demo.poll", |_, _| {
                Ok(json!(aidex_vault::VaultService::is_passive()))
            })
            .unwrap();
        router
            .register("demo.act", |_, _| {
                Ok(json!(aidex_vault::VaultService::is_passive()))
            })
            .unwrap();
        let context = CallContext::detached(ClientInfo::new("test", "1"), ShutdownSignal::new());
        assert_eq!(
            router.call(&context, "demo.poll", Value::Null).unwrap(),
            json!(true)
        );
        assert_eq!(
            router.call(&context, "demo.act", Value::Null).unwrap(),
            json!(false)
        );
        assert!(!aidex_vault::VaultService::is_passive());
    }

    #[test]
    fn client_info_is_validated() {
        assert!(ClientInfo::new("aidex-cli", "0.1.0").validate().is_ok());
        assert!(ClientInfo::new("", "1").validate().is_err());
        assert!(ClientInfo::new("bad name", "1").validate().is_err());
        assert!(ClientInfo::new("ok", "has space").validate().is_err());
    }
}
