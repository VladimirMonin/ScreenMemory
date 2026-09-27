//! `cargo run -p sm-protocol --example generate_schemas -- --output <new-dir>`
//! `cargo run -p sm-protocol --example generate_schemas -- --check schemas`
use schemars::{generate::SchemaSettings, JsonSchema};
use serde_json::Value;
use sm_protocol::{
    application::{KnownParams, RequestEnvelope, Response},
    bridge::{BridgeProbeRequest, BridgeResponse},
    errors::ErrorCode,
};
use std::{env, fs, path::Path};

fn schema<T: JsonSchema>() -> Value {
    let generator = SchemaSettings::draft2020_12().into_generator();
    serde_json::to_value(generator.into_root_schema_for::<T>()).expect("schema serializes")
}
#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(untagged)]
enum ApplicationFrame {
    Request(RequestEnvelope),
    Response(Response),
}
#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(untagged)]
enum BridgeFrame {
    Request(BridgeProbeRequest),
    Response(BridgeResponse),
}
fn rendered() -> Vec<(&'static str, Vec<u8>)> {
    let values = [
        ("application-v1.schema.json", schema::<ApplicationFrame>()),
        (
            "application-methods-v1.schema.json",
            schema::<KnownParams>(),
        ),
        ("bridge-v1.schema.json", schema::<BridgeFrame>()),
        ("foundation-errors.schema.json", schema::<ErrorCode>()),
    ];
    values
        .into_iter()
        .map(|(name, value)| {
            let mut bytes = serde_json::to_vec_pretty(&value).expect("JSON");
            bytes.push(b'\n');
            (name, bytes)
        })
        .collect()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 3 || !matches!(args[1].as_str(), "--output" | "--check") {
        return Err(
            "usage: generate_schemas (--output NEW_DIRECTORY | --check EXISTING_DIRECTORY)".into(),
        );
    }
    let dir = Path::new(&args[2]);
    let files = rendered();
    if args[1] == "--output" {
        fs::create_dir(dir)?; // must be a newly created directory
        for (name, bytes) in files {
            fs::write(dir.join(name), bytes)?;
        }
    } else {
        for (name, bytes) in files {
            if fs::read(dir.join(name))? != bytes {
                return Err(format!("schema differs: {name}").into());
            }
        }
    }
    Ok(())
}
