//! `fs.read`: bounded, line-addressed reads with binary detection.

use std::{
    fs::File,
    io::{BufRead, BufReader, ErrorKind},
};

use aidex_policy::Action;
use aidex_workspace::Access;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{MAX_HASH_BYTES, hex, parse_path, plan_path, require_file, resolve, stat};
use crate::{
    ExecutionPermit, InputError, PathAccess, PlanFacts, PlanParts, RiskProfile, Tool, ToolContext,
    ToolDescriptor, ToolError, ToolOutput, ToolPlan,
    builtin::descriptor,
    diff::looks_binary,
    schema::{normalized, parse_input},
};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadInput {
    /// Workspace-relative path of the file (`/`-separated).
    path: String,
    /// First line to return, 1-based (default 1).
    #[serde(default)]
    start_line: Option<u64>,
    /// Most lines to return (default: as many as fit `max_bytes`).
    #[serde(default)]
    max_lines: Option<u64>,
    /// Most bytes of content to return; capped by the configured read limit.
    #[serde(default)]
    max_bytes: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadOutput {
    path: String,
    /// File size in bytes.
    size: u64,
    /// SHA-256 of the whole file (absent above 256 MiB). Pass it as
    /// `expected_sha256` to `fs.write`/`fs.patch` to guard against races.
    sha256: Option<String>,
    /// The file contains NUL bytes; no content is returned.
    binary: bool,
    /// Invalid UTF-8 was replaced with U+FFFD in `content`.
    lossy_utf8: bool,
    content: Option<String>,
    /// First line returned (1-based).
    start_line: u64,
    /// Last line returned, fully or partially (0 when none).
    end_line: u64,
    /// More content follows what was returned.
    truncated: bool,
    /// The line to continue from when truncated.
    next_line: Option<u64>,
}

pub(crate) struct FsRead {
    descriptor: ToolDescriptor,
}

impl FsRead {
    pub(crate) fn new() -> Result<Self, ToolError> {
        Ok(Self {
            descriptor: descriptor::<ReadInput, ReadOutput>(
                "fs.read",
                1,
                "Read a text file in the workspace, optionally a line range. Binary files \
                 return their size and SHA-256 instead of content.",
                RiskProfile::read_only(Action::FsRead),
                &["workspace:read"],
            )?,
        })
    }
}

fn positive(field: &str, value: Option<u64>) -> Result<(), ToolError> {
    match value {
        Some(0) => Err(ToolError::invalid(
            field,
            InputError::OutOfRange,
            "must be at least 1",
        )),
        _ => Ok(()),
    }
}

impl Tool for FsRead {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn plan(&self, ctx: &ToolContext, input: Value) -> Result<ToolPlan, ToolError> {
        let input: ReadInput = parse_input(input)?;
        positive("start_line", input.start_line)?;
        positive("max_lines", input.max_lines)?;
        positive("max_bytes", input.max_bytes)?;
        let path = parse_path("path", &input.path)?;
        let absolute = resolve(ctx, "path", &path, Access::Read)?;
        // `resolve` followed any in-workspace link, so this is the target's type.
        let target = stat(&absolute)?;
        require_file(&path, &target)?;
        let facts = PlanFacts {
            paths: vec![plan_path(
                Some(&path),
                &absolute,
                PathAccess::Read,
                &target,
                None,
            )],
            ..PlanFacts::default()
        };
        let summary = format!("read {}", path.as_str());
        ToolPlan::new(
            ctx,
            &self.descriptor,
            PlanParts::new(normalized(&input)?, summary).with_facts(facts),
        )
    }

    fn execute(
        &self,
        ctx: &ToolContext,
        plan: &ToolPlan,
        _permit: &ExecutionPermit,
    ) -> Result<ToolOutput, ToolError> {
        let input: ReadInput = parse_input(plan.input().clone())?;
        let path = parse_path("path", &input.path)?;
        let absolute = resolve(ctx, "path", &path, Access::Read)?;
        require_file(&path, &stat(&absolute)?)?;
        let max_bytes = input
            .max_bytes
            .unwrap_or(ctx.limits.max_read_bytes)
            .min(ctx.limits.max_read_bytes);
        let file =
            File::open(&absolute).map_err(|error| ToolError::io("cannot open the file", error))?;
        let size = file
            .metadata()
            .map_err(|error| ToolError::io("cannot inspect the file", error))?
            .len();
        let start_line = input.start_line.unwrap_or(1);
        let read = read_lines(
            ctx,
            file,
            size,
            start_line,
            input.max_lines.unwrap_or(u64::MAX),
            usize::try_from(max_bytes).unwrap_or(usize::MAX),
        )?;
        let (content, lossy_utf8) = match &read.content {
            None => (None, false),
            Some(bytes) => match String::from_utf8(bytes.clone()) {
                Ok(text) => (Some(text), false),
                Err(_) => (Some(String::from_utf8_lossy(bytes).into_owned()), true),
            },
        };
        let summary = if read.binary {
            format!("read {} (binary, {size} bytes)", path.as_str())
        } else {
            format!(
                "read {} lines {}-{} of a {size}-byte file",
                path.as_str(),
                start_line,
                read.end_line
            )
        };
        let output = ReadOutput {
            path: path.as_str().to_owned(),
            size,
            sha256: read.sha256,
            binary: read.binary,
            lossy_utf8,
            content,
            start_line,
            end_line: read.end_line,
            truncated: read.next_line.is_some(),
            next_line: read.next_line,
        };
        ToolOutput::typed(summary, &output)
    }
}

struct LinesRead {
    binary: bool,
    content: Option<Vec<u8>>,
    end_line: u64,
    next_line: Option<u64>,
    sha256: Option<String>,
}

/// Stream the file once: hash everything (up to [`MAX_HASH_BYTES`]) while keeping
/// at most `max_bytes` of the requested lines in memory. A line longer than the
/// remaining budget is cut on a UTF-8 boundary.
fn read_lines(
    ctx: &ToolContext,
    file: File,
    size: u64,
    start_line: u64,
    max_lines: u64,
    max_bytes: usize,
) -> Result<LinesRead, ToolError> {
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut hasher = (size <= MAX_HASH_BYTES).then(Sha256::new);
    let io_error = |error| ToolError::io("cannot read the file", error);
    let binary = loop {
        match reader.fill_buf() {
            Ok(head) => break looks_binary(head),
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(io_error(error)),
        }
    };
    let mut content = Vec::new();
    let mut line = 1u64;
    let mut returned = 0u64;
    let mut end_line = 0u64;
    // Absolute offset just past the last byte examined for content.
    let mut position = 0u64;
    // Where to resume: the line containing the first byte not returned.
    let mut resume = None;
    let mut done = binary;
    loop {
        crate::context::check_cancel(&ctx.cancel)?;
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(io_error(error)),
        };
        if chunk.is_empty() {
            break;
        }
        let length = chunk.len();
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(chunk);
        }
        let mut offset = 0;
        while offset < length && !done {
            let rest = &chunk[offset..];
            let (segment, ends_line) = match rest.iter().position(|byte| *byte == b'\n') {
                Some(end) => (&rest[..=end], true),
                None => (rest, false),
            };
            if line >= start_line {
                let room = max_bytes - content.len();
                if segment.len() > room {
                    content.extend_from_slice(&segment[..room]);
                    trim_partial_utf8(&mut content);
                    end_line = line;
                    resume = Some(line);
                    done = true;
                    break;
                }
                content.extend_from_slice(segment);
                end_line = line;
            }
            offset += segment.len();
            position += segment.len() as u64;
            if ends_line {
                if line >= start_line {
                    returned += 1;
                    if returned == max_lines {
                        done = true;
                    }
                }
                line += 1;
            }
        }
        reader.consume(length);
        if done && hasher.is_none() {
            break;
        }
    }
    let next_line = if binary {
        None
    } else {
        resume.or_else(|| (done && position < size).then_some(line))
    };
    Ok(LinesRead {
        binary,
        content: (!binary).then_some(content),
        end_line,
        next_line,
        sha256: hasher.map(|hasher| hex(&hasher.finalize())),
    })
}

/// Drop an incomplete trailing UTF-8 sequence left by a byte-budget cut.
fn trim_partial_utf8(content: &mut Vec<u8>) {
    if let Err(error) = std::str::from_utf8(content)
        && error.error_len().is_none()
    {
        content.truncate(error.valid_up_to());
    }
}
