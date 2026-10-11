use aidex_native_sessions::PermissionMode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppPreferences {
    pub accent: String,
    pub theme: Theme,
    pub density: Density,
    pub reduce_motion: bool,
    pub keyboard_hints: bool,
    pub model_id: String,
    pub runtime_id: Option<String>,
    pub runtime_model_id: Option<String>,
    pub skill_ids: Vec<String>,
    pub workspace_id: String,
    pub permission_mode: Option<PermissionMode>,
    pub instructions: String,
    pub fallback_model: Option<String>,
    pub max_steps: u32,
    pub max_tokens: u32,
    pub shortcuts: BTreeMap<String, String>,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            accent: "#1a8cff".into(),
            theme: Theme::Dark,
            density: Density::Comfortable,
            reduce_motion: false,
            keyboard_hints: true,
            model_id: String::new(),
            runtime_id: None,
            runtime_model_id: None,
            skill_ids: vec![],
            workspace_id: String::new(),
            permission_mode: None,
            instructions: String::new(),
            fallback_model: None,
            max_steps: 16,
            max_tokens: 4096,
            shortcuts: BTreeMap::from([
                ("new_session".into(), "Mod+k".into()),
                ("search".into(), "Mod+p".into()),
                ("toggle_sidebar".into(), "Mod+b".into()),
                ("providers".into(), "Mod+Shift+p".into()),
            ]),
        }
    }
}

impl AppPreferences {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.accent.len() != 7
            || !self.accent.starts_with('#')
            || !self.accent[1..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("accent must be a six-digit hex color");
        }
        if self.instructions.len() > 32 * 1024 {
            return Err("instructions exceed 32 KiB");
        }
        if !(1..=128).contains(&self.max_steps) || !(1..=32_768).contains(&self.max_tokens) {
            return Err("agent limits are outside supported bounds");
        }
        if self.model_id.len() > 512
            || self.skill_ids.len() > 32
            || self.skill_ids.iter().any(|id| {
                id.is_empty()
                    || id.len() > 64
                    || !id.as_bytes()[0].is_ascii_lowercase()
                    || !id.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
                    })
            })
            || self
                .skill_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.skill_ids.len()
            || self
                .runtime_id
                .as_ref()
                .is_some_and(|value| value.len() > 128)
            || self
                .runtime_model_id
                .as_ref()
                .is_some_and(|value| value.len() > 512)
            || self.workspace_id.len() > 128
            || self
                .fallback_model
                .as_ref()
                .is_some_and(|value| value.len() > 512)
        {
            return Err("preference identifier exceeds bounds");
        }
        if self.shortcuts.len() > 16
            || self.shortcuts.iter().any(|(action, binding)| {
                !["new_session", "search", "toggle_sidebar", "providers"].contains(&action.as_str())
                    || binding.len() > 64
                    || binding.is_empty()
                    || !binding
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+')
            })
        {
            return Err("unsupported shortcut action or binding");
        }
        Ok(())
    }
}
