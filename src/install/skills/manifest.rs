//! 发布包与本地副本共用一份文件清单。先验证整个命名空间，再允许其中的键参与路径拼接。

use serde::{Deserialize, Deserializer, Serialize, de};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, io,
};

/// 文档数量上限，限制目录创建次数和清单索引的内存。
pub const MAX_FILES: usize = 1024;
/// 单篇最多 1 MiB，读取验证后的正文可以一次完整交付。
pub const MAX_FILE: u64 = 1 << 20;
/// 全包最多展开 16 MiB，不能靠堆叠许多小文档耗尽磁盘。
pub const MAX_TOTAL: u64 = 16 << 20;
/// 清单本身也来自外部；解析 JSON 前先限制字节数。
pub const MAX_MANIFEST: u64 = 256 << 10;
/// 安装的提交标记，包内与本地都在根目录下。
pub(super) const MARKER: &str = "manifest.json";

/// 一个白名单文件，长度和摘要都针对原始字节而非解码后的字符串。
#[derive(Debug, Deserialize, Serialize)]
pub struct FileRecord {
    /// 解压后的精确字节数。
    pub size: u64,
    /// 原始字节的 SHA-256，小写十六进制。
    pub sha256: String,
}

/// 一次完整安装的文件集合。未知属性可以扩展，已知属性不允许缺失或重复。
#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    /// 只解释已知的清单格式。
    pub schema: u32,
    /// 文档所描述的二进制版本，包含预发布后缀。
    pub version: String,
    /// 文件键按字节排序，列举结果不依赖平台或 zip 内的顺序。
    #[serde(deserialize_with = "unique_files")]
    pub files: BTreeMap<String, FileRecord>,
}

/// 默认的 map 反序列化会静默覆盖同名键；在 JSON 解析时拦截才能发现它。
fn unique_files<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, FileRecord>, D::Error> {
    /// 逐项构建索引，不按输入自报的条目数预分配。
    struct Files;
    impl<'de> de::Visitor<'de> for Files {
        type Value = BTreeMap<String, FileRecord>;
        /// 描述期望的 JSON 类型，供解析器产生错误。
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("文件索引")
        }
        /// 数量超限或覆盖旧键都不是完整的白名单。
        fn visit_map<A: de::MapAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
            let mut files = BTreeMap::new();
            while let Some((k, v)) = a.next_entry()? {
                if files.len() == MAX_FILES || files.insert(k, v).is_some() {
                    return Err(de::Error::custom("文件索引无效"));
                }
            }
            Ok(files)
        }
    }
    d.deserialize_map(Files)
}

/// 统一两平台的路径语义：既不是 Windows 设备名，也不能被 Win32 截短。
fn valid_segment(s: &str) -> bool {
    let stem = s.split('.').next().unwrap().to_ascii_uppercase();
    !s.is_empty()
        && !s.ends_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

impl Manifest {
    /// 不传播 serde 的具体值：损坏的本地文件可能含有不应出现在错误输出中的文本。
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        let invalid = || io::Error::other("skill 清单无效");
        if bytes.len() as u64 > MAX_MANIFEST {
            return Err(invalid());
        }
        let m: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        if m.schema != 1 || semver::Version::parse(&m.version).is_err() || m.files.is_empty() {
            return Err(invalid());
        }
        let mut total = 0;
        // 同名目录可共用，不同拼写的目录或目录与普通文件不得映射到同一路径。
        let mut paths = BTreeMap::new();
        for (key, record) in &m.files {
            let (skill, _) = key.split_once('/').ok_or_else(invalid)?;
            if key.len() > 1024
                || key.split('/').count() > 32
                || reserved(skill)
                || !key.split('/').all(valid_segment)
                || record.size > MAX_FILE
                || record.sha256.len() != 64
                || !record
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
            {
                return Err(invalid());
            }
            total += record.size;
            if total > MAX_TOTAL {
                return Err(invalid());
            }
            for (i, _) in key
                .match_indices('/')
                .chain(std::iter::once((key.len(), "")))
            {
                let prefix = &key[..i];
                if let Some(old) = paths.insert(prefix.to_ascii_lowercase(), prefix)
                    && (old != prefix || m.files.contains_key(prefix))
                {
                    return Err(invalid());
                }
            }
        }
        Ok(m)
    }
    /// 目录所有权只从验证过的键导出，清单之外的用户目录不作清理。
    pub(super) fn names(&self) -> BTreeSet<&str> {
        self.files
            .keys()
            .map(|k| k.split_once('/').unwrap().0)
            .collect()
    }
}

/// 内部目录绝不能作为 skill 名，否则替换时可能移动正在持有的锁或工作区。
pub(super) fn reserved(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(lower.as_str(), MARKER | ".lock" | ".dl" | ".staging")
        || lower.starts_with(".staging-")
}

/// 与发布清单相同的小写十六进制表示；下载、读取都比较原始字节的摘要。
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
