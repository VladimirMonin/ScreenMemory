//! Pure identities and lossless revision values. No platform, persistence or transport IO.
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

pub const UUID_PATTERN: &str = "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";
// Decimal 0..=u64::MAX; bounded in the schema as well as the decoder (JS must not parse as Number).
pub const REVISION_PATTERN: &str = "^(?:0|[1-9][0-9]{0,18}|1[0-7][0-9]{18}|18[0-3][0-9]{17}|184[0-3][0-9]{16}|1844[0-5][0-9]{15}|18446[0-6][0-9]{14}|184467[0-3][0-9]{13}|1844674[0-3][0-9]{12}|184467440[0-6][0-9]{10}|1844674407[0-2][0-9]{9}|18446744073[0-6][0-9]{8}|1844674407370[0-8][0-9]{6}|18446744073709[0-4][0-9]{5}|184467440737095[0-4][0-9]{4}|18446744073709550[0-9]{3}|18446744073709551[0-5][0-9]{2}|1844674407370955160[0-9]{1}|1844674407370955161[0-4]|18446744073709551615)$";

macro_rules! identity {
    ($($name:ident),+ $(,)?) => {$(
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name(Uuid);
        impl $name {
            pub fn new(id: Uuid) -> Self { Self(id) }
            pub fn as_uuid(self) -> Uuid { self.0 }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.0.to_string())
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(d)?;
                let id = Uuid::parse_str(&raw).map_err(D::Error::custom)?;
                if raw != id.to_string() { return Err(D::Error::custom("noncanonical UUID")); }
                Ok(Self(id))
            }
        }
        impl JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> { stringify!($name).into() }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({"type": "string", "pattern": UUID_PATTERN, "minLength": 36, "maxLength": 36})
            }
        }
    )+};
}
identity!(
    LibraryId,
    RequestId,
    HostInstanceId,
    PrincipalId,
    CorrelationId,
    ReportId,
    ConsentId,
    EvidenceId
);

/// Decimal-string u64; JSON numbers cannot roundtrip through JavaScript beyond 2^53.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Revision(u64);
impl Revision {
    pub fn new(value: u64) -> Self {
        Self(value)
    }
    pub fn value(self) -> u64 {
        self.0
    }
}
impl Serialize for Revision {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for Revision {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        if raw.is_empty()
            || (raw.len() > 1 && raw.starts_with('0'))
            || !raw.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(D::Error::custom("noncanonical decimal revision"));
        }
        raw.parse::<u64>().map(Self).map_err(D::Error::custom)
    }
}
impl JsonSchema for Revision {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Revision".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        // The ECMA-262 pattern covers the full u64 bound without a lossy JSON number.
        json_schema!({"type": "string", "pattern": REVISION_PATTERN, "minLength": 1, "maxLength": 20})
    }
}
