//! Useful native commands over the same typed authenticated application client as Tauri.
use crate::{ArgError, CliArgs, Command};
use sm_domain::{
    ModelProfileId, OperationId, ProcessingBatchId, ProcessingJobId, RecordId, Revision,
};
use sm_protocol::processing::*;
use std::{collections::BTreeMap, path::PathBuf};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessingCommand {
    Request(String),
    Policy {
        mode: Option<String>,
        background: Option<String>,
        query: Option<String>,
        paused: Option<bool>,
        revision: Option<u64>,
        operation: String,
    },
}
fn bad() -> ArgError {
    ArgError::UnexpectedArgument
}
fn id(s: &str) -> Result<Uuid, ArgError> {
    let u = Uuid::parse_str(s).map_err(|_| bad())?;
    if u.get_version() != Some(uuid::Version::Random) || s != u.to_string() {
        return Err(bad());
    }
    Ok(u)
}
fn rev(s: &str) -> Result<Revision, ArgError> {
    s.parse::<u64>()
        .ok()
        .filter(|v| *v <= i64::MAX as u64)
        .map(Revision::new)
        .ok_or_else(bad)
}
pub fn parse(argv: &[String]) -> Result<CliArgs, ArgError> {
    let at = argv
        .iter()
        .position(|s| s == "processing")
        .ok_or_else(bad)?;
    let action = argv.get(at + 1).ok_or_else(bad)?;
    let mut flags = BTreeMap::new();
    let mut config = None;
    let mut i = 0;
    while i < argv.len() {
        if i == at {
            i += 2;
            continue;
        }
        if argv[i] == "--json" {
            i += 1;
            continue;
        }
        let key = &argv[i];
        if !key.starts_with("--") {
            return Err(bad());
        }
        let value = argv.get(i + 1).ok_or_else(bad)?;
        if key == "--config" {
            if config.is_some() {
                return Err(bad());
            }
            let p = PathBuf::from(value);
            if !crate::args::absolute_local(&p) {
                return Err(ArgError::ConfigNotAbsolute);
            }
            config = Some(p);
        } else if flags.insert(key.clone(), value.clone()).is_some() {
            return Err(bad());
        }
        i += 2;
    }
    let required = |f: &mut BTreeMap<String, String>, k: &str| f.remove(k).ok_or_else(bad);
    let request = match action.as_str() {
        "status" | "jobs" => ProcessingRequest::Overview {
            before: flags
                .remove("--before")
                .map(|v| id(&v).map(ProcessingJobId::new))
                .transpose()?,
        },
        "inventory" => ProcessingRequest::Inventory {},
        "reconcile" => ProcessingRequest::Reconcile {},
        "policy" => {
            if flags.is_empty() {
                ProcessingRequest::Policy {}
            } else {
                let mode = flags.remove("--mode");
                let background = flags.remove("--background");
                let query = flags.remove("--query");
                if mode
                    .as_ref()
                    .is_some_and(|v| !matches!(v.as_str(), "automatic" | "manual" | "off"))
                    || background
                        .as_ref()
                        .is_some_and(|v| !matches!(v.as_str(), "allow" | "forbid"))
                    || query
                        .as_ref()
                        .is_some_and(|v| !matches!(v.as_str(), "ask" | "allow" | "forbid"))
                {
                    return Err(bad());
                }
                let paused = flags
                    .remove("--paused")
                    .map(|v| v.parse::<bool>().map_err(|_| bad()))
                    .transpose()?;
                let revision = flags
                    .remove("--revision")
                    .map(|v| rev(&v).map(|r| r.value()))
                    .transpose()?;
                let operation = flags
                    .remove("--operation-id")
                    .map(|s| id(&s).map(|u| u.to_string()))
                    .transpose()?
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                if !flags.is_empty() {
                    return Err(bad());
                }
                return Ok(CliArgs {
                    config: Some(config.ok_or(ArgError::MissingConfig)?),
                    json: true,
                    command: Command::Processing(ProcessingCommand::Policy {
                        mode,
                        background,
                        query,
                        paused,
                        revision,
                        operation,
                    }),
                });
            }
        }
        "profile" => ProcessingRequest::Profile {
            id: ModelProfileId::new(id(&required(&mut flags, "--id")?)?),
        },
        "save-profile" => {
            let raw = required(&mut flags, "--profile-json")?;
            if raw.len() > 8192 {
                return Err(bad());
            }
            let value: ModelProfile = serde_json::from_str(&raw).map_err(|_| bad())?;
            ProcessingRequest::SaveProfile {
                operation_id: OperationId::new(
                    flags
                        .remove("--operation-id")
                        .map(|v| id(&v))
                        .transpose()?
                        .unwrap_or_else(Uuid::new_v4),
                ),
                expected_revision: rev(&required(&mut flags, "--revision")?)?,
                value,
            }
        }
        "self-test" | "background-self-test" | "analyze" => ProcessingRequest::Plan {
            operation_id: OperationId::new(id(&required(&mut flags, "--operation-id")?)?),
            expected_policy_revision: rev(&required(&mut flags, "--policy-revision")?)?,
            records: vec![RecordId::new(id(&required(&mut flags, "--record-id")?)?)],
            kind: match action.as_str() {
                "self-test" => WorkKind::TechnicalOwnerSelfTest,
                "background-self-test" => WorkKind::TechnicalOwnerBackgroundSelfTest,
                _ => WorkKind::Analyze,
            },
            profile_id: flags
                .remove("--profile-id")
                .map(|v| id(&v).map(ModelProfileId::new))
                .transpose()?,
        },
        "batch" => ProcessingRequest::Batch {
            id: ProcessingBatchId::new(id(&required(&mut flags, "--id")?)?),
        },
        "pause" | "resume" | "cancel-batch" => ProcessingRequest::ControlBatch {
            id: ProcessingBatchId::new(id(&required(&mut flags, "--id")?)?),
            expected_revision: rev(&required(&mut flags, "--revision")?)?,
            control: match action.as_str() {
                "pause" => BatchControl::Pause,
                "resume" => BatchControl::Resume,
                _ => BatchControl::Cancel,
            },
        },
        "cancel" | "retry" => ProcessingRequest::ControlJob {
            id: ProcessingJobId::new(id(&required(&mut flags, "--id")?)?),
            expected_revision: rev(&required(&mut flags, "--revision")?)?,
            control: if action == "retry" {
                JobControl::Retry
            } else {
                JobControl::Cancel
            },
        },
        "execution" => ProcessingRequest::Execution {
            id: ProcessingJobId::new(id(&required(&mut flags, "--id")?)?),
        },
        _ => return Err(bad()),
    };
    if !flags.is_empty() {
        return Err(bad());
    }
    Ok(CliArgs {
        config: Some(config.ok_or(ArgError::MissingConfig)?),
        json: true,
        command: Command::Processing(ProcessingCommand::Request(
            serde_json::to_string(&request).map_err(|_| bad())?,
        )),
    })
}
pub async fn run(
    client: &sm_session::ApplicationClient,
    command: ProcessingCommand,
) -> Result<ProcessingReply, crate::ClientFailure> {
    let request = match command {
        ProcessingCommand::Request(json) => {
            serde_json::from_str(&json).expect("parser constructed typed request")
        }
        ProcessingCommand::Policy {
            mode,
            background,
            query,
            paused,
            revision,
            operation,
        } => {
            let old = client
                .processing(ProcessingRequest::Policy {})
                .await
                .map_err(crate::client_failure)?;
            let ProcessingReply::Policy(old) = old else {
                unreachable!("typed client")
            };
            let mut value = old.value;
            if let Some(m) = mode {
                value.mode = match m.as_str() {
                    "automatic" => ProcessingMode::Automatic,
                    "manual" => ProcessingMode::Manual,
                    _ => ProcessingMode::Off,
                };
            }
            if let Some(b) = background {
                value.background_loading = if b == "allow" {
                    LoadPermission::Allow
                } else {
                    LoadPermission::Forbid
                };
            }
            if let Some(q) = query {
                value.query_loading = match q.as_str() {
                    "allow" => QueryLoadPermission::Allow,
                    "forbid" => QueryLoadPermission::Forbid,
                    _ => QueryLoadPermission::Ask,
                };
            }
            if let Some(p) = paused {
                value.queue_paused = p;
            }
            ProcessingRequest::UpdatePolicy {
                operation_id: OperationId::new(Uuid::parse_str(&operation).expect("checked")),
                expected_revision: Revision::new(revision.unwrap_or(old.revision.value())),
                value,
            }
        }
    };
    client
        .processing(request)
        .await
        .map_err(crate::client_failure)
}
