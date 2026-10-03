# 安全策略 / Security Policy

[中文](#中文) · [English](#english)

## 中文

### 受支持的版本

只有最新发布的 0.1.x 稳定版会收到安全修复。预发布版本（`0.1.0-alpha.*` 等）不再维护，请升级。

### 威胁模型

aProxy 是**本机单用户**的 API 代理：

- 它持有上游 API 密钥（`config.toml` 的 `api_key`、`override_headers` 等），并向每个转发请求注入。
- 代理端口**本身没有鉴权**：能连到该端口的进程，就能以你的密钥调用上游、花你的额度。信任边界就是「谁能连到这个端口」。
- 默认只监听回环地址 `127.0.0.1`，目的是把这个边界收窄到本机。

**已有的防护**

- 入站 **Host 校验**（防 DNS 重绑定）：监听回环地址时，只放行 `localhost` / `127.0.0.1` / `[::1]` 与监听地址自身的主机名，外加 `allowed_hosts` 追加的条目。
- 入站 **Origin 校验**（防网页借本机代理调用上游）：任何携带 `Origin` 头的请求默认拒绝，除非精确匹配 `allowed_origins`。CLI 类客户端不发 `Origin`，不受影响。
- 被这两项拒绝的请求在本地返回 403：不转发上游、不注入密钥、不进入重试。配置语法与例外见 README 的「入站来源校验」。
- 上游请求不跟随重定向（避免把密钥与请求体带到 3xx 指向的任意主机）。
- 日志与 `status` 对凭据打码：URL 的 userinfo、查询串的值、`api_key` 与头值；`config --show` 对转换器的密钥参数与 `extra` 打码。
- 控制通道（命名管道 / Unix socket）不占用代理端口，不经 HTTP 暴露；其访问控制取决于操作系统默认权限与 `~/.aproxy` 目录权限。

**不在防护范围内**

- **本机上的其他进程与其他用户**：回环端口对本机所有用户可连。多用户共享的机器上，同机其他用户可以使用你的代理。
- **把监听地址暴露到网络**：`listen_addr` 设为 `0.0.0.0` 或局域网地址后，同网段任何主机都能免鉴权使用你的密钥。启动、`startup.log` 与 `aproxy doctor` 会对此告警，但不会阻止——**请勿这样做**，除非网络完全可信。
- **磁盘上的明文密钥**：`api_key` 以明文保存在 `config.toml`。请收紧文件权限（Linux/macOS：`chmod 600`；Windows：仅当前用户可读写），并避免把它提交进版本库或放进云同步目录。`--api-key` 命令行参数可被本机其他进程枚举，长期使用请写进配置文件。
- **外部转换器是配置驱动的命令执行**：`request_transform` / `response_transform` 会以守护进程的权限启动 `command`（不经 shell，按 argv 执行），并把完整的请求/响应（头与 body，可能含凭据）交给它。只配置你信任的程序，也不要让他人能修改你的 `config.toml`。
- **安装脚本的校验**：一键安装脚本校验下载文件的 SHA256，该校验值与二进制出自同一个 GitHub release——能发现传输损坏，但不是独立的发布者签名验证。
- 对上游服务本身的信任：代理会原样转发上游返回的内容。

### 报告漏洞

请**不要**在公开 issue 里披露漏洞细节。请使用 GitHub 的私密漏洞报告：在仓库的 **Security** 页签选择 **Report a vulnerability**（即 [Security Advisories](https://github.com/MoYeRanqianzhi/aProxy/security/advisories/new)）。

请附上：受影响的版本（`aproxy --version`）、平台、复现步骤与你判断的影响范围。如果该页面暂时无法使用，可以开一个不含细节的公开 issue，请维护者提供私下沟通的方式。

## English

### Supported versions

Only the latest released 0.1.x stable version receives security fixes. Pre-releases (`0.1.0-alpha.*` and similar) are unmaintained; please upgrade.

### Threat model

aProxy is a **local, single-user** API proxy:

- It holds your upstream API key (`api_key` in `config.toml`, `override_headers`, ...) and injects it into every forwarded request.
- The proxy port has **no authentication of its own**: any process that can reach it can call the upstream with your key and spend your quota. The trust boundary is "who can connect to this port".
- It listens on the loopback address `127.0.0.1` by default so that the boundary is the local machine.

**Defenses in place**

- Inbound **Host check** (DNS-rebinding guard): when listening on loopback, only `localhost` / `127.0.0.1` / `[::1]` and the listen address's own host name are accepted, plus anything added via `allowed_hosts`.
- Inbound **Origin check** (stops web pages from using the proxy): any request carrying an `Origin` header is rejected unless it exactly matches `allowed_origins`. CLI-style clients send no `Origin` and are unaffected.
- A request rejected by either check gets a local 403: it is not forwarded, no key is injected, no retry happens. See "Inbound origin checks" in the README for syntax and exceptions.
- Upstream requests do not follow redirects (so the key and body are never sent to an arbitrary host named by a 3xx).
- Logs and `status` mask credentials: URL userinfo, query-string values, `api_key` and header values; `config --show` masks transformer secret arguments and `extra`.
- The control channel (named pipe / Unix socket) never uses the proxy port and is not exposed over HTTP; its access control depends on OS defaults and the permissions of `~/.aproxy`.

**Out of scope**

- **Other processes and other users on the same machine**: a loopback port is reachable by every local user. On a shared machine, other users can use your proxy.
- **Exposing the listen address to a network**: with `listen_addr` set to `0.0.0.0` or a LAN address, any host on that network can use your key without authentication. Start-up, `startup.log` and `aproxy doctor` warn about this but do not block it — **do not do it** unless the network is fully trusted.
- **Plain-text key on disk**: `api_key` lives in plain text in `config.toml`. Restrict the file's permissions (Linux/macOS: `chmod 600`; Windows: current user only) and keep it out of version control and cloud-synced folders. The `--api-key` command-line flag can be enumerated by other local processes; use the config file for anything long-lived.
- **External transformers are configuration-driven command execution**: `request_transform` / `response_transform` launch `command` with the daemon's privileges (no shell, argv execution) and hand it the full request/response (headers and body, possibly with credentials). Only configure programs you trust, and do not let others modify your `config.toml`.
- **Installer verification**: the one-line install scripts check the SHA256 of what they download; that checksum comes from the same GitHub release as the binary, so it catches corruption in transit but is not an independent publisher-signature check.
- Trust in the upstream service itself: the proxy relays whatever the upstream returns.

### Reporting a vulnerability

Please do **not** disclose details in a public issue. Use GitHub's private vulnerability reporting: on the repository's **Security** tab choose **Report a vulnerability** (the [Security Advisories](https://github.com/MoYeRanqianzhi/aProxy/security/advisories/new) form).

Include the affected version (`aproxy --version`), platform, reproduction steps and what you believe the impact is. If that page is unavailable, open a public issue without details and ask the maintainers for a private channel.
