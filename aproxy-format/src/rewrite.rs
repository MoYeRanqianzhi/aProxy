//! 用显式字段与 JSON Pointer 改写请求；报告只记操作和位置，不泄露值。
use crate::config::{self, Fault};
use aproxy_envelope as w;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// 头部规则可以只接收元数据；模型或指针规则才协商完整请求体。
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Rewrite {
    /// 按名字删除头，匹配前统一为小写。
    remove_headers: Vec<String>,
    /// 经过 HTTP 值校验的整项赋值，不把值写入 report。
    set_headers: BTreeMap<String, String>,
    /// 按列出的次序删除，允许同一规则集用于有或没有该可选字段的请求。
    remove: Vec<String>,
    /// 按指针赋 JSON 值，对象末级可新建，数组下标不能产生空洞。
    set: BTreeMap<String, Value>,
    /// 正文编辑完成后按当前模型名查精确映射。
    models: BTreeMap<String, String>,
}
impl Rewrite {
    /// 在 hello 之前拒绝无效的指针转义和不能由代理送出的 HTTP 值。
    pub fn load(path: Option<&str>) -> Result<Self, &'static str> {
        let mut cfg: Self = match path {
            Some(p) => config::load(p)?,
            None => Self::default(),
        };
        let name = |s: &str| {
            http::HeaderName::from_bytes(s.as_bytes())
                .map(|h| h.to_string())
                .map_err(|_| "改写头名称无效")
        };
        for h in &mut cfg.remove_headers {
            *h = name(h)?;
        }
        for (key, value) in std::mem::take(&mut cfg.set_headers) {
            let key = name(&key)?;
            // 此角色不拥有目标 origin；认证值必须由选定渠道或密钥池写入，重试时才不会串用。
            if aproxy_ipc::redact::is_secret_header(&key) {
                return Err("set_headers 不接受凭据头，请使用 keys 或 route");
            }
            if !http::HeaderValue::from_str(&value).is_ok_and(|v| v.to_str().is_ok()) {
                return Err("改写头的值无效");
            }
            if cfg.set_headers.insert(key, value).is_some() {
                return Err("改写头名称重复");
            }
        }
        for p in cfg.remove.iter().chain(cfg.set.keys()) {
            let mut chars = p.chars();
            if !p.is_empty() && chars.next() != Some('/') {
                return Err("JSON Pointer 无效");
            }
            while let Some(c) = chars.next() {
                if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
                    return Err("JSON Pointer 无效");
                }
            }
        }
        if cfg.remove.iter().any(String::is_empty) {
            return Err("不能删除 JSON 根值");
        }
        Ok(cfg)
    }
    /// 只有需要访问 JSON 字段的配置才会协商 full。
    pub fn needs_body(&self) -> bool {
        !self.models.is_empty() || !self.set.is_empty() || !self.remove.is_empty()
    }
    /// 成功处理整轮后一次性给补丁；拒绝一处坏指针时不留下半份改写。
    pub fn request(&self, m: &w::RequestMessage, r: &mut w::Reply) -> Result<(), Fault> {
        let mut report = w::Report::new("Rewrote request fields".into());
        let mut patch = w::RequestPatch::new();
        if !self.remove_headers.is_empty() || !self.set_headers.is_empty() {
            let mut headers = m.request.headers.clone();
            for (name, replacement) in self
                .remove_headers
                .iter()
                .map(|k| (k, None))
                .chain(self.set_headers.iter().map(|(k, v)| (k, Some(v))))
            {
                let op = if let Some(v) = replacement {
                    if headers.get(name) == Some(v) {
                        continue;
                    }
                    headers.insert(name.clone(), v.clone());
                    w::EditOp::Set
                } else {
                    if headers.remove(name).is_none() {
                        continue;
                    }
                    w::EditOp::Remove
                };
                report.actions.push(w::Action::new(
                    w::StandardAction::HeaderEdit {
                        name: name.clone(),
                        op,
                    },
                    None,
                ));
            }
            if !report.actions.is_empty() {
                patch.headers = Some(headers);
            }
        }
        if self.needs_body() {
            let mut body: Value =
                serde_json::from_slice(m.request.body.as_bytes().unwrap_or_default())
                    .map_err(|_| Fault::new("请求体不是 JSON", "request body is not valid JSON"))?;
            let before = report.actions.len();
            for (path, value) in self
                .remove
                .iter()
                .map(|p| (p, None))
                .chain(self.set.iter().map(|(p, v)| (p, Some(v))))
            {
                if edit(&mut body, path, value)? {
                    report.actions.push(w::Action::new(
                        w::StandardAction::BodyEdit {
                            path: path.clone(),
                            op: if value.is_some() {
                                w::EditOp::Set
                            } else {
                                w::EditOp::Remove
                            },
                        },
                        None,
                    ));
                }
            }
            if let Some(to) = body["model"].as_str().and_then(|s| self.models.get(s))
                && body["model"].as_str() != Some(to)
            {
                body["model"] = to.as_str().into();
                report.actions.push(w::Action::new(
                    w::StandardAction::BodyEdit {
                        path: "/model".into(),
                        op: w::EditOp::Set,
                    },
                    None,
                ));
            }
            if before != report.actions.len() {
                patch.body = w::Body::Text(body.to_string());
            }
        }
        if !report.actions.is_empty() {
            r.report = w::ReportField::Present(report);
            r.request = Some(patch);
        }
        Ok(())
    }
}
/// 在已存在的父容器内操作。移除不存在的目标是幂等的；设置不存在的父对象不猜测容器类型。
fn edit(body: &mut Value, path: &str, value: Option<&Value>) -> Result<bool, Fault> {
    let invalid = || Fault::new("JSON Pointer 的目标无效", "invalid JSON Pointer target");
    let Some((parent, last)) = path.rsplit_once('/') else {
        let v = value.ok_or_else(invalid)?;
        if body == v {
            return Ok(false);
        }
        *body = v.clone();
        return Ok(true);
    };
    let key = last.replace("~1", "/").replace("~0", "~");
    match body.pointer_mut(parent) {
        Some(Value::Object(map)) => {
            if let Some(v) = value {
                if map.get(&key) == Some(v) {
                    Ok(false)
                } else {
                    map.insert(key, v.clone());
                    Ok(true)
                }
            } else {
                Ok(map.remove(&key).is_some())
            }
        }
        Some(Value::Array(items)) => {
            if key == "-"
                && let Some(v) = value
            {
                items.push(v.clone());
                return Ok(true);
            }
            let i = key
                .parse::<usize>()
                .ok()
                .filter(|i| i.to_string() == key && *i < items.len());
            match (i, value) {
                (Some(i), Some(v)) => {
                    if items[i] == *v {
                        return Ok(false);
                    }
                    items[i] = v.clone();
                    Ok(true)
                }
                (Some(i), None) => {
                    items.remove(i);
                    Ok(true)
                }
                (None, None) => Ok(false),
                _ => Err(invalid()),
            }
        }
        _ if value.is_none() => Ok(false),
        _ => Err(invalid()),
    }
}

#[cfg(test)]
/// 检验指针的转义、根值与数组语义，特别区分 null 和不存在。
mod tests {
    use super::*;
    use serde_json::json;
    /// RFC 转义逐段解释，数组不能静默扩容或把非规范下标当整数。
    #[test]
    fn edits_object_keys_arrays_null_and_root() {
        let mut b = json!({"x/y":{"~1":[null,2]},"0":false});
        let p = "/x~1y/~01";
        assert!(edit(&mut b, &format!("{p}/0"), None).unwrap_or_else(|e| panic!("{}", e.wire)));
        assert!(!edit(&mut b, &format!("{p}/1"), None).unwrap_or_else(|e| panic!("{}", e.wire)));
        assert!(
            edit(&mut b, &format!("{p}/-"), Some(&json!(3)))
                .unwrap_or_else(|e| panic!("{}", e.wire))
        );
        assert!(
            edit(&mut b, &format!("{p}/1"), Some(&json!(null)))
                .unwrap_or_else(|e| panic!("{}", e.wire))
        );
        assert!(
            !edit(&mut b, &format!("{p}/1"), Some(&json!(null)))
                .unwrap_or_else(|e| panic!("{}", e.wire))
        );
        assert!(edit(&mut b, &format!("{p}/01"), Some(&json!(5))).is_err());
        assert!(edit(&mut b, "/missing/child", Some(&json!(false))).is_err());
        assert!(edit(&mut b, "/0", Some(&json!({"n":4}))).unwrap_or_else(|e| panic!("{}", e.wire)));
        assert_eq!(b, json!({"x/y":{"~1":[2,null]},"0":{"n":4}}));
        assert!(edit(&mut b, "", Some(&json!([]))).unwrap_or_else(|e| panic!("{}", e.wire)));
        assert_eq!(b, json!([]));
    }
    /// 非法配置在首次协商前失败，错误不引用原值。
    #[test]
    fn rejects_invalid_pointer_and_header_before_hello() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rewrite.toml");
        for text in [
            "remove=['']",
            "remove=['relative']",
            "remove=['/bad~2']",
            "remove=['/bad~']",
            "set_headers={ 'X A'='sensitive' }",
            "set_headers={ 'hello'='值' }",
            "set_headers={ 'X-Test'='a', 'x-test'='b' }",
        ] {
            std::fs::write(&file, text).unwrap();
            assert!(
                Rewrite::load(Some(file.to_str().unwrap())).is_err(),
                "{text}"
            );
        }
    }
}
