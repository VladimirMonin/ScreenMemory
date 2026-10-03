use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sm_image::{ImageError, Options, Report, ABI, MAX_INPUT_BYTES, MAX_OUTPUT_BYTES};
use std::io::{Read, Write};
use uuid::Uuid;

pub const MAX_HEADER: usize = 256 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub abi: String,
    pub operation_id: Uuid,
    pub input_sha256: [u8; 32],
    pub input_length: u64,
    pub options: Options,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub abi: String,
    pub operation_id: Uuid,
    pub report: Option<Report>,
    pub error: Option<ImageError>,
    pub output_length: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    abi: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Accepted {
    operation_id: Uuid,
    input_sha256: [u8; 32],
}
pub(crate) fn read_accepted<R: Read>(r: &mut R, id: Uuid, hash: [u8; 32]) -> std::io::Result<()> {
    let a: Accepted = read_header(r)?;
    if a.operation_id != id || a.input_sha256 != hash {
        return Err(invalid());
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "phase", deny_unknown_fields)]
enum Phase {
    Encoding { operation_id: Uuid },
}
fn invalid() -> std::io::Error {
    std::io::ErrorKind::InvalidData.into()
}
pub(crate) fn write_header<W: Write, T: Serialize>(w: &mut W, value: &T) -> std::io::Result<()> {
    let json = serde_json::to_vec(value).map_err(|_| invalid())?;
    if json.len() > MAX_HEADER {
        return Err(invalid());
    }
    w.write_all(&(json.len() as u32).to_le_bytes())?;
    w.write_all(&json)?;
    w.flush()
}
pub(crate) fn read_header<R: Read, T: DeserializeOwned>(r: &mut R) -> std::io::Result<T> {
    let mut length = [0; 4];
    r.read_exact(&mut length)?;
    let n = u32::from_le_bytes(length) as usize;
    if n == 0 || n > MAX_HEADER {
        return Err(invalid());
    }
    let mut json = vec![0; n];
    r.read_exact(&mut json)?;
    serde_json::from_slice(&json).map_err(|_| invalid())
}
pub(crate) fn read_hello<R: Read>(r: &mut R) -> std::io::Result<()> {
    let h: Hello = read_header(r)?;
    if h.abi != ABI {
        return Err(invalid());
    }
    Ok(())
}
pub(crate) fn read_encoding<R: Read>(r: &mut R, id: Uuid) -> std::io::Result<()> {
    match read_header(r)? {
        Phase::Encoding { operation_id } if operation_id == id => Ok(()),
        _ => Err(invalid()),
    }
}
pub(crate) fn read_response<R: Read>(r: &mut R) -> std::io::Result<(Response, Vec<u8>)> {
    let header: Response = read_header(r)?;
    if header.output_length > MAX_OUTPUT_BYTES as u64 {
        return Err(invalid());
    }
    let mut bytes = vec![0; header.output_length as usize];
    r.read_exact(&mut bytes)?;
    let mut extra = [0; 1];
    if r.read(&mut extra)? != 0 {
        return Err(invalid());
    }
    Ok((header, bytes))
}
/// Exactly one operation per child. The supervisor assigns containment before it
/// sends the first request, so no bitstream/native allocation precedes admission.
pub fn serve<R: Read, W: Write>(mut r: R, mut w: W) -> std::io::Result<()> {
    write_header(&mut w, &Hello { abi: ABI.into() })?;
    let request: Request = read_header(&mut r)?;
    if request.abi != ABI
        || request.operation_id.is_nil()
        || request.input_length == 0
        || request.input_length > MAX_INPUT_BYTES as u64
    {
        return Err(invalid());
    }
    let mut input = vec![0; request.input_length as usize];
    r.read_exact(&mut input)?;
    if sm_image::sha256(&input) != request.input_sha256 {
        return Err(invalid());
    }
    write_header(
        &mut w,
        &Accepted {
            operation_id: request.operation_id,
            input_sha256: request.input_sha256,
        },
    )?;
    #[cfg(feature = "worker-test-seams")]
    match input.as_slice() {
        b"SM-S03-TEST-HANG" => loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        },
        b"SM-S03-TEST-CRASH" => std::process::exit(7),
        b"SM-S03-TEST-OOM" => {
            let mut retained = Vec::new();
            loop {
                let mut v = vec![0u8; 64 * 1024 * 1024];
                for n in v.iter_mut().step_by(4096) {
                    *n = 1;
                }
                retained.push(v);
                std::hint::black_box(&retained);
            }
        }
        _ => {}
    }
    let mut notified = false;
    let result = sm_image::process_with_observer(&input, &request.options, || {
        notified = true;
        write_header(
            &mut w,
            &Phase::Encoding {
                operation_id: request.operation_id,
            },
        )
        .map_err(|_| ImageError::CodecFailed)
    });
    if !notified {
        write_header(
            &mut w,
            &Phase::Encoding {
                operation_id: request.operation_id,
            },
        )?;
    }
    let (response, bytes) = match result {
        Ok(c) => {
            let (bytes, report) = c.into_parts();
            (
                Response {
                    abi: ABI.into(),
                    operation_id: request.operation_id,
                    output_length: bytes.len() as u64,
                    report: Some(report),
                    error: None,
                },
                bytes,
            )
        }
        Err(error) => (
            Response {
                abi: ABI.into(),
                operation_id: request.operation_id,
                output_length: 0,
                report: None,
                error: Some(error),
            },
            Vec::new(),
        ),
    };
    write_header(&mut w, &response)?;
    w.write_all(&bytes)?;
    w.flush()
}
