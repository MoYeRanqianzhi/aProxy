//! 清单和主题未经校验不能先收进普通 map：后一个同名键会盖掉前一个，安装页和执行者可能各信一个。
use super::{bad, field};
use crate::desktop::preferences::{language, package_id};
use aproxy_ipc::desktop::*;
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};
struct Bounded(u8, u8);
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
        if self.0 >= self.1 {
            return Err(de::Error::custom("depth"));
        }
        let mut m = Map::new();
        while let Some(k) = a.next_key::<String>()? {
            if m.contains_key(&k) {
                return Err(de::Error::custom("duplicate"));
            }
            m.insert(k, a.next_value_seed(Bounded(self.0 + 1, self.1))?);
        }
        Ok(m.into())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Value, A::Error> {
        if self.0 >= self.1 {
            return Err(de::Error::custom("depth"));
        }
        let mut v = Vec::new();
        while let Some(e) = a.next_element_seed(Bounded(self.0 + 1, self.1))? {
            v.push(e)
        }
        Ok(v.into())
    }
}
/// 逐层检查重复键与深度，最后才构建 JSON 对象。
pub(super) fn object(bytes: &[u8], depth: u8) -> Option<Map<String, Value>> {
    let mut d = serde_json::Deserializer::from_slice(
        bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes),
    );
    let Value::Object(m) = Bounded(0, depth).deserialize(&mut d).ok()? else {
        return None;
    };
    d.end().ok()?;
    Some(m)
}
/// 先清掉能改变显示方向或藏住字形的控制字符，再按 Unicode 标量计算显示长度。
pub(super) fn clean(s: &str) -> String {
    s.chars().filter(|c|!c.is_control() && !matches!(*c,'\u{061c}'|'\u{200b}'|'\u{200e}'|'\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2060}'|'\u{2066}'..='\u{2069}'|'\u{feff}')).collect::<String>().trim().to_owned()
}
fn display(v: &Value, max: usize) -> Option<String> {
    let s = clean(v.as_str()?);
    (1..=max).contains(&s.chars().count()).then_some(s)
}
fn text(v: &Value, max: usize) -> Option<Text> {
    if v.is_string() {
        return display(v, max).map(Text::Plain);
    }
    let m = v.as_object()?;
    if !(1..=16).contains(&m.len()) {
        return None;
    }
    m.iter()
        .map(|(k, v)| {
            language(k)
                .then_some(())
                .and_then(|()| Some((k.clone(), display(v, max)?)))
        })
        .collect::<Option<BTreeMap<_, _>>>()
        .map(Text::Localized)
}
fn repo(s: &str) -> Option<String> {
    // URL 解析器会消去 ..、反斜线和空 userinfo；不能用归一化后的路径给这些写法背书。
    let tail = s.strip_prefix("https://")?;
    let authority = tail.split('/').next()?;
    if authority.is_empty()
        || s.len() > 300
        || s.chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '\\' | '?' | '#'))
        || authority.contains('@')
    {
        return None;
    }
    let u = url::Url::parse(s).ok()?;
    let host = u.host_str()?;
    if host != "github.com" {
        return Some(s.into());
    }
    if !authority.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let path = tail.split_once('/')?.1;
    let (owner, r) = path.strip_suffix('/').unwrap_or(path).split_once('/')?;
    let r = r.strip_suffix(".git").unwrap_or(r);
    let n = owner.bytes().filter(|c| c.is_ascii_alphanumeric()).count();
    if !(1..=39).contains(&n)
        || owner.starts_with('-')
        || owner.ends_with('-')
        || owner.contains("--")
        || !owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || !(1..=100).contains(&r.len())
        || matches!(r, "." | "..")
        || !r
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return None;
    }
    Some(format!("https://github.com/{owner}/{r}"))
}
/// 未知键在安装目录的原文件里原样保留，传给页面的只含已核验的字段。
pub(super) fn parse(bytes: &[u8]) -> R<PackageManifest> {
    let m = object(bytes, 16).ok_or_else(|| bad("json", Some("manifest.json")))?;
    let val = |k: &str| m.get(k).unwrap_or(&Value::Null);
    let strval = |k: &str| val(k).as_str().ok_or_else(|| field(k));
    match val("schema") {
        Value::Number(n) if n.as_u64() == Some(1) => (),
        Value::Number(n) if n.is_i64() || n.is_u64() => {
            return Err(bad("unsupported-schema", Some("manifest.json")));
        }
        _ => return Err(field("schema")),
    }
    let kind = match strval("kind")? {
        "theme" => PackageKind::Theme,
        "extension" => PackageKind::Extension,
        _ => return Err(field("kind")),
    };
    let id = strval("id")?;
    if !package_id(id) {
        return Err(field("id"));
    }
    let version = strval("version")?;
    if version.len() > 64 || semver::Version::parse(version).is_err() {
        return Err(field("version"));
    }
    let repository = m
        .get("repository")
        .map(|v| v.as_str().and_then(repo).ok_or_else(|| field("repository")))
        .transpose()?;
    let license = m
        .get("license")
        .map(|v| {
            v.as_str()
                .filter(|s| {
                    (1..=100).contains(&s.len())
                        && s.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b".+-() ".contains(&b))
                })
                .map(str::to_owned)
                .ok_or_else(|| field("license"))
        })
        .transpose()?;
    let array = |k: &str, max: usize, valid: fn(&str) -> bool| -> R<Vec<String>> {
        let Some(v) = m.get(k) else { return Ok(vec![]) };
        let mut seen = BTreeSet::new();
        if kind != PackageKind::Extension {
            return Err(field(k));
        };
        v.as_array()
            .filter(|a| a.len() <= max)
            .and_then(|a| {
                a.iter()
                    .map(|v| {
                        v.as_str()
                            .filter(|s| valid(s) && seen.insert(*s))
                            .map(str::to_owned)
                    })
                    .collect()
            })
            .ok_or_else(|| field(k))
    };
    Ok(PackageManifest {
        schema: 1,
        kind,
        id: id.into(),
        name: text(val("name"), 64).ok_or_else(|| field("name"))?,
        version: version.into(),
        author: display(val("author"), 64).ok_or_else(|| field("author"))?,
        description: text(val("description"), 500).ok_or_else(|| field("description"))?,
        repository,
        license,
        formats: array("formats", 16, |s| {
            (1..=64).contains(&s.len())
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })?,
        network: array("network", 64, |s| {
            s.len() <= 253
                && s.strip_prefix("*.").unwrap_or(s).split('.').all(|l| {
                    !l.is_empty()
                        && l.len() <= 63
                        && !l.starts_with('-')
                        && !l.ends_with('-')
                        && l.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                })
        })?,
        writes: array("writes", 4, |s| {
            matches!(s, "config" | "instances" | "updates" | "formats")
        })?,
    })
}
