# 2026-09-11 渠道 P0 与可信发布落地（npm/crates.io/binstall）

## 终态

- tag 推送 → test 门槛 → 11 变体构建 → GitHub Release → npm 10 包 + crates.io，全自动（release.yml publish job）。
- npm：@meowo/aproxy 主包（JS 转发器，bin/aproxy.js）+ 9 平台子包（os/cpu/libc 装配，esbuild/rollup 同款）。OIDC 发布，provenance SLSA v1 已实证（dist-attestations）。
- crates.io：aproxy。trusted publishing config #19617（MoYeRanqianzhi/aProxy + release.yml + environment release）。CARGO_REGISTRY_TOKEN secret 保留作 workflow 兜底（`outputs.token || secrets.CARGO_REGISTRY_TOKEN`）。
- 用户 npm org 实名是 **meowo**（不是 GitHub 用户名派生的 moyeranqianzhi）。

## 关键事实（再犯即查）

1. **npm GAT 的 IP 允许列表**：granular token 建 IP allowlist 后，GitHub Actions runner IP 必被拒（PUT 伪装 404）。本地同 token 却能发——CI 发布失败先查这个。npm 渠道在 CI 唯一稳态 = trusted publishing（OIDC），token 路径不可靠。NPM_TOKEN secret 已删。
2. **npm trusted publisher 绑定强制人类 2FA**：`npm trust` CLI 与网页都要求账号级 2FA 参与且 GAT 不可绕——npm 有意设计（防 token 泄露自我授权），AI 不可代办。
3. **crates.io 绑定有官方 API 可代办**：`POST /api/v1/trusted_publishing/github_configs`，体 `{"github_config":{"crate":...,"repository_owner":...,"repository_name":...,"workflow_filename":...,"environment":...}}`（字段是 `crate` 不是 `krate`，serde rename）；token 需 TrustedPublishing endpoint scope；`GET ...?crate=aproxy` 可复核。
4. **npm 11 prerelease 强制 `--tag latest`**：不显式给 tag 直接拒发。0.1.x 全程 alpha，latest=最新 alpha 语义正确；0.2.0 起 stable 自动接管 latest。
5. **cargo include 白名单**：`README.md` 不带前导 `/` 会按 gitignore 语义匹配任意层级（npm/ 子目录 README 混进过 crate 包）。全部 `/xxx` 锚定。
6. **发布资产命名命中 binstall 内置默认**：`{ name }-{ target }{ binary-ext }` + 裸二进制（pkg-fmt bin）零配置。别画蛇添足加 metadata。
7. **npm 新平台首发顺序**：先发布包 → 才能在网页绑 TP → 才能走 OIDC。种子发布（一次手动）→ 绑定 → 自动化，是这个顺序的通用模式。

## 种子发布模式（未来新渠道复用）

本地 token 首发（build-and-publish.sh --release <tag> / cargo publish）→ 网页/API 绑 trusted publisher → 删 token → CI 纯 OIDC。npm 的 GAT 建议一开始就不要在 CI 用。
