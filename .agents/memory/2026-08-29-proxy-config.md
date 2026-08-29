---
name: proxy-config
description: 代理配置功能已落地（config.toml + CLI），含 reqwest 代理机制关键事实与 7890 实测结论
metadata:
  type: project
---

2026-08-29 实现并验证「配置文件代理」功能（提交 `3a094e0`）。

功能形态：`~/.aproxy/config.toml` 的 `proxy`（完整代理 URL，可内嵌 `user:pass@`）+ 可选 `proxy_username`/`proxy_password`（优先于 URL 内嵌凭据）；CLI `aproxy config --proxy/--proxy-username/--proxy-password/--clear-proxy`，顶层 `--proxy` 仅本次运行生效不写配置。`--show` 中代理密码打码为 `***`。支持 http/https/socks4/socks5。未配置代理时沿用 reqwest 默认系统代理（读环境变量）。

reqwest 代理机制关键事实（源码核实，reqwest-0.12.28 / hyper-util-0.1.20）：
- `auto_sys_proxy` 默认 `true`，构建时 `proxies.push(ProxyMatcher::system())` → `from_env()`，**无条件读取 `ALL_PROXY`/`HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY` 环境变量**，与 `default-features=false` 无关。`system-proxy` feature（→hyper-util `client-proxy-system`）仅在环境变量未设置时补充 OS 级回退（Windows 注册表等）。
- `ClientBuilder::proxy(Proxy)` 会**自动把 `auto_sys_proxy` 置 false**（显式代理关闭环境代理），无需手动 `no_proxy()`。
- `Proxy::all` 对 URL 内嵌 `user:pass@` 自动用于代理鉴权（http 走 Proxy-Authorization，socks 走 raw_auth）；`basic_auth(u,p)` 可覆盖。
- 代理 URL 的 scheme 合法性解析不校验协议，但 socks 连接代码在 reqwest `socks` feature（空 feature，启用 hyper-util `SocksV4/V5`）后门编译；不加则 socks:// 会在连接期失败。

路径透传确认：`src/proxy.rs` `proxy_handler` 取 `uri.path_and_query()` 拼到 `upstream_base`，任意路径/查询串完整透传（含 `/health`）。

7890 实测：临时 12346 实例 `--proxy http://127.0.0.1:7890` 请求 `https://example.com` 首次 200，netstat 证实 `aproxy->127.0.0.1:7890 ESTABLISHED`；但后续与 `curl -x http://127.0.0.1:7890 https://example.com/` 直连均 HTTP 000 —— **7890(verge-mihomo) 对 example.com 的出站抖动是环境问题**，非代理功能缺陷。真实上游 `https://xn--wnup5g6so4wn.de5.net` 经 12345 实例工作正常。

**Why:** 早前不确定 reqwest 是否读环境变量代理（误以为需补 system-proxy feature）。实际 `from_env` 始终生效，只有 OS 级回退被 feature 门控。

**How to apply:** 配置代理走 `aproxy config --proxy`；改动代理后需重启实例生效（运行中实例启动时已固定 client）。排查代理相关故障时先 `netstat -ano | grep <aproxypid> | grep 7890` 确认是否真的经代理出站，再判定是代理问题还是上游问题。相关 [[502-via-7890]]。
