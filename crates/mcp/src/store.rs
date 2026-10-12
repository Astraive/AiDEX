//! `McpConfigStore`: one closed-schema TOML file per server under
//! `DrivePaths::mcp()`, written atomically.

use std::{
    fs,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

use aidex_drive::DrivePaths;
use serde::Serialize;

use crate::{
    ImportedServer, McpError, McpErrorKind, McpSource, ServerConfig,
    config::{import_mcp_json, validate_id},
    redact::bounded_redacted,
};

/// Largest server file accepted.
const MAX_FILE_BYTES: u64 = 64 * 1024;
/// Most server files read from the directory.
pub const MAX_SERVERS: usize = 256;
const EXTENSION: &str = "toml";

/// A server file that could not be loaded; it stays on disk untouched.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InvalidServer {
    /// The file stem.
    pub id: String,
    pub error: String,
}

/// Every loadable server plus the files that failed to load.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ServerListing {
    pub servers: Vec<ServerConfig>,
    pub invalid: Vec<InvalidServer>,
}

/// The drive's MCP server configurations.
#[derive(Clone, Debug)]
pub struct McpConfigStore {
    dir: PathBuf,
}

impl McpConfigStore {
    /// The store in `DrivePaths::mcp()` (created when missing).
    pub fn open(paths: &DrivePaths) -> Result<Self, McpError> {
        let dir = paths.ensure_dir(&paths.mcp()).map_err(|error| {
            McpError::new(
                McpErrorKind::Io,
                format!("the MCP configuration directory is unavailable: {error}"),
            )
        })?;
        Ok(Self { dir })
    }

    /// A store rooted at an existing directory (tests and tools).
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Parse a `.mcp.json` into DISABLED entries attributed to `source`. Read-only:
    /// nothing is written and no value from the file's `env`/`headers` is kept.
    pub fn import_mcp_json(
        path: &Path,
        source: McpSource,
    ) -> Result<Vec<ImportedServer>, McpError> {
        import_mcp_json(path, source)
    }

    /// Every server, sorted by id; unreadable files are reported, not fatal.
    pub fn list(&self) -> Result<ServerListing, McpError> {
        let mut listing = ServerListing::default();
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(listing),
            Err(error) => return Err(McpError::io("cannot list MCP servers", &error)),
        };
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| McpError::io("cannot list MCP servers", &error))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            files.push((stem.to_owned(), path));
        }
        files.sort();
        for (stem, path) in files.into_iter().take(MAX_SERVERS) {
            match load_file(&path, &stem) {
                Ok(config) => listing.servers.push(config),
                Err(error) => listing.invalid.push(InvalidServer {
                    id: stem,
                    error: error.message,
                }),
            }
        }
        Ok(listing)
    }

    /// One server.
    pub fn get(&self, id: &str) -> Result<ServerConfig, McpError> {
        validate_id(id).map_err(|_| McpError::not_found(id))?;
        let path = self.path(id);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == ErrorKind::NotFound => Err(McpError::not_found(id)),
            Err(error) => Err(McpError::io("cannot read the MCP server", &error)),
            Ok(_) => load_file(&path, id),
        }
    }

    pub fn exists(&self, id: &str) -> bool {
        validate_id(id).is_ok() && fs::symlink_metadata(self.path(id)).is_ok()
    }

    /// Create a server; `conflict` when the id is taken.
    pub fn add(&self, config: &ServerConfig) -> Result<ServerConfig, McpError> {
        config.validate()?;
        if self.exists(&config.id) {
            return Err(McpError::new(
                McpErrorKind::Conflict,
                format!("an MCP server `{}` already exists", config.id),
            ));
        }
        if self.list()?.servers.len() >= MAX_SERVERS {
            return Err(McpError::new(
                McpErrorKind::Conflict,
                format!("at most {MAX_SERVERS} MCP servers can be configured"),
            ));
        }
        self.write(config)?;
        Ok(config.clone())
    }

    /// Replace an existing server; `not_found` when it does not exist.
    pub fn update(&self, config: &ServerConfig) -> Result<ServerConfig, McpError> {
        config.validate()?;
        self.get(&config.id)?;
        self.write(config)?;
        Ok(config.clone())
    }

    /// Enable or disable a server.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<ServerConfig, McpError> {
        let mut config = self.get(id)?;
        if config.enabled != enabled {
            config.enabled = enabled;
            self.write(&config)?;
        }
        Ok(config)
    }

    /// Delete a server file; `false` when there was none.
    pub fn remove(&self, id: &str) -> Result<bool, McpError> {
        if validate_id(id).is_err() {
            return Ok(false);
        }
        match fs::remove_file(self.path(id)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(McpError::io("cannot remove the MCP server", &error)),
        }
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.{EXTENSION}"))
    }

    fn write(&self, config: &ServerConfig) -> Result<(), McpError> {
        let text = toml::to_string(config).map_err(|_| {
            McpError::new(
                McpErrorKind::InvalidConfig,
                "the MCP server cannot be encoded",
            )
        })?;
        fs::create_dir_all(&self.dir)
            .map_err(|error| McpError::io("cannot create the MCP directory", &error))?;
        let target = self.path(&config.id);
        if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(McpError::invalid_config(
                "the MCP server file is a link and will not be replaced",
            ));
        }
        let suffix = aidex_core::EntityId::generate("tmp")
            .map(|id| id.as_str().to_owned())
            .unwrap_or_else(|_| "tmp".into());
        let temporary = self.dir.join(format!(".{}.{suffix}.partial", config.id));
        let written = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &target)
        })();
        if let Err(error) = written {
            let _ = fs::remove_file(&temporary);
            return Err(McpError::io("cannot save the MCP server", &error));
        }
        Ok(())
    }
}

fn load_file(path: &Path, stem: &str) -> Result<ServerConfig, McpError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| McpError::io("cannot inspect the MCP server file", &error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(McpError::invalid_config(
            "the MCP server file is not a regular file",
        ));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(McpError::invalid_config(
            "the MCP server file is larger than 64 KiB",
        ));
    }
    let mut text = String::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_FILE_BYTES).read_to_string(&mut text))
        .map_err(|error| McpError::io("cannot read the MCP server file", &error))?;
    let config: ServerConfig = toml::from_str(&text).map_err(|error| {
        McpError::invalid_config(format!(
            "the MCP server file is not a valid version 1 configuration: {}",
            bounded_redacted(error.message(), 300)
        ))
    })?;
    config.validate()?;
    if config.id != stem {
        return Err(McpError::invalid_config(format!(
            "the file name `{stem}.toml` does not match its id `{}`",
            config.id
        )));
    }
    Ok(config)
}
