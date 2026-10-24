use super::{require, Result};
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fmt;
use unicode_normalization::UnicodeNormalization;

pub fn bytes_digest(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(7 + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}
pub fn canonical(value: &Value) -> Result<Vec<u8>> {
    fn normalize(v: &Value) -> Result<Value> {
        Ok(match v {
            Value::String(s) => Value::String(s.nfc().collect()),
            Value::Array(a) => Value::Array(a.iter().map(normalize).collect::<Result<_>>()?),
            Value::Object(o) => {
                let mut normalized = Map::new();
                for (key, child) in o {
                    let key: String = key.nfc().collect();
                    require(
                        !normalized.contains_key(&key),
                        "IDENTITY",
                        "NFC-normalized keys collide",
                    )?;
                    normalized.insert(key, normalize(child)?);
                }
                Value::Object(normalized)
            }
            Value::Number(n) => {
                require(
                    n.is_i64() || n.is_u64(),
                    "IDENTITY",
                    "Floating-point identity input",
                )?;
                v.clone()
            }
            _ => v.clone(),
        })
    }
    Ok(serde_json::to_vec(&normalize(value)?)?)
}
pub fn digest(value: &Value) -> Result<String> {
    Ok(bytes_digest(&canonical(value)?))
}
pub fn id(kind: &str, value: &Value) -> Result<String> {
    Ok(format!(
        "{kind}:{}",
        digest(value)?.trim_start_matches("sha256:")
    ))
}

// serde_json normally accepts duplicate keys. Reject them at every nesting level before validation.
struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON value with unique keys")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Strict, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom("Nonfinite number"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_string<E: serde::de::Error>(
                self,
                v: String,
            ) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = a.next_element::<Strict>()? {
                    out.push(v.0);
                }
                Ok(Strict(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut out = Map::new();
                while let Some((k, v)) = a.next_entry::<String, Strict>()? {
                    if out.insert(k.clone(), v.0).is_some() {
                        return Err(serde::de::Error::custom(format!("Duplicate JSON key: {k}")));
                    }
                }
                Ok(Strict(Value::Object(out)))
            }
        }
        d.deserialize_any(V)
    }
}
pub fn parse(bytes: &[u8]) -> Result<Value> {
    Ok(serde_json::from_slice::<Strict>(bytes)?.0)
}
