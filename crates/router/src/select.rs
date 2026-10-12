//! Candidate filtering and ranking (spec §36).

use std::{cmp::Ordering, collections::HashSet};

use aidex_providers::{CapabilitySource, Locality, ModelDescriptor, ModelRole, ToolCallSupport};

use crate::{
    PrivacyMode, RejectReason, Rejection, RouteDecision, RouteError, RouteRequest, TaskKind,
};

/// Default number of fallbacks kept after the primary.
pub const DEFAULT_MAX_FALLBACKS: usize = 3;

/// Selects a model for a task from the models providers currently offer.
///
/// Hard constraints (never relaxed): remote models are excluded when offline,
/// air-gapped, local-only, or secret-heavy; the context window, tool support,
/// JSON-schema output, embeddings/chat capability, and known RAM needs must
/// fit; models with assumed (unverified) limits are only used on explicit
/// request. Eligible models are ranked by: the caller's preferred model, the
/// privacy mode's locality preference, role match, native over structured
/// tools, configured over discovered metadata, then name for determinism.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Router {
    max_fallbacks: usize,
}

impl Default for Router {
    fn default() -> Self {
        Self {
            max_fallbacks: DEFAULT_MAX_FALLBACKS,
        }
    }
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_max_fallbacks(mut self, max: usize) -> Self {
        self.max_fallbacks = max;
        self
    }

    /// Choose a primary model and ordered fallbacks, explaining the choice.
    pub fn route(
        &self,
        request: &RouteRequest,
        models: &[ModelDescriptor],
    ) -> Result<RouteDecision, RouteError> {
        let mut seen = HashSet::new();
        let models: Vec<&ModelDescriptor> = models
            .iter()
            .filter(|model| seen.insert(&model.model))
            .collect();
        let mut reasons = constraint_reasons(request);

        if let Some(wanted) = &request.user_override {
            let model = models
                .iter()
                .find(|model| &model.model == wanted)
                .ok_or_else(|| RouteError::OverrideNotFound {
                    model: wanted.clone(),
                })?;
            check(request, model, true).map_err(|reason| RouteError::OverrideRejected {
                rejection: Rejection::new(wanted.clone(), reason),
            })?;
            reasons.push(format!("user override: {wanted}"));
            reasons.push("fallback disabled: the model was chosen explicitly".into());
            return Ok(RouteDecision {
                task: request.task,
                privacy: request.privacy,
                primary: wanted.clone(),
                fallbacks: Vec::new(),
                allow_remote: request.remote_block().is_none(),
                preferred_role: request.task.preferred_role(),
                reasons,
                rejected: Vec::new(),
            });
        }

        let mut eligible = Vec::new();
        let mut rejected = Vec::new();
        for model in models {
            match check(request, model, false) {
                Ok(()) => eligible.push(model),
                Err(reason) => rejected.push(Rejection::new(model.model.clone(), reason)),
            }
        }
        if let Some(preferred) = &request.preferred {
            if eligible.iter().any(|model| &model.model == preferred) {
                reasons.push(format!("preferred model {preferred} is eligible"));
            } else if let Some(rejection) = rejected.iter().find(|r| &r.model == preferred) {
                reasons.push(format!(
                    "preferred model {preferred} skipped: {}",
                    rejection.explanation
                ));
            } else {
                reasons.push(format!("preferred model {preferred} is not offered"));
            }
        }
        eligible.sort_by(|a, b| compare(request, a, b));
        let Some((primary, rest)) = eligible.split_first() else {
            reasons.push(if rejected.is_empty() {
                "no models are offered by any provider".into()
            } else {
                format!("all {} candidate model(s) were rejected", rejected.len())
            });
            return Err(RouteError::NoEligibleModel { reasons, rejected });
        };
        reasons.push(format!(
            "selected {}: {}",
            primary.model,
            describe(request, primary)
        ));
        let fallbacks: Vec<_> = rest
            .iter()
            .take(self.max_fallbacks)
            .map(|model| model.model.clone())
            .collect();
        if !fallbacks.is_empty() {
            let names: Vec<String> = fallbacks.iter().map(ToString::to_string).collect();
            reasons.push(format!("fallbacks in order: {}", names.join(", ")));
        }
        if !rejected.is_empty() {
            reasons.push(format!("{} model(s) rejected", rejected.len()));
        }
        Ok(RouteDecision {
            task: request.task,
            privacy: request.privacy,
            primary: primary.model.clone(),
            fallbacks,
            allow_remote: request.remote_block().is_none(),
            preferred_role: request.task.preferred_role(),
            reasons,
            rejected,
        })
    }
}

/// The first hard constraint `model` violates. `explicit` admits models with
/// assumed limits (the user chose them knowingly).
fn check(
    request: &RouteRequest,
    model: &ModelDescriptor,
    explicit: bool,
) -> Result<(), RejectReason> {
    if model.locality == Locality::Remote
        && let Some(cause) = request.remote_block()
    {
        return Err(RejectReason::RemoteForbidden { cause });
    }
    let capabilities = &model.capabilities;
    if request.task == TaskKind::Embed {
        if !capabilities.embeddings {
            return Err(RejectReason::WrongCapability {
                needed: "embeddings".into(),
            });
        }
    } else if !capabilities.chat {
        return Err(RejectReason::WrongCapability {
            needed: "chat".into(),
        });
    }
    if request.required_context > model.context_window {
        return Err(RejectReason::ContextTooSmall {
            window: model.context_window,
            required: request.required_context,
        });
    }
    if request.needs_tools && !capabilities.tool_calls.is_supported() {
        return Err(RejectReason::NoToolSupport);
    }
    if request.needs_json_schema && !capabilities.json_schema_output {
        return Err(RejectReason::NoJsonSchema);
    }
    if model.locality == Locality::Local
        && let (Some(required), Some(available)) = (model.min_ram_bytes, request.host.ram_bytes)
        && required > available
    {
        return Err(RejectReason::InsufficientMemory {
            required,
            available,
        });
    }
    if !explicit && source_rank(model.source).is_none() {
        return Err(RejectReason::UnverifiedCapabilities);
    }
    Ok(())
}

/// Preference among metadata sources: declared limits first, then limits the
/// provider reported, then the conservative known-family catalog. Assumed
/// limits (`None`) are only usable when the user names the model.
fn source_rank(source: CapabilitySource) -> Option<u8> {
    match source {
        CapabilitySource::Configured => Some(0),
        CapabilitySource::Discovered => Some(1),
        CapabilitySource::Catalog => Some(2),
        CapabilitySource::Assumed => None,
    }
}

fn locality_rank(privacy: PrivacyMode, locality: Locality) -> u8 {
    match (privacy, locality) {
        (PrivacyMode::CloudPreferred, Locality::Remote)
        | (
            PrivacyMode::Hybrid | PrivacyMode::LocalOnly | PrivacyMode::AirGapped,
            Locality::Local,
        ) => 0,
        _ => 1,
    }
}

fn role_rank(role: ModelRole, model: &ModelDescriptor) -> u8 {
    if model.has_role(role) {
        0
    } else if model.has_role(ModelRole::General) {
        1
    } else {
        2
    }
}

fn compare(request: &RouteRequest, a: &ModelDescriptor, b: &ModelDescriptor) -> Ordering {
    let role = request.task.preferred_role();
    let key = |model: &ModelDescriptor| {
        (
            u8::from(request.preferred.as_ref() != Some(&model.model)),
            locality_rank(request.privacy, model.locality),
            role_rank(role, model),
            u8::from(
                request.needs_tools && model.capabilities.tool_calls != ToolCallSupport::Native,
            ),
            source_rank(model.source).unwrap_or(u8::MAX),
        )
    };
    key(a).cmp(&key(b)).then_with(|| a.model.cmp(&b.model))
}

fn constraint_reasons(request: &RouteRequest) -> Vec<String> {
    let role = request.task.preferred_role();
    let mut reasons = vec![format!(
        "task {}: models with the {} role are preferred",
        request.task,
        role.as_str()
    )];
    reasons.push(match request.remote_block() {
        Some(block) => block.describe().to_owned(),
        None if request.privacy == PrivacyMode::CloudPreferred => {
            "privacy mode cloud_preferred: remote models first, local fallback".into()
        }
        None => "privacy mode hybrid: local models first, remote when needed".into(),
    });
    if request.required_context > 0 {
        reasons.push(format!(
            "requires a context window of at least {} tokens",
            request.required_context
        ));
    }
    if request.needs_tools {
        reasons.push("requires tool calls".into());
    }
    if request.needs_json_schema {
        reasons.push("requires JSON-schema output".into());
    }
    reasons
}

fn describe(request: &RouteRequest, model: &ModelDescriptor) -> String {
    let role = request.task.preferred_role();
    let role = match role_rank(role, model) {
        0 => format!("{} role", role.as_str()),
        1 => "general role".to_owned(),
        _ => "no matching role".to_owned(),
    };
    let mut parts = vec![
        format!("{} model", model.locality.as_str()),
        role,
        format!("context {} tokens", model.context_window),
        format!("tools {}", model.capabilities.tool_calls.as_str()),
    ];
    if request.preferred.as_ref() == Some(&model.model) {
        parts.insert(0, "preferred".into());
    }
    parts.join(", ")
}
