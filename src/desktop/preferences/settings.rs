//! 设置是用户文档的一部分。JSON 没有日期，TOML 没有 null；只接受能无损读回的写入。
use super::*;
use serde_json::{Map, Value as Json};
use toml_edit::{Array, InlineTable, Table};
const TABLE_BYTES: usize = 64 << 10;
fn bad_table(key: &str) -> Fault {
    Fault::DesktopToml(TomlIssue {
        key: Some(key.into()),
        ..issue(TomlIssueKind::Value)
    })
}
fn from_toml(v: Value) -> Json {
    match v {
        Value::String(v) => v.into_value().into(),
        Value::Integer(v) => {
            let n = v.into_value();
            if n.unsigned_abs() > (1 << 53) {
                n.to_string().into()
            } else {
                n.into()
            }
        }
        Value::Float(v) => {
            let n = v.into_value();
            serde_json::Number::from_f64(n)
                .map(Json::Number)
                .unwrap_or_else(|| n.to_string().to_ascii_lowercase().into())
        }
        Value::Boolean(v) => v.into_value().into(),
        Value::Datetime(v) => v.value().to_string().into(),
        Value::Array(a) => Json::Array(a.into_iter().map(from_toml).collect()),
        Value::InlineTable(t) => Json::Object(
            t.into_iter()
                .map(|(k, v)| (k.to_string(), from_toml(v)))
                .collect(),
        ),
    }
}
fn to_toml(v: Json) -> R<Value> {
    Ok(match v {
        Json::Null => return Err(Fault::DesktopBadInput {}),
        Json::Bool(b) => b.into(),
        Json::String(s) => s.into(),
        Json::Number(n) => {
            if n.is_i64() || n.is_u64() {
                n.as_i64()
                    .filter(|v| v.unsigned_abs() <= 1 << 53)
                    .ok_or(Fault::DesktopBadInput {})?
                    .into()
            } else {
                n.as_f64().ok_or(Fault::DesktopBadInput {})?.into()
            }
        }
        Json::Array(a) => {
            let mut arr = Array::new();
            for v in a {
                arr.push(to_toml(v)?)
            }
            arr.into()
        }
        Json::Object(o) => {
            let mut t = InlineTable::new();
            for (k, v) in o {
                t.insert(k, to_toml(v)?);
            }
            t.into()
        }
    })
}
impl DesktopToml {
    /// 只投影这个扩展的一张表，保留任意嵌套数据；不会把相邻扩展的设置交给调用者。
    pub(in crate::desktop) fn settings(&self, id: &str) -> R<Map<String, Json>> {
        let mut item = self.doc.as_item();
        for key in ["extensions", "settings", id] {
            let t = item
                .as_table_like()
                .ok_or_else(|| bad_table("extensions.settings"))?;
            let Some(child) = t.get(key) else {
                return Ok(Map::new());
            };
            item = child;
        }
        if item.as_table_like().is_none() {
            return Err(bad_table("extensions.settings"));
        }
        let mut standalone = DocumentMut::new();
        standalone[id] = item.clone();
        if standalone.to_string().len() > TABLE_BYTES {
            return Err(crate::desktop::packages::limit(
                "settings-bytes",
                TABLE_BYTES as u64,
                None,
            ));
        }
        let Json::Object(o) = from_toml(item.clone().into_value().unwrap()) else {
            unreachable!()
        };
        Ok(o)
    }
    /// null 只在最外层表示删除。修改前后的注释与其他键交给同一棵 toml_edit 语法树保存。
    pub(in crate::desktop) fn change_settings(
        &mut self,
        id: &str,
        changes: Map<String, Json>,
    ) -> R<Map<String, Json>> {
        let mut item = self.doc.as_item_mut();
        for (depth, k) in ["extensions", "settings", id].into_iter().enumerate() {
            let inline = item.is_inline_table();
            let t = item
                .as_table_like_mut()
                .ok_or_else(|| bad_table("extensions.settings"))?;
            item = t.entry(k).or_insert_with(|| {
                if inline {
                    Item::Value(InlineTable::new().into())
                } else {
                    let mut t = Table::new();
                    t.set_implicit(depth < 2);
                    Item::Table(t)
                }
            });
        }
        let t = item
            .as_table_like_mut()
            .ok_or_else(|| bad_table("extensions.settings"))?;
        for (k, v) in changes {
            if !(1..=64).contains(&k.len())
                || !k
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            {
                return Err(Fault::DesktopBadInput {});
            }
            if v.is_null() {
                t.remove(&k);
            } else {
                let mut value = to_toml(v)?;
                if let Some(old) = t.get(&k).and_then(Item::as_value) {
                    *value.decor_mut() = old.decor().clone();
                }
                t.insert(&k, Item::Value(value));
            }
        }
        self.settings(id)
    }
}
