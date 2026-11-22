//! 包内 JSON 不能在丢掉重名键之后才校验；主题只返回允许的字段，绝不把原文拼进宿主页样式。
use super::{Problem, fail};
use aproxy_ipc::{
    desktop::{ExtField as Field, ExtKind, InvalidReason as Why, Permission},
    redact::Secrets,
};
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::{collections::BTreeSet, fmt, sync::LazyLock};

struct Bounded(u8);
impl<'de> DeserializeSeed<'de> for Bounded {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Bounded {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded JSON")
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("number"))
    }
    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Value, A::Error> {
        if self.0 >= 8 {
            return Err(de::Error::custom("depth"));
        }
        let mut m = Map::new();
        while let Some(k) = a.next_key::<String>()? {
            if m.contains_key(&k) {
                return Err(de::Error::custom("duplicate"));
            }
            m.insert(k, a.next_value_seed(Bounded(self.0 + 1))?);
        }
        Ok(m.into())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Value, A::Error> {
        if self.0 >= 8 {
            return Err(de::Error::custom("depth"));
        }
        let mut v = Vec::new();
        while let Some(e) = a.next_element_seed(Bounded(self.0 + 1))? {
            v.push(e)
        }
        Ok(v.into())
    }
}
pub(super) fn object(s: &str) -> Option<Map<String, Value>> {
    let mut d = serde_json::Deserializer::from_str(s);
    let Value::Object(m) = Bounded(0).deserialize(&mut d).ok()? else {
        return None;
    };
    d.end().ok()?;
    Some(m)
}
fn clean(v: &str) -> String {
    v.chars().filter(|c|!c.is_control() && !matches!(*c,'\u{061c}'|'\u{200e}'|'\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')).collect::<String>().trim().to_owned()
}
pub(super) fn key(v: &str, limit: usize) -> bool {
    let b = v.as_bytes();
    b.len() <= limit
        && b.first().is_some_and(u8::is_ascii_lowercase)
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && !reserved(v)
}
fn reserved(v: &str) -> bool {
    let stem = v.split('.').next().unwrap_or_default();
    matches!(
        stem,
        "con" | "prn" | "aux" | "nul" | "clock$" | "conin$" | "conout$"
    ) || (stem.len() == 4
        && (stem.starts_with("com") || stem.starts_with("lpt"))
        && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
pub(super) fn safe_file(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && !v.starts_with('.')
        && !v.ends_with('.')
        && !reserved(v)
        && v.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}
pub(super) struct Manifest {
    pub name: String,
    pub version: String,
    pub kind: ExtKind,
    pub permissions: Vec<Permission>,
}
pub(super) fn text(bytes: &[u8], field: Field) -> Result<&str, Problem> {
    std::str::from_utf8(bytes)
        .ok()
        .filter(|s| !s.starts_with('\u{feff}') && !s.contains('\0') && !s.trim().is_empty())
        .ok_or(fail(Why::Text, field))
}
pub(super) fn manifest(bytes: &[u8], id: &str, secrets: &Secrets) -> Result<Manifest, Problem> {
    let m = object(text(bytes, Field::Manifest)?).ok_or(fail(Why::Json, Field::Manifest))?;
    if m.get("manifestVersion").and_then(Value::as_u64) != Some(1) {
        return Err(fail(Why::ManifestVersion, Field::ManifestVersion));
    }
    let get = |k, f| m.get(k).and_then(Value::as_str).ok_or(fail(Why::Field, f));
    if get("id", Field::Id)? != id {
        return Err(fail(Why::Field, Field::Id));
    }
    let raw = get("name", Field::Name)?;
    let name = clean(raw);
    if raw.len() > 256 || !(1..=64).contains(&name.chars().count()) {
        return Err(fail(Why::Field, Field::Name));
    }
    let version = get("version", Field::Version)?;
    if version.len() > 64 || semver::Version::parse(version).is_err() {
        return Err(fail(Why::Field, Field::Version));
    }
    if [id, &name, version]
        .iter()
        .any(|s| secrets.find(s.as_bytes()).is_some())
    {
        return Err(fail(Why::PrivateData, Field::Manifest));
    }
    let kind: ExtKind = serde_json::from_value(m.get("kind").cloned().unwrap_or_default())
        .map_err(|_| fail(Why::Field, Field::Kind))?;
    let entry = get("entry", Field::Entry)?;
    if !safe_file(entry) {
        return Err(fail(Why::Path, Field::Entry));
    }
    if entry
        != if kind == ExtKind::Theme {
            "theme.txt"
        } else {
            "main.mjs"
        }
    {
        return Err(fail(Why::Field, Field::Entry));
    }
    let permissions: Vec<Permission> =
        serde_json::from_value(m.get("permissions").cloned().unwrap_or_default())
            .map_err(|_| fail(Why::Field, Field::Permissions))?;
    if permissions.len() > 4
        || permissions.iter().collect::<BTreeSet<_>>().len() != permissions.len()
        || (kind == ExtKind::Theme && !permissions.is_empty())
    {
        return Err(fail(Why::Field, Field::Permissions));
    }
    Ok(Manifest {
        name,
        version: version.to_owned(),
        kind,
        permissions,
    })
}
fn color(v: &Value) -> Option<String> {
    static OK: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)^oklch\(\s*([0-9]*\.?[0-9]+)(%?)\s+([0-9]*\.?[0-9]+)\s+([0-9]*\.?[0-9]+)(?:deg)?\s*\)$").unwrap()
    });
    let s = v.as_str()?.trim();
    if s.strip_prefix('#')
        .is_some_and(|h| matches!(h.len(), 3 | 6) && h.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Some(s.to_ascii_lowercase());
    }
    let m = OK.captures(s)?;
    let l = m[1].parse::<f64>().ok()? / if &m[2] == "%" { 100. } else { 1. };
    let c = m[3].parse::<f64>().ok()?;
    let h = m[4].parse::<f64>().ok()?;
    (l <= 1. && c <= 0.5 && h <= 360.).then(|| format!("oklch({l} {c} {h})"))
}
pub(super) fn theme(bytes: &[u8], secrets: &Secrets) -> Result<String, Problem> {
    let invalid = fail(Why::Theme, Field::Theme);
    let m = text(bytes, Field::Theme)?
        .trim()
        .strip_prefix("aproxy-theme-v1:")
        .and_then(object)
        .ok_or(invalid)?;
    let mut out = Map::new();
    for k in ["name", "author"] {
        if m.contains_key(k) || k == "name" {
            let name = clean(m.get(k).and_then(Value::as_str).ok_or(invalid)?);
            if name.encode_utf16().count() > 40 || k == "name" && name.is_empty() {
                return Err(invalid);
            }
            if secrets.find(name.as_bytes()).is_some() {
                return Err(fail(Why::PrivateData, Field::Theme));
            }
            if !name.is_empty() {
                out.insert(k.into(), name.into());
            }
        }
    }
    for (k, allowed) in [
        ("contrast", &["standard", "more"][..]),
        ("font", &["rounded", "system", "mono"][..]),
    ] {
        if let Some(v) = m.get(k) {
            if !v.as_str().is_some_and(|s| allowed.contains(&s)) {
                return Err(invalid);
            }
            out.insert(k.into(), v.clone());
        }
    }
    for (k, min, max, scale) in [
        ("radius", 0., 1.5, 100.),
        ("motion", 0., 1.5, 100.),
        ("titlebarMin", 36., 64., 1.),
    ] {
        if let Some(v) = m.get(k) {
            let n = v
                .as_f64()
                .filter(|n| (min..=max).contains(n))
                .ok_or(invalid)?;
            out.insert(k.into(), ((n * scale).round() / scale).into());
        }
    }
    for variant in ["light", "dark"] {
        if let Some(v) = m.get(variant) {
            let c = v.as_object().ok_or(invalid)?;
            let mut seeds = Map::new();
            for k in ["primary", "text", "surface", "ok", "retry", "error", "warn"] {
                if let Some(v) = c.get(k) {
                    seeds.insert(k.into(), color(v).ok_or(invalid)?.into());
                }
            }
            out.insert(variant.into(), seeds.into());
        }
    }
    let result = format!("aproxy-theme-v1:{}", Value::Object(out));
    if result.len() > 16384 || secrets.find(result.as_bytes()).is_some() {
        return Err(fail(Why::PrivateData, Field::Theme));
    }
    Ok(result)
}
