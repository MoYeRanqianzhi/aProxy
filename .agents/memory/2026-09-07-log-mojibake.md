---
name: log-mojibake
description: 日志乱码两案并结：控制台 GBK→SetConsoleOutputCP(65001)（2b88808）；错误预览乱码实为上游 zstd 压缩体被 lossy 渲染→hex 摘要（1d5c75f）+ 日志文件 UTF-8 BOM
metadata:
  type: project
---

2026-09-07 用户二次报告「日志中文乱码」，取证后确认是**两个独立问题**，第一次修复只覆盖了其一：

**案一（2b88808 已修）**：控制台输出乱码——Windows 控制台默认代码页（中文系统 936/GBK）把 Rust 的 UTF-8 输出显示为乱码。修复：main 启动即 `SetConsoleOutputCP(65001)`。

**案二（1d5c75f 修复，用户指的「log 中乱码」）**：字节级扫描生产日志（12345/12349）确认——**全部行均为合法 UTF-8，文件编码无问题**；乱码集中在「错误响应预览」行：上游（Cloudflare 类）对错误页返回 **zstd 压缩体**（magic `28 b5 2f fd`），reqwest 未开自动解压（保真透传语义不可开），旧代码 `String::from_utf8_lossy(&body[..500])` 把控制字节渲染成整片替换符+控制字符墙（550/1611 行受污染）。

**修复语义**：`preview_body(body, limit)`——lossy 渲染后 U+FFFD/C0 控制字符占比 ≥10% 判为二进制，输出「二进制/压缩内容，共 N 字节，hex 前 48」摘要（hex 可直接识别 zstd/gzip magic）；文本体正常预览。另加防御：日志文件新建/截断（启动 2MiB 检查、运行期 `settings.log_rotate_mb` 轮转）时写 UTF-8 BOM（`daemon::write_utf8_bom_if_empty`），让按 ANSI 探测的查看器（记事本旧版/Get-Content）正确识别；`aproxy logs` 跟随器 0 偏移读取时剥 BOM。

**Why:** 「乱码」报告必须字节级取证定位层（文件编码？行内容？渲染层？）——本案文件层完好、内容层混入二进制、查看层另有 ANSI 探测问题，三层叠加表现为同一症状。

**How to apply:** 凡 `tracing` 输出外部字节（body/错误串），必须过 `preview_body` 或等价的可打印性判定；日志文件写入点（新建/截断/轮转）保持 BOM；未来若开 reqwest gzip 解压须重估「字节保真回放」语义（解压会破坏透传）。相关 [[daemon-model]] [[warning-zero-policy]]。
