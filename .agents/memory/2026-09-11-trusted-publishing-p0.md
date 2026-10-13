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
4. **npm 11 对预发布版本必须显式给 `--tag`**：不给直接拒发。2026-10-04 起（WS-5a，`npm/build-and-publish.sh` 的 dist-tag 选择）预发布（版本含 -alpha/-beta/-rc）发 `next`、稳定版发 `latest`，与 GitHub 的 prerelease 标志、release.yml 的 make_latest 一一对应；此前「全程 alpha 都发 latest」的做法已废止（会让 `npm i @meowo/aproxy` 装到预发布）。
5. **cargo include 白名单**：`README.md` 不带前导 `/` 会按 gitignore 语义匹配任意层级（npm/ 子目录 README 混进过 crate 包）。全部 `/xxx` 锚定。
6. **发布资产命名命中 binstall 内置默认**：`{ name }-{ target }{ binary-ext }` + 裸二进制（pkg-fmt bin）零配置。别画蛇添足加 metadata。
7. **npm 新平台首发顺序**：先发布包 → 才能在网页绑 TP → 才能走 OIDC。种子发布（一次手动）→ 绑定 → 自动化，是这个顺序的通用模式。

## 种子发布模式（未来新渠道复用）

本地 token 首发（build-and-publish.sh --release <tag> / cargo publish）→ 网页/API 绑 trusted publisher → 删 token → CI 纯 OIDC。npm 的 GAT 建议一开始就不要在 CI 用。

## 2026-10-04：format 线的 Trusted Publishing 没配全（format-v0.1.1 实测）

- **crates.io**：可信发布配置按 workflow 文件名匹配。仓库只登记了 `release.yml`，所以
  `release-format.yml` 换 token 时报 400「does not match the workflow filename
  release-format.yml … Expected workflow filenames: release.yml」。认证步骤本身失败，
  `|| secrets.CARGO_REGISTRY_TOKEN` 兜底根本走不到。需要所有者给 `aproxy-format` 与
  `aproxy-envelope` 各加一条 workflow = `release-format.yml`、environment = `release`
  的配置（第 3 条的 API 或网页）。**2026-10-04 已补齐**（所有者提供临时 token，主代理经
  第 3 条 API 创建并回读）：aproxy-format ← release-format.yml（#23480）、aproxy-envelope ←
  release-format.yml（#23481）与 release.yml（#23482，主线在 envelope 版本变动时也会发布它）；
  aproxy 仍是 release.yml（#19617）。随后单独重跑 publish-format-crates，aproxy-format 0.1.1
  经 OIDC 发布成功（crates.io 的 trustpub_data 指向 run 37192303424）。
- **npm**：format 线 10 个包（`@meowo/aproxy-format` 主包 + 9 个平台包）原本没有绑定 Trusted Publisher，
  OIDC 发布返回 PUT 404。**2026-10-06 所有者在网页上补齐**，0.1.1 经 OIDC 发布成功。**教训**：npm 绑定按
  workflow 文件名匹配，format 线是 `release-format.yml`，不是主线的 `release.yml`；所有者第一次照着
  主线包填写，又 404 了一轮。请所有者做绑定时，要逐字段给出取值（Organization `MoYeRanqianzhi`、
  Repository `aProxy`、Workflow `release-format.yml`、Environment `release`），并点明 10 个包
  都要绑定（脚本先发平台包，只绑主包也会在第一个平台包失败）。
- **本机凭据**：`~/.cargo/credentials.toml` 里的 crates.io token 已失效（发布返回
  403 authentication failed），本机 npm 未登录（401）。发布只能走 CI 的 OIDC。
- 绑定补齐后用 `gh run rerun <run-id> --failed` 只重跑失败的发布 job 即可：两个发布 job 都按
  「版本已存在才跳过」幂等。format 0.1.1 的 npm 包就是这样在 run 37192303424 上补发的。
- npm 的 `+ @meowo/...@x.y.z` 日志出现后，registry 元数据要过几分钟才显示新版本与 dist-tag，
  马上查到旧值不代表发布失败（主线 0.1.0 与 format 0.1.1 都遇到过）。

## 2026-10-11：0.2.0 无 UI 版本的 npm 种子发布（npm 11.17 实测）

- 所有者给了一个 GAT（带「跳过 2FA」），主代理用它把 `@meowo/aproxy-headless` 与 9 个
  `@meowo/aproxy-headless-<平台>` 以 0.0.0 占位版本（只有 README 与 package.json，不含二进制）发布成功。
- **用同一个 GAT 做 `npm trust github` 返回 403，`npm trust list` 也 403**——与 npm 文档一致：信任命令要求
  账号级 2FA，不接受跳过 2FA 的 GAT，所以绑定与查看都只能由所有者在自己的终端或网页完成。文档要点：
  包必须已存在；必须显式给 `--allow-publish`（默认 false，不给就没有发布权限）；第一次 2FA 时网页上可勾选
  「5 分钟内跳过 2FA」，之后的 `npm trust` 批量执行不再要求验证。
- **本机陷阱**：在 `C:/Users/MoYeR` 之下、没有 package.json 的目录里运行 npm，会把用户目录下的 `.npmrc`（里面是
  失效的旧 token）当作 project 配置，覆盖 `--userconfig` 里的 token，表现为 401。在包目录里运行即可。
  `npm trust` 的参数要写成 `--userconfig=<路径>`（带等号），分开写会报「Unknown positional argument」。
- 所有者 2026-10-11 告知 10 个包的绑定已完成（release.yml / release）。主代理用 GAT 看不了信任配置，没有独立核验；
  0.2.0 第一次由 CI 发布 headless 包时若返回 404，先查这 10 个包的绑定。
- 不把 token 写进任何文件：`--userconfig` 指向的临时文件里只写 `${NPM_TOKEN}` 占位，token 只在当条命令的
  环境变量里出现。crates.io 的 token 只用于只读核对（`GET /api/v1/trusted_publishing/github_configs?crate=`）：
  aproxy #19617、aproxy-envelope #23481/#23482、aproxy-format #23480 均在。

## 2026-10-13：aproxy-desktop 的 crates.io 种子发布（0.2.0-alpha.1 之前）

- 新 crate 做不了可信发布首发。用户在会话里明确授权主代理用他 2026-10-11 给的 crates.io token 代办（「这件事你自己在终端做，之前的token还有效，等做完我再撤销权限」）。
- 做法：在仓库之外的临时目录建只有一行 crate 文档的占位包，`aproxy-desktop 0.0.0`，edition 2024，带 description、license、repository。
  - 本机 cargo 配了 rsproxy 镜像，要 `cargo publish --registry crates-io`。
  - token 只放在当条命令的 `CARGO_REGISTRY_TOKEN` 里，不进任何文件；临时目录用完即删。
- 随后用第 3 条的 API 创建可信发布配置 #25198（MoYeRanqianzhi/aProxy、release.yml、release），回读一致。字段照 aproxy 的 #19617 填写。
- 同一个 token 有 publish-new 与可信发布读写的权限。用户说做完就撤销，之后要用 token 得重新向所有者要。
- `GET /api/v1/me` 用 token 访问会回「只能在网站上操作」，不能拿它判断 token 是否有效；用可信发布配置的只读查询判断。
