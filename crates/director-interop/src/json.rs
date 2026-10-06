use crate::Error;
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::fmt;

pub const MAX_BYTES: usize = psf_guard_director_core::MAX_REQUEST_BYTES;
const MAX_DEPTH: usize = 32;
const MAX_ITEMS: usize = 4096;

struct Unique(Value);

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded JSON with unique object keys")
            }
            fn visit_bool<E>(self, v: bool) -> Result<Unique, E> {
                Ok(Unique(Value::Bool(v)))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Unique, E> {
                Ok(Unique(Value::Number(v.into())))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Unique, E> {
                Ok(Unique(Value::Number(v.into())))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Unique, E> {
                Number::from_f64(v)
                    .map(|n| Unique(Value::Number(n)))
                    .ok_or_else(|| E::custom("invalid number"))
            }
            fn visit_str<E>(self, v: &str) -> Result<Unique, E> {
                Ok(Unique(Value::String(v.into())))
            }
            fn visit_string<E>(self, v: String) -> Result<Unique, E> {
                Ok(Unique(Value::String(v)))
            }
            fn visit_unit<E>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut out = Vec::new();
                while let Some(Unique(v)) = a.next_element()? {
                    if out.len() == MAX_ITEMS {
                        return Err(serde::de::Error::custom("too many items"));
                    }
                    out.push(v);
                }
                Ok(Unique(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut out = Map::new();
                while let Some(key) = a.next_key::<String>()? {
                    if out.contains_key(&key) || out.len() == MAX_ITEMS {
                        return Err(serde::de::Error::custom("ambiguous or oversized object"));
                    }
                    out.insert(key, a.next_value::<Unique>()?.0);
                }
                Ok(Unique(Value::Object(out)))
            }
        }
        deserializer.deserialize_any(JsonVisitor)
    }
}

pub fn decode(bytes: &[u8]) -> Result<Value, Error> {
    if bytes.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    let Unique(value) = serde_json::from_slice(bytes).map_err(|_| Error::InvalidJson)?;
    let mut pending = vec![(&value, 0)];
    let mut count = 0;
    while let Some((node, depth)) = pending.pop() {
        count += 1;
        if depth > MAX_DEPTH || count > MAX_ITEMS {
            return Err(Error::LimitExceeded);
        }
        match node {
            Value::Array(items) => pending.extend(items.iter().map(|v| (v, depth + 1))),
            Value::Object(items) => pending.extend(items.values().map(|v| (v, depth + 1))),
            _ => {}
        }
    }
    Ok(value)
}

pub fn object(value: &Value) -> Result<&Map<String, Value>, Error> {
    value.as_object().ok_or(Error::InvalidValue)
}

pub fn text(value: &Value, maximum: usize) -> Result<String, Error> {
    let text = value.as_str().ok_or(Error::InvalidValue)?;
    if text.len() > maximum || text.chars().any(char::is_control) {
        return Err(Error::InvalidValue);
    }
    Ok(text.to_owned())
}

pub fn optional_text(map: &Map<String, Value>, key: &str) -> Result<Option<String>, Error> {
    map.get(key)
        .filter(|v| !v.is_null())
        .map(|v| text(v, 120))
        .transpose()
}

/// Older peers can send numeric strings. Blank/unreadable numbers stay unknown.
pub fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|v| v.is_finite())
}

pub fn ranged(value: &Value, minimum: f64, maximum: f64) -> Result<Option<f64>, Error> {
    number(value)
        .map(|v| {
            if v < minimum || v > maximum {
                Err(Error::InvalidValue)
            } else {
                Ok(v)
            }
        })
        .transpose()
}

pub fn integer(value: &Value, minimum: u32) -> Result<Option<u32>, Error> {
    ranged(value, f64::from(minimum), f64::from(u32::MAX))?
        .map(|v| {
            if v.fract() != 0.0 {
                Err(Error::InvalidValue)
            } else {
                Ok(v as u32)
            }
        })
        .transpose()
}

pub fn boolean(map: &Map<String, Value>, key: &str, default: bool) -> Result<bool, Error> {
    map.get(key)
        .map_or(Ok(default), |v| v.as_bool().ok_or(Error::InvalidValue))
}

pub fn array(value: &Value, maximum: usize) -> Result<&[Value], Error> {
    let items = value.as_array().ok_or(Error::InvalidValue)?;
    if items.len() > maximum {
        Err(Error::LimitExceeded)
    } else {
        Ok(items)
    }
}
