//! Explicit image commands; no filesystem walks or decoder in the client.
use crate::args::{ArgError, CliArgs, Command};
use sm_protocol::images::{CanonicalProfile, DuplicatePolicy, ExportFormat, ImportControl};
use std::path::PathBuf;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageCommand {
    Import {
        paths: Vec<String>,
        batch: Option<String>,
        profile: CanonicalProfile,
        duplicates: DuplicatePolicy,
        wait_ms: u32,
    },
    Status {
        batch: String,
        wait_ms: u32,
    },
    Report {
        batch: String,
        after: u32,
    },
    Control {
        batch: String,
        control: ImportControl,
    },
    Export {
        record: String,
        operation: Option<String>,
        format: ExportFormat,
        destination: Option<String>,
        max_side: Option<u32>,
        background: Option<[u8; 3]>,
    },
    Metadata {
        record: String,
    },
}
pub fn parse(argv: &[String]) -> Result<CliArgs, ArgError> {
    let mut config = None;
    let mut json = false;
    let mut words = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--config" => {
                if config.is_some() {
                    return Err(ArgError::UnexpectedArgument);
                }
                i += 1;
                let p = PathBuf::from(argv.get(i).ok_or(ArgError::MissingConfig)?);
                if !crate::args::absolute_local(&p) {
                    return Err(ArgError::ConfigNotAbsolute);
                }
                config = Some(p);
            }
            "--json" => json = true,
            _ => words.push(argv[i].clone()),
        }
        i += 1;
    }
    if config.is_none() {
        return Err(ArgError::MissingConfig);
    }
    let verb = words.first().ok_or(ArgError::MissingCommand)?.as_str();
    let action = if verb == "import" && words.get(1).is_some_and(|w| !w.starts_with('-')) {
        words[1].as_str()
    } else {
        "start"
    };
    let mut paths = Vec::new();
    let mut batch = None;
    let mut record = None;
    let mut operation = None;
    let mut destination = None;
    let mut profile = CanonicalProfile::WebpExact;
    let mut duplicates = DuplicatePolicy::ReuseRecord;
    let mut format = ExportFormat::Png;
    let mut wait_ms = 0;
    let mut after = 0;
    let mut max_side = None;
    let mut background = None;
    let start = if action == "start" { 1 } else { 2 };
    if !(words.len() - start).is_multiple_of(2) {
        return Err(ArgError::UnexpectedArgument);
    }
    let mut seen = std::collections::HashSet::new();
    for pair in words[start..].as_chunks::<2>().0 {
        let key = pair[0].as_str();
        let value = pair[1].as_str();
        if key != "--path" && !seen.insert(key) {
            return Err(ArgError::UnexpectedArgument);
        }
        match key {
            "--path" => {
                if !crate::args::absolute_local(std::path::Path::new(value)) {
                    return Err(ArgError::OutputNotAbsolute);
                }
                paths.push(value.into());
            }
            "--batch-id" => batch = Some(identity(value)?),
            "--record-id" => record = Some(identity(value)?),
            "--operation-id" => operation = Some(identity(value)?),
            "--profile" => {
                profile = match value {
                    "webp_exact" => CanonicalProfile::WebpExact,
                    "avif_sdr90" => CanonicalProfile::AvifSdr90,
                    _ => return Err(ArgError::UnknownArgument),
                }
            }
            "--duplicates" => {
                duplicates = match value {
                    "reuse_record" => DuplicatePolicy::ReuseRecord,
                    "keep_both" => DuplicatePolicy::KeepBoth,
                    _ => return Err(ArgError::UnknownArgument),
                }
            }
            "--format" => {
                format = match value {
                    "png" => ExportFormat::Png,
                    "jpeg" => ExportFormat::Jpeg,
                    "webp" => ExportFormat::Webp,
                    "avif" => ExportFormat::Avif,
                    _ => return Err(ArgError::UnknownArgument),
                }
            }
            "--output" => {
                if !crate::args::absolute_local(std::path::Path::new(value)) {
                    return Err(ArgError::OutputNotAbsolute);
                }
                destination = Some(value.into());
            }
            "--wait-ms" => {
                wait_ms = value.parse().map_err(|_| ArgError::UnknownArgument)?;
                if wait_ms > 180000 {
                    return Err(ArgError::UnknownArgument);
                }
            }
            "--after-ordinal" => after = value.parse().map_err(|_| ArgError::UnknownArgument)?,
            "--max-side" => {
                let n = value.parse().map_err(|_| ArgError::UnknownArgument)?;
                if !matches!(n, 320 | 1000 | 1024 | 2048) {
                    return Err(ArgError::UnknownArgument);
                }
                max_side = Some(n);
            }
            "--jpeg-background" => {
                let rgb = value
                    .split(',')
                    .map(str::parse::<u8>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| ArgError::UnknownArgument)?;
                background = Some(rgb.try_into().map_err(|_| ArgError::UnknownArgument)?);
            }
            _ => return Err(ArgError::UnknownArgument),
        }
    }
    let allowed: &[&str] = match (verb, action) {
        ("import", "start") => &[
            "--path",
            "--batch-id",
            "--profile",
            "--duplicates",
            "--wait-ms",
        ],
        ("import", "status") => &["--batch-id", "--wait-ms"],
        ("import", "report") => &["--batch-id", "--after-ordinal"],
        ("import", "pause" | "cancel" | "resume" | "retry") => &["--batch-id"],
        ("export", _) => &[
            "--record-id",
            "--operation-id",
            "--format",
            "--output",
            "--max-side",
            "--jpeg-background",
        ],
        ("metadata", _) => &["--record-id"],
        _ => return Err(ArgError::UnknownArgument),
    };
    if words[start..]
        .as_chunks::<2>()
        .0
        .iter()
        .any(|p| !allowed.contains(&p[0].as_str()))
    {
        return Err(ArgError::UnexpectedArgument);
    }
    let required = |v: Option<String>| v.ok_or(ArgError::UnexpectedArgument);
    let command = match (verb, action) {
        ("import", "start") => {
            if paths.is_empty() || paths.len() > 128 {
                return Err(ArgError::UnexpectedArgument);
            }
            ImageCommand::Import {
                paths,
                batch,
                profile,
                duplicates,
                wait_ms,
            }
        }
        ("import", "status") => ImageCommand::Status {
            batch: required(batch)?,
            wait_ms,
        },
        ("import", "report") => ImageCommand::Report {
            batch: required(batch)?,
            after,
        },
        ("import", "pause" | "cancel" | "resume" | "retry") => ImageCommand::Control {
            batch: required(batch)?,
            control: match action {
                "pause" => ImportControl::Pause,
                "cancel" => ImportControl::CancelProcessing,
                "resume" => ImportControl::Resume,
                _ => ImportControl::Retry,
            },
        },
        ("export", _) => ImageCommand::Export {
            record: required(record)?,
            operation,
            format,
            destination,
            max_side,
            background,
        },
        ("metadata", _) => ImageCommand::Metadata {
            record: required(record)?,
        },
        _ => return Err(ArgError::UnknownArgument),
    };
    Ok(CliArgs {
        config,
        json,
        command: Command::Image(command),
    })
}
fn identity(s: &str) -> Result<String, ArgError> {
    let id = uuid::Uuid::parse_str(s).map_err(|_| ArgError::UnknownArgument)?;
    if id.to_string() != s || id.get_version_num() != 4 {
        return Err(ArgError::UnknownArgument);
    }
    Ok(s.into())
}
