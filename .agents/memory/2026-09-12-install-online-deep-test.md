# 2026-09-12 install 在线渠道深度实测（WSL）——测试 tag 发布机制全坑集

## 最终状态

- 测试版本 `v0.1.0-alpha.12t3` 三渠道全齐（GitHub/npm/crates.io，产物自报 = tag）
- WSL Debian 12 深度测试 **3 轮 × 16 场景全 PASS**：latest/特定版本/升级
  （alpha.9→latest）/降级拒绝与放行/断网全失败聚合/显式代理救场/settings
  链+代理/npm 单链/github 单链/url 模板+sha256/binstall/cargo 编译渠道/
  skill 断网放弃/--skills-only/坏渠道故障切换

## 实测挖出并修复的产品 bug（按暴露顺序）

1. **run_online 版本回写缺失**：DownloadCtx 用编译版本初建，解析目标版本后
   未回写——全渠道按 ctx.version 定位产物，`install <特定版本>` 查错版本
2. **npm 组包 skill zip**：zip 输出路径按 cd 后 cwd 解析（../../ 指向不存在
   的 .claude/skills/npm/）+ 平铺形态与消费侧（要 aproxy-cli/ 顶层前缀）不符
3. **在线下载产物缺 chmod 755**：--from 路径有 ensure_executable、在线路径漏
   ——unix 下载成功却 Permission denied
4. **skill 支线下载目录自毁**：zip 下载进 skills/.staging，install_skill_dir
   开头 remove_dir_all(.staging) 把刚下的 zip 删了——skill 支线永远失败。
   下载改独立 skills/.dl
5. **run_install_online 状态机缺口**：建 Marking 后直接进
   run_forward_from_staged，无实例场景跳广播直进 advance(Swapping) → 非法
   迁移。在线路径此前从未跑通到安装段（一直被前面的下载问题挡住）。
   现在建状态后补推进 Downloading→Downloaded + 补记 staged_path/sha256
6. **产品韧性改进**：gnu 产物试跑失败（CI glibc 2.39 > Debian 12 的 2.36）
   → musl 静态产物自动回退重跑链条（下载链条失败不回退——重复网络错误）；
   latest 解析 github API 限流时 npm dist-tags 兜底；--download-proxy 贯穿
   skill 支线（此前只认 settings）

## 测试 tag 发布机制的坑（发布工程教训）

- **产物版本必须对齐 tag**：CI build+publish 两个 job 都要对齐（publish 是
  独立 checkout）。crates.io 按 .crate 内 manifest 注册版本，不对齐 =
  测试 tag 抢注正式号。**alpha.10/alpha.11 已被污染，正式版只能顺延**——
  crates.io 不支持删版本。正式版现为 alpha.12（尚未发布）
- **测试 tag 一次性**：npm/crates.io 版本不可覆盖，发布失败（哪怕只在后段）
  后绝不能重打同号，必须 bump tN。t1 的 npm 已发、crates.io 没有——渠道
  不齐是废版本的常态
- 版本对齐 step 的三连坑：Windows runner 默认 pwsh（bash 语法全崩）→
  强制 shell: bash；BSD/GNU sed 的 -i 语法分裂 → 纯 python；Windows python
  默认 cp1252 读 UTF-8 Cargo.lock 崩 → 显式 encoding='utf-8'；Cargo.lock
  可能 CRLF → 正则 \r?\n；cargo publish 拒绝脏工作区 → --allow-dirty
- crates.io publish 后 .crate 有数分钟 S3 传播延迟（刚发布版本下载 403
  AccessDenied，等即可，非 bug）
- GitHub API 无 token 60 req/h/IP——共享代理出口下 latest 查询极易 403
  （这就是 npm dist-tags 兜底的价值）

## WSL 网络环境事实

- reqwest（rustls+webpki）直连 github API/registry.npmjs.org/crates.io 均 TLS
  正常；**release assets 域（objects.githubusercontent.com）直连失败**——
  下载走代理即可
- WSL 无 curl/wget/python3；openssl s_client 的 verify code 20 是 CA bundle
  缺失的假象（reqwest 实际能通）
- cargo（libcurl+openssl）需 SSL_CERT_FILE（Git 的 ca-bundle.crt 可从
  /mnt/d/Git 引用）+ https_proxy env
- 7890 代理监听 0.0.0.0，WSL 经宿主网关（ip route show default 第 3 字段）
  可达；代理出口 IP 跑 GitHub API 限流很快

相关：[[2026-09-12-release-version-alignment]]、[[2026-09-11-install-pitfalls]]
