//! The provider registry: configured and runtime providers, account metadata,
//! aggregated model listings with per-provider failures and catalog status,
//! and the persisted default model.

use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard},
    thread,
};

use aidex_core::{ServiceErrorCode, Timestamp};
use aidex_drive::DrivePaths;
use serde::{Deserialize, Serialize};

use crate::{
    AccountMetadata, ConfigError, ContentPart, CredentialResolver, EgressGate, HealthStatus,
    HttpOptions, ModelConfig, ModelDescriptor, ModelEvent, ModelInput, ModelObserver,
    ModelProvider, ModelRef, NoopObserver, OpenAiCompatibleProvider, PartLimits,
    ProviderAccountsConfig, ProviderConfig, ProviderDescriptor, ProviderError, ProviderErrorKind,
    ProviderHealth, ProviderId, ProvidersConfig,
    config::{PROVIDER_ACCOUNTS_FILE, PROVIDERS_FILE, prepare_config_file, write_toml},
};

/// How a provider entered the registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderOrigin {
    /// Declared in `providers.toml`.
    Configured,
    /// Registered by a running service (for example local inference); not persisted.
    Runtime,
}

/// The last model listing the registry recorded for a provider. Kept in
/// memory only; reading it never opens a connection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogStatus {
    pub fetched_at: Timestamp,
    /// Models listed by that call (0 when it failed).
    pub model_count: usize,
    pub error: Option<ProviderFailure>,
}

/// A provider as `model.providers` reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderEntry {
    pub descriptor: ProviderDescriptor,
    pub origin: ProviderOrigin,
    /// The account's name from `provider-accounts.toml`.
    #[serde(default)]
    pub display_name: Option<String>,
    /// The preset the account was created from.
    #[serde(default)]
    pub preset_id: Option<String>,
    /// Disabled accounts are configured but neither listed nor used.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// Whether the account discovers models (runtime providers: `false`).
    #[serde(default)]
    pub discover_models: bool,
    /// The account's model declarations (runtime providers: empty).
    #[serde(default)]
    pub declared_models: Vec<ModelConfig>,
    /// The last listing recorded in this process, if any.
    #[serde(default)]
    pub catalog: Option<CatalogStatus>,
}

fn enabled_by_default() -> bool {
    true
}

/// A change to a configured account's metadata; `None` keeps a field.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderPatch {
    /// `Some(None)` clears the display name.
    pub display_name: Option<Option<String>>,
    pub enabled: Option<bool>,
}

/// A provider whose model listing failed; reported alongside the models of
/// the providers that succeeded, never hidden.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderFailure {
    pub provider_id: ProviderId,
    pub kind: ProviderErrorKind,
    pub message: String,
}

impl ProviderFailure {
    pub fn new(provider_id: ProviderId, error: &ProviderError) -> Self {
        Self {
            provider_id,
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelListing {
    /// Models ordered by `provider/model`.
    pub models: Vec<ModelDescriptor>,
    pub failures: Vec<ProviderFailure>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RegistryError {
    Config(ConfigError),
    /// A provider configuration was rejected.
    Invalid(String),
    Duplicate(ProviderId),
    /// Not registered, or (for listings and defaults) disabled.
    NotFound(ProviderId),
    /// The provider does not list this model.
    ModelNotListed(ModelRef),
    /// The provider was registered at runtime and is not in `providers.toml`.
    NotConfigured(ProviderId),
    Provider {
        provider_id: ProviderId,
        error: ProviderError,
    },
}

impl RegistryError {
    pub fn code(&self) -> ServiceErrorCode {
        match self {
            Self::Config(error) => error.code(),
            Self::Invalid(_) => ServiceErrorCode::InvalidArgument,
            Self::Duplicate(_) | Self::NotConfigured(_) => ServiceErrorCode::Conflict,
            Self::NotFound(_) | Self::ModelNotListed(_) => ServiceErrorCode::NotFound,
            Self::Provider { error, .. } => error.code(),
        }
    }
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(f, "provider configuration: {error}"),
            Self::Invalid(message) => f.write_str(message),
            Self::Duplicate(id) => write!(f, "provider `{id}` already exists"),
            Self::NotFound(id) => write!(f, "provider `{id}` is not registered"),
            Self::ModelNotListed(model) => {
                write!(f, "model `{model}` is not offered by its provider")
            }
            Self::NotConfigured(id) => write!(
                f,
                "provider `{id}` is managed by a running service and is not in providers.toml"
            ),
            Self::Provider { provider_id, error } => write!(f, "provider `{provider_id}`: {error}"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<ConfigError> for RegistryError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

/// Message of the refusal reported for disabled providers.
const DISABLED: &str = "provider is disabled";

struct Entry {
    provider: Arc<dyn ModelProvider>,
    origin: ProviderOrigin,
}

struct Inner {
    config: ProvidersConfig,
    providers: BTreeMap<ProviderId, Entry>,
    /// Metadata of configured accounts; an absent entry means the defaults.
    accounts: BTreeMap<ProviderId, AccountMetadata>,
}

impl Inner {
    fn is_enabled(&self, id: &ProviderId) -> bool {
        self.accounts
            .get(id)
            .is_none_or(|metadata| metadata.enabled)
    }

    fn origin(&self, id: &ProviderId) -> Result<ProviderOrigin, RegistryError> {
        self.providers
            .get(id)
            .map(|entry| entry.origin)
            .ok_or_else(|| RegistryError::NotFound(id.clone()))
    }

    fn require_configured(&self, id: &ProviderId) -> Result<(), RegistryError> {
        match self.origin(id)? {
            ProviderOrigin::Configured => Ok(()),
            ProviderOrigin::Runtime => Err(RegistryError::NotConfigured(id.clone())),
        }
    }

    fn entry(
        &self,
        catalog: &BTreeMap<ProviderId, CatalogStatus>,
        id: &ProviderId,
        entry: &Entry,
    ) -> ProviderEntry {
        let config = match entry.origin {
            ProviderOrigin::Configured => self.config.providers.iter().find(|p| &p.id == id),
            ProviderOrigin::Runtime => None,
        };
        let metadata = self.accounts.get(id).cloned().unwrap_or_default();
        ProviderEntry {
            descriptor: entry.provider.descriptor().clone(),
            origin: entry.origin,
            display_name: metadata.display_name,
            preset_id: metadata.preset_id,
            enabled: metadata.enabled,
            discover_models: config.is_some_and(|config| config.discover_models),
            declared_models: config
                .map(|config| config.models.clone())
                .unwrap_or_default(),
            catalog: catalog.get(id).cloned(),
        }
    }
}

/// Store `metadata` for `id`, dropping entries that carry only defaults.
fn set_metadata(
    accounts: &mut BTreeMap<ProviderId, AccountMetadata>,
    id: &ProviderId,
    metadata: AccountMetadata,
) {
    if metadata.is_default() {
        accounts.remove(id);
    } else {
        accounts.insert(id.clone(), metadata);
    }
}

/// Trim the display name and validate the result.
fn normalized(mut metadata: AccountMetadata) -> Result<AccountMetadata, RegistryError> {
    metadata.display_name = metadata.display_name.map(|name| name.trim().to_owned());
    metadata.validate().map_err(RegistryError::Invalid)?;
    Ok(metadata)
}

type Listed = (
    ProviderId,
    Arc<dyn ModelProvider>,
    Result<Vec<ModelDescriptor>, ProviderError>,
);

/// All model providers known to this process.
///
/// Configured providers come from `private_config/providers.toml` and their
/// account metadata from `private_config/provider-accounts.toml`; runtime
/// providers (such as the local llama.cpp service) are registered by their
/// owning service and are never written to disk. Methods take `&self`, so one
/// `Arc<ProviderRegistry>` is shared by every caller.
pub struct ProviderRegistry {
    config_path: Option<PathBuf>,
    accounts_path: Option<PathBuf>,
    resolver: Arc<dyn CredentialResolver>,
    gate: Arc<dyn EgressGate>,
    observer: Arc<dyn ModelObserver>,
    options: HttpOptions,
    inner: RwLock<Inner>,
    /// Lock order: `inner` before `catalog`.
    catalog: Mutex<BTreeMap<ProviderId, CatalogStatus>>,
}

impl fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let providers: Vec<String> = self
            .read()
            .providers
            .keys()
            .map(ToString::to_string)
            .collect();
        f.debug_struct("ProviderRegistry")
            .field("config_path", &self.config_path)
            .field("providers", &providers)
            .finish_non_exhaustive()
    }
}

impl ProviderRegistry {
    /// Load configured providers from the drive. A missing file is an empty
    /// registry; an invalid file or provider entry is an error.
    pub fn load(
        paths: &DrivePaths,
        resolver: Arc<dyn CredentialResolver>,
        gate: Arc<dyn EgressGate>,
    ) -> Result<Self, RegistryError> {
        Self::load_with(paths, resolver, gate, HttpOptions::default())
    }

    /// [`ProviderRegistry::load`] with explicit HTTP options. Account metadata
    /// for ids that are not configured is ignored (and dropped on the next
    /// write); a missing metadata file means no metadata.
    pub fn load_with(
        paths: &DrivePaths,
        resolver: Arc<dyn CredentialResolver>,
        gate: Arc<dyn EgressGate>,
        options: HttpOptions,
    ) -> Result<Self, RegistryError> {
        let path = prepare_config_file(paths, PROVIDERS_FILE)?;
        let accounts_path = prepare_config_file(paths, PROVIDER_ACCOUNTS_FILE)?;
        let config = ProvidersConfig::load(&path)?;
        let mut accounts = ProviderAccountsConfig::load(&accounts_path)?.into_map();
        accounts.retain(|id, _| config.providers.iter().any(|p| &p.id == id));
        let registry = Self {
            config_path: Some(path),
            accounts_path: Some(accounts_path),
            ..Self::in_memory_with(resolver, gate, options)
        };
        let mut providers = BTreeMap::new();
        for provider in &config.providers {
            providers.insert(
                provider.id.clone(),
                Entry {
                    provider: registry.build(provider)?,
                    origin: ProviderOrigin::Configured,
                },
            );
        }
        *registry.write() = Inner {
            config,
            providers,
            accounts,
        };
        Ok(registry)
    }

    /// A registry without persistence (tests and embedding).
    pub fn in_memory(resolver: Arc<dyn CredentialResolver>, gate: Arc<dyn EgressGate>) -> Self {
        Self::in_memory_with(resolver, gate, HttpOptions::default())
    }

    fn in_memory_with(
        resolver: Arc<dyn CredentialResolver>,
        gate: Arc<dyn EgressGate>,
        options: HttpOptions,
    ) -> Self {
        Self {
            config_path: None,
            accounts_path: None,
            resolver,
            gate,
            observer: Arc::new(NoopObserver),
            options,
            inner: RwLock::new(Inner {
                config: ProvidersConfig::default(),
                providers: BTreeMap::new(),
                accounts: BTreeMap::new(),
            }),
            catalog: Mutex::new(BTreeMap::new()),
        }
    }

    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn ModelObserver>) -> Self {
        self.observer = observer;
        self
    }

    fn read(&self) -> RwLockReadGuard<'_, Inner> {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Inner> {
        self.inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn catalog(&self) -> MutexGuard<'_, BTreeMap<ProviderId, CatalogStatus>> {
        self.catalog
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn build(&self, config: &ProviderConfig) -> Result<Arc<dyn ModelProvider>, RegistryError> {
        if config.kind != crate::ProviderKind::OpenaiCompatible {
            return crate::cloud::CloudProvider::new(
                config,
                Arc::clone(&self.resolver),
                Arc::clone(&self.gate),
                self.options,
            )
            .map(|provider| Arc::new(provider) as Arc<dyn ModelProvider>)
            .map_err(RegistryError::Invalid);
        }
        OpenAiCompatibleProvider::with_options(
            config,
            Arc::clone(&self.resolver),
            Arc::clone(&self.gate),
            self.options,
        )
        .map(|provider| Arc::new(provider) as Arc<dyn ModelProvider>)
        .map_err(|error| RegistryError::Invalid(error.0))
    }

    fn persist(&self, config: &ProvidersConfig) -> Result<(), RegistryError> {
        if let Some(path) = &self.config_path {
            write_toml(path, config)?;
        }
        Ok(())
    }

    /// Write metadata for the accounts `config` configures.
    fn persist_accounts(
        &self,
        config: &ProvidersConfig,
        accounts: &BTreeMap<ProviderId, AccountMetadata>,
    ) -> Result<(), RegistryError> {
        if let Some(path) = &self.accounts_path {
            let configured: HashSet<&ProviderId> = config.providers.iter().map(|p| &p.id).collect();
            let kept: BTreeMap<ProviderId, AccountMetadata> = accounts
                .iter()
                .filter(|(id, _)| configured.contains(id))
                .map(|(id, metadata)| (id.clone(), metadata.clone()))
                .collect();
            write_toml(path, &ProviderAccountsConfig::from_map(&kept))?;
        }
        Ok(())
    }

    fn accounts_file_exists(&self) -> bool {
        self.accounts_path
            .as_ref()
            .is_some_and(|path| path.exists())
    }

    /// Persist then apply a new configuration and/or account metadata. The
    /// metadata is written first: if `providers.toml` then fails, the previous
    /// metadata is written back (metadata for unconfigured ids is ignored on
    /// load anyway), so neither file is left describing a state the other
    /// does not.
    fn commit(
        &self,
        inner: &mut Inner,
        config: Option<ProvidersConfig>,
        accounts: Option<BTreeMap<ProviderId, AccountMetadata>>,
    ) -> Result<(), RegistryError> {
        let target = config.as_ref().unwrap_or(&inner.config);
        if let Some(accounts) = &accounts {
            self.persist_accounts(target, accounts)?;
        }
        if let Some(config) = &config
            && let Err(error) = self.persist(config)
        {
            if accounts.is_some() {
                let _ = self.persist_accounts(&inner.config, &inner.accounts);
            }
            return Err(error);
        }
        if let Some(config) = config {
            inner.config = config;
        }
        if let Some(accounts) = accounts {
            inner.accounts = accounts;
        }
        Ok(())
    }

    fn entry_of(&self, inner: &Inner, id: &ProviderId) -> Option<ProviderEntry> {
        let catalog = self.catalog();
        inner
            .providers
            .get(id)
            .map(|entry| inner.entry(&catalog, id, entry))
    }

    /// Validate, persist, and activate a configured provider with default
    /// account metadata.
    pub fn add_provider(
        &self,
        config: ProviderConfig,
    ) -> Result<ProviderDescriptor, RegistryError> {
        self.add_provider_with(config, AccountMetadata::default())
            .map(|entry| entry.descriptor)
    }

    /// Validate, persist, and activate a configured provider together with its
    /// account metadata (written to `provider-accounts.toml`, never to
    /// `providers.toml`).
    pub fn add_provider_with(
        &self,
        config: ProviderConfig,
        metadata: AccountMetadata,
    ) -> Result<ProviderEntry, RegistryError> {
        let metadata = normalized(metadata)?;
        let provider = self.build(&config)?;
        let descriptor = provider.descriptor().clone();
        let mut inner = self.write();
        if inner.providers.contains_key(&config.id) {
            return Err(RegistryError::Duplicate(config.id));
        }
        let mut next = inner.config.clone();
        next.providers.push(config);
        // Rewrite the metadata file when it exists, so a stale entry for a
        // reused id cannot come back on the next load.
        let accounts = (!metadata.is_default() || self.accounts_file_exists()).then(|| {
            let mut accounts = inner.accounts.clone();
            set_metadata(&mut accounts, &descriptor.id, metadata.clone());
            accounts
        });
        self.commit(&mut inner, Some(next), accounts)?;
        inner.providers.insert(
            descriptor.id.clone(),
            Entry {
                provider,
                origin: ProviderOrigin::Configured,
            },
        );
        self.catalog().remove(&descriptor.id);
        let entry = self
            .entry_of(&inner, &descriptor.id)
            .ok_or_else(|| RegistryError::NotFound(descriptor.id.clone()))?;
        drop(inner);
        self.observer.observe(&ModelEvent::ProviderAdded {
            provider_id: descriptor.id.clone(),
            provider_kind: descriptor.kind,
            locality: descriptor.locality,
        });
        Ok(entry)
    }

    /// Atomically replace account configuration after validating its adapter,
    /// keeping its account metadata.
    pub fn update_provider(
        &self,
        config: ProviderConfig,
    ) -> Result<ProviderDescriptor, RegistryError> {
        self.update_provider_with(config, None)
            .map(|entry| entry.descriptor)
    }

    /// Atomically replace account configuration (and, when `metadata` is
    /// given, its account metadata). The default model is cleared when it
    /// points at this provider and the provider no longer offers it or is
    /// disabled.
    pub fn update_provider_with(
        &self,
        config: ProviderConfig,
        metadata: Option<AccountMetadata>,
    ) -> Result<ProviderEntry, RegistryError> {
        let metadata = metadata.map(normalized).transpose()?;
        let provider = self.build(&config)?;
        let descriptor = provider.descriptor().clone();
        let mut inner = self.write();
        inner.require_configured(&config.id)?;
        let mut next = inner.config.clone();
        let position = next
            .providers
            .iter()
            .position(|p| p.id == config.id)
            .ok_or_else(|| RegistryError::NotConfigured(config.id.clone()))?;
        next.providers[position] = config;
        let enabled = metadata
            .as_ref()
            .map_or_else(|| inner.is_enabled(&descriptor.id), |m| m.enabled);
        let cleared = next.default_model().is_some_and(|m| {
            m.provider_id == descriptor.id
                && (!enabled
                    || (!next.providers[position].discover_models
                        && !next.providers[position]
                            .models
                            .iter()
                            .any(|model| model.id == m.model_id)))
        });
        if cleared {
            next.default_model = None;
        }
        let accounts = metadata.map(|metadata| {
            let mut accounts = inner.accounts.clone();
            set_metadata(&mut accounts, &descriptor.id, metadata);
            accounts
        });
        self.commit(&mut inner, Some(next), accounts)?;
        inner.providers.insert(
            descriptor.id.clone(),
            Entry {
                provider,
                origin: ProviderOrigin::Configured,
            },
        );
        self.catalog().remove(&descriptor.id);
        let entry = self
            .entry_of(&inner, &descriptor.id)
            .ok_or_else(|| RegistryError::NotFound(descriptor.id.clone()))?;
        drop(inner);
        if cleared {
            self.observer
                .observe(&ModelEvent::DefaultChanged { model: None });
        }
        Ok(entry)
    }

    /// Change a configured account's display name or enabled flag. Disabling
    /// the provider of the default model clears the default.
    pub fn patch_provider(
        &self,
        id: &ProviderId,
        patch: ProviderPatch,
    ) -> Result<ProviderEntry, RegistryError> {
        let mut inner = self.write();
        inner.require_configured(id)?;
        let mut metadata = inner.accounts.get(id).cloned().unwrap_or_default();
        if let Some(display_name) = patch.display_name {
            metadata.display_name = display_name;
        }
        if let Some(enabled) = patch.enabled {
            metadata.enabled = enabled;
        }
        let metadata = normalized(metadata)?;
        let cleared = !metadata.enabled
            && inner
                .config
                .default_model()
                .is_some_and(|model| &model.provider_id == id);
        let config = cleared.then(|| {
            let mut next = inner.config.clone();
            next.default_model = None;
            next
        });
        let mut accounts = inner.accounts.clone();
        set_metadata(&mut accounts, id, metadata);
        self.commit(&mut inner, config, Some(accounts))?;
        let entry = self
            .entry_of(&inner, id)
            .ok_or_else(|| RegistryError::NotFound(id.clone()))?;
        drop(inner);
        if cleared {
            self.observer
                .observe(&ModelEvent::DefaultChanged { model: None });
        }
        Ok(entry)
    }

    /// Remove a configured provider, its account metadata, and the default
    /// model if it pointed there.
    pub fn remove_provider(&self, id: &ProviderId) -> Result<(), RegistryError> {
        let mut inner = self.write();
        inner.require_configured(id)?;
        let mut next = inner.config.clone();
        next.providers.retain(|provider| &provider.id != id);
        let cleared = next
            .default_model()
            .is_some_and(|model| &model.provider_id == id);
        if cleared {
            next.default_model = None;
        }
        let accounts = inner.accounts.contains_key(id).then(|| {
            let mut accounts = inner.accounts.clone();
            accounts.remove(id);
            accounts
        });
        self.commit(&mut inner, Some(next), accounts)?;
        inner.providers.remove(id);
        self.catalog().remove(id);
        drop(inner);
        self.observer.observe(&ModelEvent::ProviderRemoved {
            provider_id: id.clone(),
        });
        if cleared {
            self.observer
                .observe(&ModelEvent::DefaultChanged { model: None });
        }
        Ok(())
    }

    /// Register a runtime provider (not persisted).
    pub fn register(&self, provider: Box<dyn ModelProvider>) -> Result<(), RegistryError> {
        self.register_shared(Arc::from(provider))
    }

    /// Register a runtime provider the caller keeps a handle to.
    pub fn register_shared(&self, provider: Arc<dyn ModelProvider>) -> Result<(), RegistryError> {
        let descriptor = provider.descriptor().clone();
        let mut inner = self.write();
        if inner.providers.contains_key(&descriptor.id) {
            return Err(RegistryError::Duplicate(descriptor.id));
        }
        inner.providers.insert(
            descriptor.id.clone(),
            Entry {
                provider,
                origin: ProviderOrigin::Runtime,
            },
        );
        self.catalog().remove(&descriptor.id);
        drop(inner);
        self.observer.observe(&ModelEvent::ProviderAdded {
            provider_id: descriptor.id,
            provider_kind: descriptor.kind,
            locality: descriptor.locality,
        });
        Ok(())
    }

    /// Remove a runtime provider.
    pub fn unregister(&self, id: &ProviderId) -> Result<(), RegistryError> {
        let mut inner = self.write();
        match inner.origin(id)? {
            ProviderOrigin::Configured => {
                return Err(RegistryError::Invalid(format!(
                    "provider `{id}` is configured; remove it with `aidex provider remove`"
                )));
            }
            ProviderOrigin::Runtime => {}
        }
        inner.providers.remove(id);
        self.catalog().remove(id);
        drop(inner);
        self.observer.observe(&ModelEvent::ProviderRemoved {
            provider_id: id.clone(),
        });
        Ok(())
    }

    /// Every provider, ordered by id, including disabled accounts, with
    /// account metadata and the last recorded catalog status. Opens no
    /// connection.
    pub fn providers(&self) -> Vec<ProviderEntry> {
        let inner = self.read();
        let catalog = self.catalog();
        inner
            .providers
            .iter()
            .map(|(id, entry)| inner.entry(&catalog, id, entry))
            .collect()
    }

    /// One provider as [`ProviderRegistry::providers`] reports it.
    pub fn provider_entry(&self, id: &ProviderId) -> Option<ProviderEntry> {
        let inner = self.read();
        self.entry_of(&inner, id)
    }

    /// The adapter of an enabled provider; `None` when it is unknown or disabled.
    pub fn provider(&self, id: &ProviderId) -> Option<Arc<dyn ModelProvider>> {
        let inner = self.read();
        inner
            .providers
            .get(id)
            .filter(|_| inner.is_enabled(id))
            .map(|entry| Arc::clone(&entry.provider))
    }

    /// The stored configuration of a configured provider (enabled or not).
    pub fn provider_config(&self, id: &ProviderId) -> Option<ProviderConfig> {
        self.read()
            .config
            .providers
            .iter()
            .find(|provider| &provider.id == id)
            .cloned()
    }

    /// The descriptor of `model`'s provider (enabled or not) and the preset
    /// its account was created from: what input decisions depend on.
    fn input_account(
        &self,
        model: &ModelDescriptor,
    ) -> Option<(ProviderDescriptor, Option<String>)> {
        let inner = self.read();
        let id = &model.model.provider_id;
        let entry = inner.providers.get(id)?;
        let preset = inner
            .accounts
            .get(id)
            .and_then(|metadata| metadata.preset_id.clone());
        Some((entry.provider.descriptor().clone(), preset))
    }

    /// Whether `model` reads images and PDFs through its provider account
    /// ([`crate::model_input`]); neither for an unknown provider. Opens no
    /// connection.
    pub fn model_input(&self, model: &ModelDescriptor) -> ModelInput {
        self.input_account(model)
            .map(|(provider, preset)| crate::model_input(&provider, preset.as_deref(), model))
            .unwrap_or_default()
    }

    /// Whether `part` can be sent to `model` ([`crate::accepts_part`]);
    /// `false` for an unknown provider.
    pub fn accepts_part(&self, model: &ModelDescriptor, part: &ContentPart) -> bool {
        self.input_account(model).is_some_and(|(provider, preset)| {
            crate::accepts_part(&provider, preset.as_deref(), model, part)
        })
    }

    /// The vendor limits on image and PDF parts for `model`
    /// ([`crate::part_limits`]); `None` for an unknown provider.
    pub fn part_limits(&self, model: &ModelDescriptor) -> Option<PartLimits> {
        self.input_account(model)
            .map(|(provider, preset)| crate::part_limits(&provider, preset.as_deref(), model))
    }

    fn enabled_providers(&self) -> Vec<(ProviderId, Arc<dyn ModelProvider>)> {
        let inner = self.read();
        inner
            .providers
            .iter()
            .filter(|(id, _)| inner.is_enabled(id))
            .map(|(id, entry)| (id.clone(), Arc::clone(&entry.provider)))
            .collect()
    }

    /// Record the outcome of listing `provider` while it is still the adapter
    /// registered under `id`.
    fn record(
        &self,
        id: &ProviderId,
        provider: &Arc<dyn ModelProvider>,
        result: &Result<Vec<ModelDescriptor>, ProviderError>,
    ) {
        let inner = self.read();
        let current = inner
            .providers
            .get(id)
            .is_some_and(|entry| Arc::ptr_eq(&entry.provider, provider));
        if !current {
            return;
        }
        let status = CatalogStatus {
            fetched_at: Timestamp::now(),
            model_count: result.as_ref().map_or(0, Vec::len),
            error: result
                .as_ref()
                .err()
                .map(|error| ProviderFailure::new(id.clone(), error)),
        };
        self.catalog().insert(id.clone(), status);
    }

    /// List one adapter, keeping only its own models and turning a panic into
    /// an explicit failure.
    fn list_one(
        id: &ProviderId,
        provider: &dyn ModelProvider,
    ) -> Result<Vec<ModelDescriptor>, ProviderError> {
        catch_unwind(AssertUnwindSafe(|| provider.models()))
            .unwrap_or_else(|_| Err(ProviderError::unavailable("model listing panicked")))
            .map(|models| {
                models
                    .into_iter()
                    .filter(|model| &model.model.provider_id == id)
                    .collect()
            })
    }

    /// List `providers` concurrently and record each one's catalog status.
    fn list(&self, providers: Vec<(ProviderId, Arc<dyn ModelProvider>)>) -> ModelListing {
        let results: Vec<Listed> = thread::scope(|scope| {
            let handles: Vec<_> = providers
                .into_iter()
                .map(|(id, provider)| {
                    scope.spawn(move || {
                        let result = Self::list_one(&id, provider.as_ref());
                        (id, provider, result)
                    })
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .collect()
        });
        let mut listing = ModelListing::default();
        for (id, provider, result) in results {
            self.record(&id, &provider, &result);
            match result {
                Ok(models) => listing.models.extend(models),
                Err(error) => listing.failures.push(ProviderFailure::new(id, &error)),
            }
        }
        listing.models.sort_by(|a, b| a.model.cmp(&b.model));
        listing
    }

    /// Models from every enabled provider, listed concurrently. A failing
    /// provider is reported in [`ModelListing::failures`]; the others are
    /// still listed. Disabled providers are skipped without a failure. Each
    /// provider's outcome is recorded as its [`CatalogStatus`].
    pub fn models(&self) -> ModelListing {
        self.list(self.enabled_providers())
    }

    /// Drop cached listings (of `provider`, or of every enabled provider) and
    /// list again. With `provider`, only that provider is listed and
    /// returned; an unknown or disabled provider is reported as a failure.
    pub fn refresh_models(&self, provider: Option<&ProviderId>) -> ModelListing {
        let providers = match provider {
            None => self.enabled_providers(),
            Some(id) => {
                let inner = self.read();
                let Some(entry) = inner.providers.get(id) else {
                    return Self::failed(id, &format!("provider `{id}` is not registered"));
                };
                if !inner.is_enabled(id) {
                    return Self::failed(id, DISABLED);
                }
                vec![(id.clone(), Arc::clone(&entry.provider))]
            }
        };
        for (_, provider) in &providers {
            provider.invalidate_models();
        }
        self.list(providers)
    }

    fn failed(id: &ProviderId, reason: &str) -> ModelListing {
        ModelListing {
            models: Vec::new(),
            failures: vec![ProviderFailure::new(
                id.clone(),
                &ProviderError::unavailable(reason),
            )],
        }
    }

    /// Models of one enabled provider (its cache is used when fresh); the
    /// outcome is recorded as its [`CatalogStatus`]. Unknown and disabled
    /// providers are `NotFound`.
    pub fn models_for(&self, id: &ProviderId) -> Result<Vec<ModelDescriptor>, RegistryError> {
        let provider = self
            .provider(id)
            .ok_or_else(|| RegistryError::NotFound(id.clone()))?;
        let result = Self::list_one(id, provider.as_ref());
        self.record(id, &provider, &result);
        result.map_err(|error| RegistryError::Provider {
            provider_id: id.clone(),
            error,
        })
    }

    pub fn default_model(&self) -> Option<ModelRef> {
        self.read().config.default_model()
    }

    /// Persist `model` as the default after checking its enabled provider
    /// lists it.
    pub fn set_default_model(&self, model: ModelRef) -> Result<ModelDescriptor, RegistryError> {
        let descriptor = self
            .models_for(&model.provider_id)?
            .into_iter()
            .find(|descriptor| descriptor.model == model)
            .ok_or_else(|| RegistryError::ModelNotListed(model.clone()))?;
        self.store_default(Some(model))?;
        Ok(descriptor)
    }

    pub fn clear_default_model(&self) -> Result<(), RegistryError> {
        self.store_default(None)
    }

    fn store_default(&self, model: Option<ModelRef>) -> Result<(), RegistryError> {
        let mut inner = self.write();
        // The listing check ran without the lock. A provider disabled or
        // removed since then cleared no default, so refuse to persist one on
        // it now; under this lock the check and the write are atomic with
        // patch_provider and remove_provider.
        if let Some(model) = &model
            && (!inner.providers.contains_key(&model.provider_id)
                || !inner.is_enabled(&model.provider_id))
        {
            return Err(RegistryError::NotFound(model.provider_id.clone()));
        }
        let mut next = inner.config.clone();
        next.default_model = model.as_ref().map(ToString::to_string);
        self.commit(&mut inner, Some(next), None)?;
        drop(inner);
        self.observer.observe(&ModelEvent::DefaultChanged { model });
        Ok(())
    }

    /// A live health check; disabled providers report `Refused` without any
    /// connection.
    pub fn health(&self, id: &ProviderId) -> Result<ProviderHealth, RegistryError> {
        let inner = self.read();
        let entry = inner
            .providers
            .get(id)
            .ok_or_else(|| RegistryError::NotFound(id.clone()))?;
        if !inner.is_enabled(id) {
            return Ok(ProviderHealth {
                status: HealthStatus::Refused,
                error: None,
                message: Some(DISABLED.into()),
                latency_ms: None,
                checked_at: Timestamp::now(),
            });
        }
        let provider = Arc::clone(&entry.provider);
        drop(inner);
        Ok(provider.health())
    }
}
