---
name: bounded-retry-paths
description: 受限重试路径 bounded_retry_paths——配置驱动的正则路径，命中者失败 3 次即透传；含首版内置 URL 被否决的纠正教训
metadata:
  type: project
---

# 受限重试路径（2026-09-20）

## 问题与根因

现象：Claude Code compact 总是出错；仅转发模式与直连都正常。

根因：compact 依赖 `POST /v1/messages/count_tokens`，部分镜像/聚合上游未实现
该端点，确定性 404（HTML/JSON 错误页）；「4xx/5xx 一律无限重试」让该调用永远
等不到终态，Claude Code 等到自己的超时后报错。12233 复现日志实证（2026-09-20
13:44 UTC：count_tokens 404 风暴 attempt 1→7+、退避封顶 320s）；2026-09-15 的
12345 日志有同一风暴（另一上游）。compact 的总结请求本身是成功的——害死
compact 的是这个辅助调用。教训：**「无限重试」对瞬时错误是保障，对确定性错误
是纯伤害**。

## 最终设计（用户定向后重写）

**`bounded_retry_paths` 是 config.toml / settings.json 配置项（正则数组，空 =
关闭），绝不内置任何 URL**——哪些上游端点确定性报错完全因上游而异，内置即
越权假设；用户也可能用其他 agent 软件，不能把 count_tokens 当成唯一场景。

- 匹配：模式对「`路径?查询串` **整体**」做正则匹配，编译时自动锚定
  `^(?:模式)$`——普通路径即精准匹配；**查询串不可省略**，`?` 是元字符须写
  `\?`（toml 建议单引号字符串）；通配用 `.*`。非法正则 validate() 拒绝启动。
- 编译入口唯一：`Config::compile_bounded_retry_pattern`，validate 与
  AppState::new 共用（校验通过 = 运行期编译必成功）；AppState 持预编译
  `Arc<Vec<Regex>>`，热路径只 is_match。
- 行为：命中请求总尝试第 1、2 次的有响应失败照常重试，第 3 次起有响应失败
  立即透传终止（`BOUNDED_RETRY_MAX_ATTEMPTS=3`，前两次零延迟）；网络错误
  不封顶；非命中请求零影响；保活通道以终态 SSE error 事件收场。
- 分层：toml 显式 > settings 全局默认（`bounded_retry_paths`）> 内置空；
  消费走 `bounded_retry_paths()` 访问器，不得 unwrap（doctor/find/测试不经
  settings 注入）。

## 首版被否决的教训（feedback，2026-09-20 用户明确纠正）

首版把 `/count_tokens` 内置成硬编码后缀列表 + 匹配时剥掉查询串——用户否决：
「你没有权利内置任何 url」「不能省略 ? 后面的内容，必须精准匹配 + 通配符
（正则）」「token 统计的 bug 只是我恰好遇到，不能假设只有这一种问题」。
**规则：行为差异类功能一律做成配置项；具体 URL/端点/软件特例绝不写进代码；
匹配语义与用户逐字对齐（含查询串）。** 修订版按此重写：配置驱动 + 整体正则。

诊断技巧：生产日志的 WARN 风暴是这类问题的第一现场；重试行没有请求 ID，
归因靠相邻 INFO 行（method/path）与错误预览内容交叉比对。

相关：[[forward-only]] [[compressed-body-inspection]] [[disconnect-billing-protection]]
