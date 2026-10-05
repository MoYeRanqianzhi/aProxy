# 计划：两个 skill 按 skill-creator 标准重写（2026-10-06 起）

## 为什么
用户 2026-10-06：skill 标准是全英文，且「不要直接翻译，直接翻译救不了原本就不符合标准的写法」。
原则已写进 AGENTS.md / CLAUDE.md 的 project_conventions。用户用 `/skill-creator` 下达本任务。

## 做法与现状
- 旧版快照：`.agents/skill-eval.local/skill-snapshot/{aproxy-cli,aproxy-format}`（本地，评测基线用）。
- 重写要求（给子代理的 brief）：`.agents/plan/skill-rewrite-brief.local.md`——面向操作 aProxy 的
  agent；说明原因而非堆「必须」；去掉维护者与版本史内容；每条事实对照源码，代码为准并上报差异；
  程序输出的中文原文保留并附英文释义；新布局：aproxy-cli 的 behaviors.md 拆出 clients.md 与
  troubleshooting.md，compatibility.md 只讲版本判定与 0.1.0 之后的差异（alpha 不再支持）。
- 已完成：两个 SKILL.md 由主代理重写（英文，quick_validate 通过）。
- 进行中：4 个 opus 子代理重写 references（behaviors/clients/troubleshooting、config-toml/settings-json、
  commands/compatibility、aproxy-format 的四个 reference + test_format.py）。
- 评测用例：`.agents/skill-evals/<skill>/evals.json`（每个 skill 3 个，含可区分优劣的陷阱：
  count_tokens 带 `?beta=true` 查询串、Claude Code 走流式而跨协议 SSE 不支持等）。

## 进度（2026-10-06）
- 重写已提交（0608f93）；子代理报出的代码问题已修（fc18505、4a6eeaa、be1101c），其余入 TODO。
- 第一轮评测（iteration-1，工作区 `.agents/skill-eval.local/`，sonnet 各跑一次）：新 97.7% / 旧 100%。
  6 个常见任务新旧都能做对，区分度低；唯一失分是新版回归——用例 1 里新版按 clients.md「一个实例
  一个上游」自行把 Codex 改接新开的 12346，违背用户指定的 12345。评审页面
  `.agents/skill-eval.local/review-iteration-1.html`，已请用户查看。

## 接下来
1. 读用户反馈（评审页面导出的 feedback.json）。
2. 修回归：用户指定的实例照用；上游协议可能不合时说明风险、把另开实例作为可选方案。
3. 第二轮评测补「旧文档写错」的区分性用例：重试退避（0/0/0/5 s）、Gemini 的 x-goog-api-key、
   看门狗恢复延迟、Windows 临时 APROXY_HOME 下 `stop <端口>` 会打到生产实例、format 回信不带 body
   会清空请求体。
4. 可选：description 触发优化（run_loop）。

## 并行中的其他事（见 TODO「下一版发布前」）
修复 4（b46a15f）、修复 5（3ce26e9）已提交。skill 文档需体现的新行为已转告子代理：转换器纳入
心跳节拍、提交后请求转换失败的 `proxy_transform_failed` 事件、override_headers 的 accept-encoding
启动 warn、断开提示（只收心跳 ≥60s）。自定义心跳 + format 扩展只有设计草稿，定稿前要给用户看。
