//! Closed manual-card/category command adapter, shared native API, no local resolver.
use crate::{ArgError, CliArgs, Command};
use sm_protocol::knowledge::*;
use std::{collections::BTreeMap, path::PathBuf};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeCommand(pub String);
fn bad() -> ArgError {
    ArgError::UnexpectedArgument
}
fn decode<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, ArgError> {
    serde_json::from_value(serde_json::Value::String(s.into())).map_err(|_| bad())
}
pub fn parse(argv: &[String]) -> Result<CliArgs, ArgError> {
    let at = argv.iter().position(|s| s == "knowledge").ok_or_else(bad)?;
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
    let take = |f: &mut BTreeMap<String, String>, k: &str| f.remove(k).ok_or_else(bad);
    let req = match action.as_str() {
        "read" => KnowledgeRequest::Read {
            record_id: decode(&take(&mut flags, "--record-id")?)?,
        },
        "categories" => KnowledgeRequest::Categories {},
        "category" => KnowledgeRequest::Category(
            serde_json::from_str(&take(&mut flags, "--request-json")?).map_err(|_| bad())?,
        ),
        "edit" => {
            let operation_id = decode(&take(&mut flags, "--operation-id")?)?;
            let record_id = decode(&take(&mut flags, "--record-id")?)?;
            let expected_record_revision = decode(&take(&mut flags, "--expected-revision")?)?;
            let expected_catalog_revision = decode(&take(&mut flags, "--catalog-revision")?)?;
            let expected_category_revision = flags
                .remove("--category-revision")
                .map(|s| decode(&s))
                .transpose()?;
            let mut patch = CardPatch::default();
            if let Some(s) = flags.remove("--title") {
                patch.title = FieldPatch::Set { value: decode(&s)? };
            }
            if let Some(s) = flags.remove("--description") {
                patch.description = FieldPatch::Set { value: decode(&s)? };
            }
            if let Some(s) = flags.remove("--category-id") {
                patch.category_id = FieldPatch::Set {
                    value: Some(decode(&s)?),
                };
            }
            if let Some(s) = flags.remove("--clear-category") {
                if s != "true" || !matches!(patch.category_id, FieldPatch::Keep) {
                    return Err(bad());
                }
                patch.category_id = FieldPatch::Set { value: None };
            }
            for (key, which) in [
                ("--inherit-title", 0),
                ("--inherit-description", 1),
                ("--inherit-category", 2),
            ] {
                if let Some(s) = flags.remove(key) {
                    if s != "true" {
                        return Err(bad());
                    }
                    match which {
                        0 if matches!(patch.title, FieldPatch::Keep) => {
                            patch.title = FieldPatch::Inherit
                        }
                        1 if matches!(patch.description, FieldPatch::Keep) => {
                            patch.description = FieldPatch::Inherit
                        }
                        2 if matches!(patch.category_id, FieldPatch::Keep) => {
                            patch.category_id = FieldPatch::Inherit
                        }
                        _ => return Err(bad()),
                    }
                }
            }
            KnowledgeRequest::Edit(CardEdit {
                operation_id,
                record_id,
                expected_record_revision,
                expected_catalog_revision,
                expected_category_revision,
                patch,
            })
        }
        _ => return Err(bad()),
    };
    if !flags.is_empty() {
        return Err(bad());
    }
    Ok(CliArgs {
        config: Some(config.ok_or(ArgError::MissingConfig)?),
        json: true,
        command: Command::Knowledge(KnowledgeCommand(
            serde_json::to_string(&req).map_err(|_| bad())?,
        )),
    })
}
pub async fn run(
    client: &sm_session::ApplicationClient,
    command: KnowledgeCommand,
) -> Result<KnowledgeReply, crate::ClientFailure> {
    client
        .knowledge(serde_json::from_str(&command.0).expect("parser typed request"))
        .await
        .map_err(crate::client_failure)
}
