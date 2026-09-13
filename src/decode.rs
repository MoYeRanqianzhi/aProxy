//! 响应体解码：把「按 `content-encoding` 压缩的上游响应体」解出一份副本，
//! **仅供检查与日志预览使用**——转发给客户端的字节始终是上游原样。
//!
//! 为什么需要它：agent 客户端普遍发送 `accept-encoding: gzip, deflate, br, zstd`，
//! 而本代理刻意不让 reqwest 自动解压（上游字节必须原样转发，才能兑现
//! 「status/头/体字节保真」这一既有契约，见 `proxy.rs` 模块头）。但**检查**
//! 是跑在同一份原始字节上的，于是压缩响应会让三项能力同时静默失效：
//!
//! - `retry::is_error_body` 对压缩体做 JSON 解析必然失败 → 「HTTP 200 携带错误
//!   JSON」不再触发重试
//! - `retry::is_stream_error_body` 的 SSE 行扫描对压缩体全失效 → 流尾 error
//!   事件检测不到
//! - `proxy::preview_body` 只看到高熵数据 → 只能打 hex 摘要，排障时读不出内容。
//!   而 **brotli 没有 magic number**（gzip 的 `1f 8b`、zstd 的 `28 b5 2f fd`
//!   都能一眼认出），连「这是压缩体」都难判断
//!
//! 2026-09-14 实测暴露的正是最后一例：`opencode.ai` 把 404 页以 brotli 返回，
//! 日志里只剩一串 hex（解出来是 Cloudflare 的 HTML 404 页），用户完全无法定位。
//!
//! 所以这里解出一份**副本**喂给检查路径，转发路径丝毫不受影响。
//!
//! 能力与边界：
//! - 支持 `gzip` / `x-gzip` / `deflate`（zlib 与裸 deflate 两种变体都试）/
//!   `br` / `zstd`；多层编码（如 `gzip, br`，按规范后列者后应用）按逆序逐层解
//! - 截断的压缩流取「已解出的前缀」——磁盘模式只喂 1 KiB 头部快照，全量解码
//!   既不可能也无必要（预览用前缀足够）
//! - 解码输出超 [`MAX_DECODED`] 视为失控，返回 `None`（防解压炸弹）
//! - 返回 `None` 的语义一律是「请用原始字节」，调用方无需区分失败原因
//!
//! 已知边界：**磁盘模式（响应 > 1 MiB）的重试判定不做解码**——增量扫描器
//! （`proxy::StreamErrorScanner`）吃的是原始字节流，为它改造成流式解码的收益与
//! 风险不成比例（> 1 MiB 的压缩错误体现实中不存在）。该路径只解头部快照用于
//! 预览，判定仍以状态码为准。

use std::io::Read;

/// 检查用解码的输出上限：超出即按「不可解码」处理（调用方回退原始字节）。
/// 真实错误体解压后都是 KB 级；上限只为把恶意/失控上游的 CPU 与内存开销钉死
/// 在常数——压缩炸弹可以用几百字节换出几十 GB。
const MAX_DECODED: usize = 8 * 1024 * 1024;

/// brotli 解码器的内部窗口缓冲（`Decompressor::new` 的参数，与文件 IO 块同量级）
const BROTLI_BUF: usize = 4096;

/// 按 `content-encoding` 解出一份响应体副本（仅供检查/预览）。
///
/// 返回 `None` = 调用方应使用原始字节，三种情形：无需解码（无该头或
/// `identity`）、编码不支持/解码无产出（含截断到无法解出任何内容）、输出超上限。
pub fn for_inspection(content_encoding: Option<&str>, body: &[u8]) -> Option<Vec<u8>> {
    let encoding = content_encoding?.trim();
    if encoding.is_empty() || encoding.eq_ignore_ascii_case("identity") {
        return None;
    }
    // 多层编码按逆序解：`gzip, br` 表示先 gzip 后 br，故先解 br 再解 gzip。
    // 首层直接在原始字节上解——单层是绝对常态，这样能省掉一次整体拷贝
    //（压缩体可达 1 MiB，且首轮判定对**每个**响应都要跑一遍）。
    let mut layers = encoding
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .rev();
    let mut data = decode_one(layers.next()?, body)?;
    for layer in layers {
        data = decode_one(layer, &data)?;
    }
    Some(data)
}

/// 解一层编码。空层（`gzip,` 这类尾随逗号产生的空串）与未知编码一律 `None`。
fn decode_one(encoding: &str, body: &[u8]) -> Option<Vec<u8>> {
    if encoding.is_empty() {
        return None;
    }
    let lower = encoding.to_ascii_lowercase();
    match lower.as_str() {
        "gzip" | "x-gzip" => decode_reader(flate2::read::GzDecoder::new(body)),
        // `deflate` 的规范含义是 zlib 包装，但历史上大量实现直接发裸 deflate。
        // 先按 zlib 试（其前两字节有 CMF/FLG 校验，裸流会在头校验处立即失败且
        // 无任何产出），无产出再按裸流试——顺序不能反，否则 zlib 流的头会被
        // 当成 deflate 数据解出垃圾。
        "deflate" => decode_reader(flate2::read::ZlibDecoder::new(body))
            .or_else(|| decode_reader(flate2::read::DeflateDecoder::new(body))),
        "br" => decode_reader(brotli::Decompressor::new(body, BROTLI_BUF)),
        // zstd 帧头解析失败（非 zstd 数据）即无解码器可用
        "zstd" => decode_reader(ruzstd::decoding::StreamingDecoder::new(body).ok()?),
        _ => None,
    }
}

/// 读完解码器的全部输出；截断流（磁盘模式的头部快照、上游中途断开）取已解出的
/// 前缀。无任何产出即视为解码失败——调用方回退原始字节。
fn decode_reader<R: Read>(reader: R) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    // take(MAX+1)：多读 1 字节才能区分「恰好等于上限」与「已超限」
    let mut limited = reader.take(MAX_DECODED as u64 + 1);
    match limited.read_to_end(&mut out) {
        // 正常读完；或截断流但已解出部分内容——后者是磁盘模式与上游半途断开的常态
        Ok(_) => {}
        Err(_) if !out.is_empty() => {}
        Err(_) => return None,
    }
    if out.is_empty() || out.len() > MAX_DECODED {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// 测试样本：同时含 ASCII、中文与换行，覆盖多字节 UTF-8 的边界
    const SAMPLE: &str = "错误响应预览：404 Not Found\n<html><body>Not Found</body></html>\n";

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    fn brotli(data: &[u8]) -> Vec<u8> {
        // into_inner 会以 BROTLI_OPERATION_FINISH 收尾（否则流不完整）
        let mut enc = brotli::CompressorWriter::new(Vec::new(), 4096, 5, 22);
        enc.write_all(data).unwrap();
        enc.into_inner()
    }

    fn zstd(data: &[u8]) -> Vec<u8> {
        ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
    }

    #[test]
    fn gzip_decodes_to_original() {
        let raw = gzip(SAMPLE.as_bytes());
        assert_ne!(raw, SAMPLE.as_bytes(), "样本应确实被压缩");
        let got = for_inspection(Some("gzip"), &raw).expect("gzip 应可解码");
        assert_eq!(String::from_utf8(got).unwrap(), SAMPLE);
    }

    #[test]
    fn encoding_name_is_case_insensitive_and_has_alias() {
        // HTTP 头值大小写不敏感；x-gzip 是 gzip 的历史别名
        let raw = gzip(SAMPLE.as_bytes());
        for name in ["GZIP", "Gzip", "x-gzip", "X-GZIP"] {
            let got = for_inspection(Some(name), &raw).unwrap_or_else(|| panic!("{name} 应可解码"));
            assert_eq!(String::from_utf8(got).unwrap(), SAMPLE);
        }
    }

    #[test]
    fn zlib_and_raw_deflate_both_decode() {
        // 规范说 deflate = zlib，但裸 deflate 实现广泛存在，两种都必须能解
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(SAMPLE.as_bytes()).unwrap();
        let zlib = z.finish().unwrap();

        let mut d = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        d.write_all(SAMPLE.as_bytes()).unwrap();
        let raw_deflate = d.finish().unwrap();

        assert_ne!(zlib, raw_deflate, "两种 deflate 变体的字节应不同");
        for (label, data) in [("zlib", &zlib), ("raw", &raw_deflate)] {
            let got = for_inspection(Some("deflate"), data)
                .unwrap_or_else(|| panic!("{label} deflate 应可解码"));
            assert_eq!(String::from_utf8(got).unwrap(), SAMPLE, "{label}");
        }
    }

    #[test]
    fn brotli_decodes_to_original() {
        // 回归锚点：brotli **没有 magic number**，正是 2026-09-14 事故里
        // 「日志只剩 hex、连是压缩体都判断不出」的原因
        let raw = brotli(SAMPLE.as_bytes());
        let got = for_inspection(Some("br"), &raw).expect("brotli 应可解码");
        assert_eq!(String::from_utf8(got).unwrap(), SAMPLE);
    }

    #[test]
    fn zstd_decodes_to_original() {
        let raw = zstd(SAMPLE.as_bytes());
        let got = for_inspection(Some("zstd"), &raw).expect("zstd 应可解码");
        assert_eq!(String::from_utf8(got).unwrap(), SAMPLE);
    }

    #[test]
    fn layered_encodings_decode_in_reverse_order() {
        // `gzip, br` 语义：先 gzip 后 br，故解码须先 br 再 gzip
        let raw = brotli(&gzip(SAMPLE.as_bytes()));
        let got = for_inspection(Some("gzip, br"), &raw).expect("多层编码应可解码");
        assert_eq!(String::from_utf8(got).unwrap(), SAMPLE);
    }

    #[test]
    fn absent_or_identity_encoding_needs_no_decode() {
        // None / identity / 空白：本就不需要解码，返回 None 让调用方用原始字节
        for enc in [
            None,
            Some(""),
            Some("  "),
            Some("identity"),
            Some("IDENTITY"),
        ] {
            assert!(
                for_inspection(enc, SAMPLE.as_bytes()).is_none(),
                "{enc:?} 不应触发解码"
            );
        }
    }

    #[test]
    fn unknown_encoding_returns_none() {
        for enc in ["compress", "exotic-future-codec", ""] {
            assert!(
                for_inspection(Some(enc), SAMPLE.as_bytes()).is_none(),
                "{enc} 是未知编码，应回退原始字节"
            );
        }
    }

    #[test]
    fn undecodable_body_returns_none() {
        // 明文 body 配上虚假的 content-encoding：解码无产出 → None（回退原始字节，
        // 不能把垃圾当解码结果返回）
        for enc in ["gzip", "br", "zstd", "deflate"] {
            assert!(
                for_inspection(Some(enc), SAMPLE.as_bytes()).is_none(),
                "{enc}: 非压缩数据不应被当作解码成功"
            );
        }
    }

    #[test]
    fn truncated_stream_yields_decoded_prefix() {
        // 磁盘模式只喂 1 KiB 头部快照，且上游可能半途断开——此时应取已解出的
        // 前缀（预览够用），而不是整体判为失败
        let long = "错误响应预览与排障：".repeat(400);
        let compressed = gzip(long.as_bytes());
        let truncated = &compressed[..compressed.len() * 3 / 5];

        let got = for_inspection(Some("gzip"), truncated).expect("截断流应解出前缀");
        assert!(!got.is_empty(), "前缀不应为空");
        assert!(got.len() < long.len(), "截断流不应解出完整内容");
        // 按**字节**比较而非字符串：截断点落在多字节 UTF-8 字符中间是常态，
        // 前缀本就不保证是合法 UTF-8（调用方 preview_body 走 from_utf8_lossy，
        // 判定路径的 serde_json 解析失败即视为非错误体，均不受影响）
        assert_eq!(
            got,
            long.as_bytes()[..got.len()],
            "解出的前缀必须是原文的字节前缀（否则是错位解码）"
        );
    }

    #[test]
    fn decompression_bomb_is_refused() {
        // 解压炸弹：几百 KB 输入解出 9 MiB（> MAX_DECODED）。必须拒绝而不是
        // 把内存吃满——返回 None 让调用方用原始字节（hex 摘要）
        let bomb = gzip(&vec![0u8; MAX_DECODED + 1024 * 1024]);
        assert!(
            bomb.len() < 1024 * 1024,
            "测试前提：炸弹输入应远小于其输出（实际 {} 字节）",
            bomb.len()
        );
        assert!(
            for_inspection(Some("gzip"), &bomb).is_none(),
            "超过输出上限应拒绝解码"
        );
    }

    #[test]
    fn exactly_at_limit_is_accepted() {
        // 边界：输出恰好等于上限时应接受（take(MAX+1) 的 +1 正是为此）
        let raw = gzip(&vec![0u8; MAX_DECODED]);
        let got = for_inspection(Some("gzip"), &raw).expect("恰好等于上限应接受");
        assert_eq!(got.len(), MAX_DECODED);
    }
}
