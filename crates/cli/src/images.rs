use crate::{client_failure, image_args::ImageCommand, ClientFailure};
use serde_json::{json, Value};
use sm_domain::{ImportBatchId, OperationId, RecordId};
use sm_protocol::images::*;
use sm_session::ApplicationClient;
use std::time::{Duration, Instant};
use uuid::Uuid;

pub async fn run(client: &ApplicationClient, command: ImageCommand) -> (u8, Value) {
    let mut identity = Value::Null;
    let result: Result<(u8, Value), ClientFailure> = async {
        match command {
            ImageCommand::Import {
                paths,
                batch,
                profile,
                duplicates,
                wait_ms,
            } => {
                let batch_id = ImportBatchId::new(
                    batch
                        .map(|s| Uuid::parse_str(&s).expect("parsed UUID"))
                        .unwrap_or_else(Uuid::new_v4),
                );
                identity = json!({"batch_id":batch_id});
                let report = client
                    .import_paths(ImportRequest {
                        batch_id,
                        source_paths: paths,
                        profile,
                        duplicates,
                    })
                    .await
                    .map_err(client_failure)?;
                wait(client, report, wait_ms).await
            }
            ImageCommand::Status { batch, wait_ms } => {
                let batch_id = ImportBatchId::new(Uuid::parse_str(&batch).expect("parsed UUID"));
                identity = json!({"batch_id":batch_id});
                let report = client
                    .import_status(ImportBatchRequest { batch_id })
                    .await
                    .map_err(client_failure)?;
                wait(client, report, wait_ms).await
            }
            ImageCommand::Report { batch, after } => {
                let batch_id = ImportBatchId::new(Uuid::parse_str(&batch).expect("parsed UUID"));
                identity = json!({"batch_id":batch_id});
                let report = client
                    .import_report_page(ImportReportPageRequest {
                        batch_id,
                        after_ordinal: after,
                    })
                    .await
                    .map_err(client_failure)?;
                Ok((0, json!(report)))
            }
            ImageCommand::Control { batch, control } => {
                let batch_id = ImportBatchId::new(Uuid::parse_str(&batch).expect("parsed UUID"));
                identity = json!({"batch_id":batch_id});
                let report = client
                    .import_control(ImportControlRequest { batch_id, control })
                    .await
                    .map_err(client_failure)?;
                Ok((0, json!(report)))
            }
            ImageCommand::Metadata { record } => {
                let record_id = RecordId::new(Uuid::parse_str(&record).expect("parsed UUID"));
                identity = json!({"record_id":record_id});
                Ok((
                    0,
                    json!(client
                        .record_metadata(RecordMetadataRequest { record_id })
                        .await
                        .map_err(client_failure)?),
                ))
            }
            ImageCommand::Export {
                record,
                operation,
                format,
                destination,
                max_side,
                background,
            } => {
                let operation_id = OperationId::new(
                    operation
                        .map(|s| Uuid::parse_str(&s).expect("parsed UUID"))
                        .unwrap_or_else(Uuid::new_v4),
                );
                identity = json!({"operation_id":operation_id});
                let receipt = client
                    .export_image(
                        ExportRequest {
                            operation_id,
                            record_id: RecordId::new(
                                Uuid::parse_str(&record).expect("parsed UUID"),
                            ),
                            asset_id: None,
                            format,
                            max_side,
                            jpeg_background: background,
                            destination_path: destination,
                            metadata_policy: ExportMetadataPolicy::StripPrivate,
                        },
                        None,
                    )
                    .await
                    .map_err(client_failure)?;
                Ok((if receipt.partial { 7 } else { 0 }, json!(receipt)))
            }
        }
    }
    .await;
    match result {
        Ok(value) => value,
        Err(failure) => (
            failure.exit,
            json!({"identity":identity,"safe_code":failure.code,"retryable":failure.retryable,"completed":false}),
        ),
    }
}
async fn wait(
    client: &ApplicationClient,
    mut report: ImportReport,
    ms: u32,
) -> Result<(u8, Value), ClientFailure> {
    if ms == 0 {
        return Ok((0, json!(report)));
    }
    let end = Instant::now() + Duration::from_millis(u64::from(ms));
    loop {
        if !report.active_work {
            return Ok((
                if matches!(report.state, ImportBatchState::Completed) {
                    0
                } else {
                    7
                },
                json!(report),
            ));
        }
        if Instant::now() >= end {
            return Ok((
                crate::EXIT_WAIT,
                json!({"batch_id":report.batch_id,"wait_timed_out":true,"report":report}),
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        report = client
            .import_status(ImportBatchRequest {
                batch_id: report.batch_id,
            })
            .await
            .map_err(client_failure)?;
    }
}
