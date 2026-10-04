//! Typed JSON CLI search adapter; no local SQL, source resolution or model calls.
use crate::{ArgError, CliArgs, Command};
use sm_protocol::{knowledge::KnowledgeId, search::*};
use std::{collections::BTreeMap, path::PathBuf};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchCommand(pub String);
fn bad() -> ArgError {
    ArgError::UnexpectedArgument
}
fn id(s: &str) -> Result<KnowledgeId, ArgError> {
    serde_json::from_value(serde_json::Value::String(s.into())).map_err(|_| bad())
}
pub fn parse(argv: &[String]) -> Result<CliArgs, ArgError> {
    let at = argv.iter().position(|s| s == "search").ok_or_else(bad)?;
    let action = argv
        .get(at + 1)
        .filter(|s| !s.starts_with("--"))
        .map_or("start", String::as_str);
    let mut flags = BTreeMap::new();
    let mut config = None;
    let mut i = 0;
    while i < argv.len() {
        if i == at {
            i += if action == "start" { 1 } else { 2 };
            continue;
        }
        if argv[i] == "--json" {
            i += 1;
            continue;
        }
        let key = &argv[i];
        let value = argv.get(i + 1).ok_or_else(bad)?;
        if !key.starts_with("--") {
            return Err(bad());
        }
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
    let req = match action {
        "page" => SearchRequest::Page {
            cursor: id(&take(&mut flags, "--cursor")?)?,
        },
        "cancel" => SearchRequest::Cancel {
            search_id: id(&take(&mut flags, "--search-id")?)?,
        },
        "start" => {
            let r = SearchStart {
                search_id: flags
                    .remove("--search-id")
                    .map(|s| id(&s))
                    .transpose()?
                    .unwrap_or(KnowledgeId(uuid::Uuid::new_v4())),
                text: flags.remove("--text").unwrap_or_default(),
                filters: flags
                    .remove("--filters-json")
                    .map(|s| serde_json::from_str(&s).map_err(|_| bad()))
                    .transpose()?
                    .unwrap_or_default(),
                limit: flags
                    .remove("--limit")
                    .map(|s| s.parse().map_err(|_| bad()))
                    .transpose()?
                    .unwrap_or(30),
                candidate_depth: flags
                    .remove("--candidate-depth")
                    .map(|s| s.parse().map_err(|_| bad()))
                    .transpose()?
                    .unwrap_or(100),
            };
            // Closed typed shape/limits here; unsupported filters remain a shared service refusal.
            if r.text.len() > 4096
                || r.text.chars().count() > 1024
                || !(1..=200).contains(&r.limit)
                || !(100..=2000).contains(&r.candidate_depth)
                || r.limit > r.candidate_depth
            {
                return Err(bad());
            }
            SearchRequest::Start(r)
        }
        _ => return Err(bad()),
    };
    if !flags.is_empty() {
        return Err(bad());
    }
    Ok(CliArgs {
        config: Some(config.ok_or(ArgError::MissingConfig)?),
        json: true,
        command: Command::Search(SearchCommand(
            serde_json::to_string(&req).map_err(|_| bad())?,
        )),
    })
}
pub async fn run(
    client: &sm_session::ApplicationClient,
    command: SearchCommand,
) -> Result<SearchReply, crate::ClientFailure> {
    client
        .search(serde_json::from_str(&command.0).expect("parser typed request"))
        .await
        .map_err(crate::client_failure)
}
