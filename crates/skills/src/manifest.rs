//! The validated metadata of one skill, read from its front matter.

use crate::{
    FrontMatter, FrontMatterValue,
    name::{display_name, is_skill_name},
};

/// Longest `description`, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 1024;
/// Longest `version`, in characters.
pub const MAX_VERSION_CHARS: usize = 64;
/// Longest `display-name`, in characters.
pub const MAX_DISPLAY_NAME_CHARS: usize = 64;
/// Most `allowed-tools` entries.
pub const MAX_ALLOWED_TOOLS: usize = 64;
/// Largest instruction body (after the front matter).
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// Largest front matter block.
pub const MAX_FRONT_MATTER_BYTES: usize = 16 * 1024;
/// Largest `SKILL.md` read: the body plus its front matter and delimiters.
pub const MAX_SKILL_FILE_BYTES: u64 = (MAX_BODY_BYTES + MAX_FRONT_MATTER_BYTES + 64) as u64;

const NAME_RULE: &str =
    "`name` must be 1-64 lowercase letters, digits, or `-`, starting with a letter or digit";

/// What a skill declares about itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillManifest {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub version: Option<String>,
    /// Offered to the user in the composer (`user-invocable`, default true).
    pub user_invocable: bool,
    /// The model may load it on its own (`disable-model-invocation`, default false).
    pub model_invocable: bool,
    /// Informational `allowed-tools`; AiDEX policy still decides every tool call.
    pub allowed_tools: Vec<String>,
}

impl SkillManifest {
    /// Validate `front_matter`. A missing `name` falls back to `directory` when that
    /// is a valid skill name. Messages never quote values.
    pub fn from_front_matter(
        front_matter: Option<&FrontMatter>,
        directory: Option<&str>,
    ) -> Result<Self, String> {
        let empty = FrontMatter::default();
        let front_matter = front_matter.unwrap_or(&empty);
        let name = match front_matter.get("name") {
            Some(FrontMatterValue::Text(name)) if is_skill_name(name.trim()) => {
                name.trim().to_owned()
            }
            Some(_) => return Err(NAME_RULE.into()),
            None => match directory {
                Some(directory) if is_skill_name(directory) => directory.to_owned(),
                _ => {
                    return Err(format!(
                        "`name` is missing and the directory name is not a valid skill name; {NAME_RULE}"
                    ));
                }
            },
        };
        let description = match front_matter.get("description") {
            Some(FrontMatterValue::Text(text)) if !text.trim().is_empty() => text.trim().to_owned(),
            Some(FrontMatterValue::Text(_)) | None => {
                return Err(
                    "`description` is required (add YAML front matter with a description)".into(),
                );
            }
            Some(_) => return Err("`description` must be text".into()),
        };
        if description.chars().count() > MAX_DESCRIPTION_CHARS {
            return Err(format!(
                "`description` is longer than {MAX_DESCRIPTION_CHARS} characters"
            ));
        }
        if description
            .chars()
            .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
        {
            return Err("`description` contains control characters".into());
        }
        let version = optional_text(front_matter, &["version"], MAX_VERSION_CHARS)?;
        let display = optional_text(
            front_matter,
            &["display-name", "display_name"],
            MAX_DISPLAY_NAME_CHARS,
        )?;
        let user_invocable = boolean(front_matter, &["user-invocable", "user_invocable"], true)?;
        let model_invocable = !boolean(
            front_matter,
            &["disable-model-invocation", "disable_model_invocation"],
            false,
        )?;
        let allowed_tools = allowed_tools(front_matter)?;
        Ok(Self {
            display_name: display.unwrap_or_else(|| display_name(&name)),
            name,
            description,
            version,
            user_invocable,
            model_invocable,
            allowed_tools,
        })
    }
}

/// The name a skill would be listed under even when it is invalid: its declared
/// `name` when that is a valid slug, else its directory name when valid.
pub(crate) fn declared_name(
    front_matter: Option<&FrontMatter>,
    directory: Option<&str>,
) -> Option<String> {
    front_matter
        .and_then(|front_matter| front_matter.get("name"))
        .and_then(FrontMatterValue::as_text)
        .map(str::trim)
        .filter(|name| is_skill_name(name))
        .or(directory.filter(|name| is_skill_name(name)))
        .map(str::to_owned)
}

fn optional_text(
    front_matter: &FrontMatter,
    keys: &[&str],
    max_chars: usize,
) -> Result<Option<String>, String> {
    let Some((key, value)) = front_matter.get_any(keys) else {
        return Ok(None);
    };
    let FrontMatterValue::Text(text) = value else {
        return Err(format!("`{key}` must be text"));
    };
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.chars().count() > max_chars || text.chars().any(char::is_control) {
        return Err(format!(
            "`{key}` must be at most {max_chars} characters on one line"
        ));
    }
    Ok(Some(text.to_owned()))
}

/// YAML 1.1 spellings, as Claude Code accepts them.
fn boolean(front_matter: &FrontMatter, keys: &[&str], default: bool) -> Result<bool, String> {
    let Some((key, value)) = front_matter.get_any(keys) else {
        return Ok(default);
    };
    match value {
        FrontMatterValue::Bool(value) => Ok(*value),
        FrontMatterValue::Text(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "y" | "1" => Ok(true),
            "false" | "no" | "off" | "n" | "0" => Ok(false),
            _ => Err(format!("`{key}` must be true or false")),
        },
        FrontMatterValue::List(_) => Err(format!("`{key}` must be true or false")),
    }
}

/// A list, or a comma-separated string (`Read, Grep, Bash(git status:*)`).
fn allowed_tools(front_matter: &FrontMatter) -> Result<Vec<String>, String> {
    let Some((key, value)) = front_matter.get_any(&["allowed-tools", "allowed_tools"]) else {
        return Ok(Vec::new());
    };
    let tools: Vec<String> = match value {
        FrontMatterValue::List(items) => items
            .iter()
            .map(|item| item.trim().to_owned())
            .filter(|item| !item.is_empty())
            .collect(),
        FrontMatterValue::Text(text) => text
            .split(',')
            .map(|item| item.trim().to_owned())
            .filter(|item| !item.is_empty())
            .collect(),
        FrontMatterValue::Bool(_) => return Err(format!("`{key}` must be a list of tool names")),
    };
    if tools.len() > MAX_ALLOWED_TOOLS
        || tools
            .iter()
            .any(|tool| tool.chars().count() > 256 || tool.chars().any(char::is_control))
    {
        return Err(format!(
            "`{key}` must list at most {MAX_ALLOWED_TOOLS} tool names of up to 256 characters"
        ));
    }
    Ok(tools)
}
