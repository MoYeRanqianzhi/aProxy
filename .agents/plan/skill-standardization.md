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

## 接下来
1. 子代理交付后逐份审阅（核对上报的「旧文档与代码不符」清单，必要时修代码或文档）。
2. 同步仓库内对 skill 的外部引用：README.md:156、README_EN.md:251（链接改指 clients.md），
   记忆 agent-client-timeouts-survey、aproxy-cli-skill-versioning 中的文件与章节名。
3. 按 skill-creator 跑评测：每个用例「新 skill / 旧快照」各一次（sonnet，并发 ≤4，禁止运行
   aproxy 与触碰 ~/.aproxy），写断言、评分、`eval-viewer/generate_review.py` 出报告给用户看。
4. 按用户反馈迭代；最后可选做 description 触发优化（run_loop）。
5. 提交；CHANGELOG [Unreleased] 记一笔「skill 改为英文并重写」。

## 并行中的其他事（见 TODO「下一版发布前」）
修复 4（b46a15f）、修复 5（3ce26e9）已提交。skill 文档需体现的新行为已转告子代理：转换器纳入
心跳节拍、提交后请求转换失败的 `proxy_transform_failed` 事件、override_headers 的 accept-encoding
启动 warn、断开提示（只收心跳 ≥60s）。自定义心跳 + format 扩展只有设计草稿，定稿前要给用户看。
