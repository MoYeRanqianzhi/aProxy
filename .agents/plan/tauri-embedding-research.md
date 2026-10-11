# Tauri 2 编进 aproxy 本体：技术调研

> 计划 [release-0.2.0](release-0.2.0.md) W5 的附件：alpha.1 第 3 步（Tauri 外壳）与第 4 步（双变体发布流水线）动手前读。
> 调研日期 2026-10-12（子代理只读调研，未实际构建）。主代理据此做的取舍与待用户拍板的问题写在计划 W5「Tauri 嵌入调研结论」一节；
> 本文是证据与来源。第 3、4 步完成、结论沉淀进记忆后删除本文件。
> 标记约定：【证实】= 读到源码、文档或 issue 原文；【推断】= 推理，没实测。没有被实际构建验证的部分集中在 §9。

只读调研。我没改仓库，没跑 cargo，没碰 aproxy 进程和 `~/.aproxy`。

我把 tauri 2.12.3、tauri-build 2.7.2、tauri-codegen/macros 2.7.2、tauri-utils 2.10.1、tauri-runtime-wry 2.12.1、wry 0.57.0、tao 0.37.1、tauri-winres 0.3.6、embed-resource 3.0.12、window-vibrancy 0.8.1 的 .crate 解压到系统临时目录通读。同时读了 Cargo book、Tauri 官方文档、MS Learn 和相关 issue/PR。引用写作 `crate@版本 路径:行`，`[n]` 对应末尾来源表。

基线版本（crates.io，2026-10-12）：tauri 稳定版 2.12.3（另有 3.0.0-alpha.4，不建议用）、tauri-build 2.7.2、wry 0.57.0、tao 0.37.1、tauri-winres 0.3.6、embed-resource 3.0.12、window-vibrancy 0.8.1。

## 0. 会改变方案的发现

1. **【证实】Windows 阻断：tauri-build 2.7.x 默认会让依赖它的 crate 链接失败。**
   - 默认的 `staticVCRuntime=true` 会往 tauri-build 的 `OUT_DIR` 写一个 82 字节的假 `msvcrt.lib`，并用 `cargo:rustc-link-search` 发布（`tauri-build@2.7.2 static_vcruntime.rs:33-58`）。
   - `rustc-link-search` 会传给依赖者，抵消它的 `/NODEFAULTLIB` 却是包内作用域，传不出去。
   - 结果：任何依赖该 crate 的包（路径依赖、集成测试、下游库）在 x64 和 x86 MSVC 上链接失败（LNK4003、LNK1120）[9]。这正是 aproxy 依赖 aproxy-desktop 的布局。
   - 修复 PR #16247 在 2026-10-12 查询时仍是 open，2.7.2（2026-10-10 发布）源码仍含该逻辑。
   - 绕法：`tauri.conf.json` 写 `"build":{"windows":{"staticVCRuntime":false}}`。issue 实测矩阵中这一行能过，aarch64 据 reporter 读源码不受影响。
   - 仓库没有 `crt-static` 设置，aproxy 现在就是动态 CRT，关掉后与现状一致。

2. **【证实】带 Tauri 的 exe 没有 Common-Controls v6 清单就起不来。**
   - 它会在 main 之前以 `0xC0000139`（STATUS_ENTRYPOINT_NOT_FOUND）退出，`aproxy --version`、守护进程和看护者都会受影响。
   - `tests/` 下 8 个文件用 `env!("CARGO_BIN_EXE_aproxy")` spawn 默认特性构建的真 bin。所以 Windows 上 `cargo test` 要通过，清单或关 v6 是前提，不是可选项（根因见 §2）。

3. **【源码证实机制，实际表现待实验】`cargo publish` 的校验可能失败。**
   - `tauri-build` 会往包根写 `gen/schemas/*`（`tauri-utils@2.10.1 acl/schema.rs:323-330`，`tauri-build@2.7.2 acl.rs:121-142`）。
   - cargo 在校验构建前后对解包目录做哈希，有差异就报 "Source directory was modified by build.rs"（`cargo src/ops/cargo_package/verify.rs:119-130`）[5]。

4. **【证实】MSRV 冲突。** tauri 2.12.3、tauri-build 2.7.2、codegen、macros、utils、runtime-wry，以及 tauri 实际依赖的 muda 0.20.0 和 tray-icon 0.25.1，都是 `rust-version = "1.90"`，aproxy 现为 1.88。`desktop` 默认开，默认构建的有效下限就是 1.90。

5. **【证实】运行期失败多数是 panic，不是 Err。**
   - Linux 无显示时 tao 直接 `expect("Failed to initialize gtk backend!")`（`tao@0.37.1 linux/event_loop.rs:211-220`）。
   - `App::run` 里 setup（含 config 窗口创建，WebView2 缺失也走这条）失败会 `panic!("Failed to setup app")`（`tauri@2.12.3 app.rs:1444`）。
   - 需要在 Builder 之前自己预检（见 §1.2、§5）。

6. **【证实】CI 基础设施。**
   - `ubuntu-22.04` 和 `ubuntu-22.04-arm` 镜像 2026-09-17 起弃用，2027-04-17 退役，退役前有 brownout 故意失败 [19]。
   - Alpine 的 arm64 job container 不被 runner 支持，维护者表示不修 [19]。

7. **【证实】crate 名可用。** `aproxy-desktop` 和 `aproxy-ipc` 在 crates.io 的 API 返回 404（2026-10-12），尚未被占。

---

## 1. 作为库嵌入现有二进制

### 1.1 只用 cargo build 需要什么

不需要 tauri-cli。Tauri 自己的 bench 就是这样的 crate：`build.rs` 只有 `tauri_build::build()`，`main` 里 `Builder::default().run(generate_context!())`，用 `tauri.conf.json` 加 `public/`，由 cargo 构建 [18]。

需要的东西：

- **Cargo（aproxy-desktop）**：
  ```toml
  tauri = { version = "2.12", default-features = false,
            features = ["wry", "compression", "x11", "custom-protocol"] }
  [build-dependencies]
  tauri-build = "2.7"
  ```
  - `common-controls-v6` 是 tauri 的默认特性（`tauri@2.12.3 Cargo.toml:70-91`）。`default-features=false` 会同时去掉它，见 §2。
  - tauri-runtime-wry 在 tauri 里是 `default-features=false`，`x11` 和 `dbus` 靠 tauri 的同名特性转发。关了 default 后必须显式加 `x11`，否则 X11 会话没有支持。`dbus` 会引入 libdbus-1 的直接链接，可评估是否关。
  - `custom-protocol`：`generate_context!` 的 `dev` 标志是 `cfg!(not(feature="custom-protocol"))`（`tauri-macros@2.7.2 context.rs:155`）。dev 模式且配置了 `devUrl` 时资产为空，窗口去连 devUrl（`tauri-codegen context.rs:176-181`）。所以要开 `custom-protocol`，并且 `tauri.conf.json` 不写 `devUrl`。
- **build.rs**：`tauri_build::build()`。
- **tauri.conf.json**：
  - 只有 `identifier` 必填（`tauri-utils config.rs:4125`，无 serde default），且配置是 `deny_unknown_fields`。
  - `version` 可省，省略时取 `CARGO_PKG_VERSION`（`tauri-winres@0.3.6 lib.rs:149-153`）。
  - 最小集：`identifier`、`productName`、`build.frontendDist`、`build.windows.staticVCRuntime`、`app.security.csp`、`bundle.icon`。
  - 不要写 `devUrl` 和 `before*Command`。
- **图标**：
  - Windows 必须有 `.ico`，找不到直接报错 "`icons/icon.ico` not found; required for generating a Windows Resource file"（`tauri-build@2.7.2 lib.rs:860-870`）。
  - Linux 和 macOS 必须有 RGBA 的 `icons/icon.png`（`tauri-codegen context.rs:211-226`，`image.rs:84`）。
  - macOS 的 `.icns` 只在 dev 用。
- **capabilities/*.json**：可选，但 JS 调核心 API 必须有（见 §7）。
- **dist/**：编译期必须存在，否则 panic "`frontendDist`… doesn't exist"（`tauri-codegen context.rs:185-190`）。

### 1.2 事件循环与 tokio

- **线程要求**
  - `Builder::build` 在调用线程创建事件循环。macOS 必须在主线程，Windows 和 Linux 默认也要求主线程。
  - `Builder::any_thread()` 只在 windows/linux 暴露（`tauri@2.12.3 app.rs:1695-1706`）。
- **退出方式**
  - `App::run` 永不返回，结束时 `std::process::exit`。
  - `App::run_return` 返回退出码（`app.rs:1360-1430`）。`Builder::run` 等于 build 加 `App::run`，所以返回类型虽是 `Result`，实际不会返回。
- **与 `#[tokio::main]` 的关系**（aproxy 现状是 `src/main.rs:21` 的 `#[tokio::main] async fn main`）
  - 【证实】直接在里面调用可行：主线程在 `block_on` 里被事件循环占住，worker 线程继续跑 spawn 出的任务。
  - 【证实】`tauri::async_runtime::set(Handle::current())` 必须在 Tauri 首次使用异步运行时之前调，且只能调一次，重复调用 panic "runtime already initialized"（`async_runtime.rs:276-284`）。不调的话 Tauri 会惰性创建自己的第二个多线程运行时（`async_runtime.rs:243-248`）。
  - 【证实】在 `#[tokio::main]` 的主线程上调 `Handle::block_on` 或 `tauri::async_runtime::block_on` 会 panic，因为处于异步上下文 [21]。Tauri 自身除 asset 协议的测试外不调 `block_on`，但用户 `setup` 钩子里常见，要避开。
  - 【推断】更稳的做法：`aproxy desktop` 在进入 `#[tokio::main]` 之前同步处理。手建 `Runtime`，`let _g = rt.enter();`，再 `set(rt.handle().clone())`，事件循环在非异步上下文的主线程跑。代价是 main 要改成手动 runtime。
- **预检**
  - `tauri::webview_version()` 是公开 API（`lib.rs:209`）。Windows 调 `GetAvailableCoreWebView2BrowserVersionString`（`wry@0.57.0 webview2/mod.rs:1927`），Linux 返回 libwebkit 版本。
  - 再检查 `DISPLAY` 或 `WAYLAND_DISPLAY`。
  - 建议 `app.windows` 留空，自己用 `WebviewWindowBuilder` 建窗口并在建之前预检。

### 1.3 generate_context! 的路径语义

- 无参调用读 `$CARGO_MANIFEST_DIR/tauri.conf.json`，相对路径也相对它（`tauri-macros@2.7.2 context.rs:36-50,126-131`）。`CARGO_MANIFEST_DIR` 是被编译 crate 的目录。作为 crates.io 依赖时就是 `~/.cargo/registry/src/…/aproxy-desktop-x.y.z/`。
- `frontendDist` 相对配置文件所在目录（`config_parent.join(path)`，`tauri-codegen context.rs:185-193`）。
- 资产在宏展开时 brotli 压缩，写入 `OUT_DIR` 后 `include_bytes!` 嵌入。每个原文件还有一条 `const _: &[u8] = include_bytes!(原文件)` 供 rustc 追踪变更（`embedded_assets.rs:401`），改了 dist 会触发重编。
- ACL 数据（`acl-manifests.json`、`capabilities.json`）读自同一 crate 的 `OUT_DIR`（`context.rs:377-390`）。
- 新的 `tauri_build_context!` 加 `Attributes::codegen` 可把嵌入挪到 build.rs，也是个选项（`tauri-build@2.7.2` 的 `codegen` 特性）。

### 1.4 tauri-build 的 build.rs 放哪

【证实】必须放 aproxy-desktop 自己的 build.rs，理由有四条：

- `generate_context!` 读的是同包 `OUT_DIR` 里的 ACL 数据。
- `cfg(dev)` 别名按本包设置。
- tauri-build 的配置、`Cargo.toml` 和 `capabilities/` 都取自 CWD（包根）。
- `DEP_TAURI_DEV` 只传给直接依赖 tauri 的包。

aproxy 根包的 build.rs 是否要做事取决于清单策略（§2），其余不需要。

### 1.5 Windows 资源与清单怎样落到 aproxy.exe

- **资源内容**：tauri-build 在 Windows 上编一个资源：图标 id 32512、版本信息、默认清单（只含 Common-Controls v6 依赖，`windows-app-manifest.xml`）。流程是 `tauri_winres::compile` 调 `embed_resource::compile(rc, NONE)`（`tauri-winres@0.3.6 lib.rs:541`）。
- **投递方式取决于调用方包有没有 bin**（`embed-resource@3.0.12 lib.rs:429-449`）：
  - 有 bin（`Cargo.toml` 含 `bin` 键，或存在 `src/main.rs` 或 `src/bin/`）：发 `cargo:rustc-link-arg-bins=<resource.lib>`。
  - 没有 bin：回退为 `rustc-link-search` 加 `rustc-link-lib=dylib=resource`。
- **Cargo 语义**
  - 构建脚本指令只"influence compilation of the package"，`rustc-link-arg-bins` 是 "only when building a binary target" [1]。
  - `rustc-link-lib` 的 `-l` 只传给包的 lib 目标，无 lib 时才传所有目标 [1]。
  - 因此 `-bins` 的资源永远到不了上层包的 exe。这就是 tauri#13948（"app imported into a crate as a library"）失败的原因。维护者原话：要么关 v6 feature，要么让 importer 自己的 build script 加清单 [7]。
  - tauri 自带的 `__TAURI_WORKSPACE__` 只在 tauri crate 自己的 build.rs 里给它自己的测试嵌清单（`tauri@2.12.3 build.rs:272-278,483-500`）。对下游无效，#13948 里有人试过。
- **【推断】aproxy-desktop 是纯 lib 时**走回退路径，`-l dylib=resource` 记在 rlib 里，理论上会随链接带到 aproxy.exe。我没找到公开案例验证。
- **三个坑**
  - 给 aproxy-desktop 加 `src/main.rs` 或 `src/bin/`，会静默翻回 `-bins`，清单从此到不了 aproxy.exe。
  - 回退产物名固定为 `resource.lib`。aproxy 若也用 embed-resource，rc 文件别叫 resource，否则同名互相覆盖。
  - 自检办法：Windows 默认窗口图标取自 exe 的图标资源（`tauri@2.12.3 image/mod.rs:61-85`）。资源没进 exe 时图标为 None，只打日志。
- **不要依赖回退。** 清单问题的稳妥做法见 §2。

---

## 2. RT-05（tauri#13419）

**根因链**

1. 【证实】tauri 默认特性 `common-controls-v6` 会打开 muda 的 About 对话框（`muda@0.21.2 windows/mod.rs:1244-1255`）。
2. 【证实】同一特性还会打开 tauri-runtime-wry 的错误对话框（`dialog/windows.rs:26-90`）。它们都静态导入 comctl32 的 `TaskDialogIndirect`，该函数只有 v6 导出。
3. 【证实】tauri-build 通过 embed-resource 的 `rustc-link-arg-bins` 只给 bin 嵌清单。lib 单测 harness、集成测试 exe 和 doctest exe 没有清单，加载时以 `0xC0000139` 退出 [6]。
4. 【证实，社区实测】issue 评论的 `dumpbin` 显示失败的 lib 测试 exe 导入 `TaskDialogIndirect`、`SetWindowSubclass`、`RemoveWindowSubclass`、`DefSubclassProc`，且无 `RT_MANIFEST`。System32 的 v5.82 只缺 `TaskDialogIndirect`。
5. 评论还指出：`rustc-link-arg-tests` 覆盖不到 lib 单测 exe；cargo 无论有没有测试都会构建并运行 lib 单测 exe；把测试挪进 `tests/` 没用。
6. 上游 issue 仍 open，最后评论 2026-10-06，维护者只说"看能不能做成选项"。

**在你们的布局下的具体做法**

1. **aproxy lib 不引用 aproxy_desktop。**
   - 【推断】rustc 不链接未引用的 `--extern` crate，lib 测试 exe 和集成测试 exe 因此不会带 Tauri。
   - 验证：`dumpbin /imports target\debug\deps\<测试>.exe` 不含 comctl32。
2. **aproxy-desktop 设 `[lib] test = false` 和 `doctest = false`。**
   - Cargo 默认 lib 目标 `test=true`，doctest 同理 [4]。
   - 不设的话 `cargo test --workspace` 会构建并运行它的空测试 exe 和每个 doctest exe，它们链接了 Tauri。
3. **aproxy bin 里引用 aproxy_desktop 的代码加 `#[cfg(all(feature="desktop", not(test)))]`。** 否则 bin 的单测 harness 也会链接 Tauri。`server.rs` 和 `commands/*` 带单测，这条不能漏。
4. **bin 本体必须能启动。** 二选一，或叠加：
   - **A：关掉 v6。** 即 §1.1 的 `default-features=false`，改用 `MessageBoxW` 兜底（muda `windows/mod.rs:1244-1253`，`tauri-runtime-wry dialog/windows.rs:26-45`）。
     - #13954 里有人用 `default-features=false, features=["wry","compression"]` 解决，#13948 里维护者也提到 [8][7]。
     - 剩下的 comctl32 导入 v5 就有（社区 dump）。代价是 About 和"WebView2 缺失"对话框降级为纯 MessageBox，没有链接。
   - **B：aproxy 自己的 build.rs 嵌 bin-only 清单。** 条件是 `CARGO_FEATURE_DESKTOP` 且 windows-msvc：
     ```rust
     println!("cargo::rustc-link-arg-bins=/MANIFEST:EMBED");
     println!("cargo::rustc-link-arg-bins=/MANIFESTINPUT:{abs_xml}");
     ```
     - XML 复用 `tauri-build/src/windows-app-manifest.xml`。
     - 同时 aproxy-desktop 用 `WindowsAttributes::new_without_app_manifest()` 避免双清单（我推断重复会报 CVT1100，没验证）。
     - `/MANIFEST:EMBED` 加 `/MANIFESTINPUT` 是 Tauri 核心成员在 #13419 里给的绕法，Tauri 自己的 `embed_manifest_for_tests` 同款。
   - 还可以用 `/DELAYLOAD:comctl32.dll`，审计报告提到过，我没见 Tauri 案例，仅作选项。
5. **CI 加两道门**
   - 每个 Windows 产物 `--version` 冒烟，非零即失败。
   - `cargo tree -e normal --no-default-features` 断言无 tauri。
6. 同时必须应用第 0 节第 1 条的 `staticVCRuntime:false`。

---

## 3. crates.io 打包前端产物

- **include 带上 dist**
  - 指定 `include` 时不应用 gitignore，所以被忽略的 `dist/` 照样进包 [2]。
  - 建议：
    ```toml
    include = ["/src/**","/build.rs","/Cargo.toml","/tauri.conf.json","/tauri.*.conf.json",
               "/capabilities/**","/icons/**","/dist/**","/README.md","/LICENSE"]
    ```
  - 子目录里有 `Cargo.toml` 的会被当子包整个跳过 [2]。
  - 用 `cargo package --list` 核对。
- **frontendDist 必须在包根之内。**
  - 【推断】包内容只来自包根目录，Tauri 模板常见的 `../dist` 打不进包。
  - 现有 `desktop/` 的 Svelte `src/` 会和 Rust 的 `src/` 冲突。可行布局：crate 根为 `desktop/`，前端挪到 `desktop/ui/`，Vite 的 `outDir` 设为 `../dist`，`include` 不含 `/ui/**`。
  - 另一种：crate 在 `desktop/src-tauri/`，发布前 CI 把 dist 拷进去。
- **发布前 CI**
  - 先 `setup-node`、`npm ci`、`npm run build`（现有 build 脚本含 `svelte-check --fail-on-warnings`），再 `cargo package --list` 断言 `dist/index.html` 在包内。
  - `desktop/package.json` 目前没有 `@tauri-apps/api`，需要加。
  - 【证实】Tauri 官方 Vite 指南的 `build.target` 是 Windows 用 `chrome105`、其余 `safari13`；现有 `desktop/vite.config.ts` 是 `es2022`，macOS 老系统可能不够。
- **大小上限**
  - 【证实】crates.io 对 `.crate` 的限制是 10MB [3]。
  - 实测原型 dist 共 461KB，`tar.gz` 206KB（gzip -9：JS 81KB、CSS 17KB、woff2 两个共 104KB 基本不可压）。远低于上限。
- **verify 构建的代价**
  - `cargo publish` 的校验会编译整个包。ubuntu 发布 job 要装 webkit 开发包，并承担 Tauri 的编译时间。
  - 用多个 `-p` 时 aproxy 的校验会用本地刚打的 aproxy-desktop（release.yml 已有此注释）。
- **gen/schemas 问题**（第 0 节第 3 条）
  - 内容含 `{target}-schema.json`，按构建平台命名（linux 加 desktop）。
  - 办法一：发布 job 在同一 Linux runner 先 build 一次，把 `gen/schemas/**` 纳入 include，校验时内容一致就不会报错。
  - 办法二：对 aproxy-desktop 用 `--no-verify`，另做一次解包构建验证。
  - 本机/仓库的 `.gitignore` 要忽略 `desktop/gen/`。
- **只读环境**：`cargo install` 编译依赖时 tauri-build 要写依赖源码目录，只读的 vendor 或 Nix 环境会失败【推断】。

---

## 4. 各目标的构建依赖与可行性

| 目标 | 结论 | 依据与条件 |
|---|---|---|
| Windows x64、x64-v3 | 能 | 受 #16238 影响，需 `staticVCRuntime:false`；v3 只是 RUSTFLAGS，无已知冲突【推断】 |
| Windows i686 | 有条件 | 文档：`rustup target add i686-pc-windows-msvc` [15]；`webview2-com-sys` 带 x86 加载器；同受 #16238（该逻辑覆盖 x86 和 x86_64） |
| Windows arm64 | 有条件 | 需 "C++ ARM64 build tools"，原生 `windows-11-arm` runner；WebView2 有 ARM64 加载器和运行时安装包 [16]；#16238 的桩逻辑对 aarch64 提前返回（reporter 读源码，未实测） |
| Linux gnu x64、x64-v3、arm64 | 有条件 | 见下 |
| Linux musl x64、arm64 | 有条件，无公开先例 | 见下 |
| macOS arm64、x64 | 能 | WKWebView 是系统组件；`macos-latest` 现在是 macOS 26 arm64，`macos-13` 已不在列表，Intel 用 `macos-15-intel`；arm runner 交叉出 x86_64 是常规做法。默认 `minimumSystemVersion` 是 10.13，文档写 10.15+ |

### Linux gnu

- **构建系统与发行版**
  - 官方文档要求用"想支持的最老基线系统"构建，建议用 Docker 或 GitHub Actions，例举 Ubuntu 22.04 和 Debian 12，因为它们的标准源有 `libwebkit2gtk-4.1-dev` [14]。
  - 包搜索确认：Ubuntu 20.04（focal）没有 4.1，jammy（22.04）在 universe 里有，Debian bullseye 以前没有，bookworm 有 [20]。
  - 官方 apt 清单：`libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev` [13]。
  - 精简判断【推断】：`libxdo` 只在 `linux-libxdo` 特性下需要；tray 走 dlopen，构建期不一定要 appindicator 开发包；rustls 方案不需要 `libssl-dev`。以实验为准。
- **glibc 下限与 webkit 版本**
  - 【推断】glibc 下限等于构建机：22.04 容器约 2.35，不能再用 zigbuild 压到 2.28，因为链接的是宿主 GTK 和 WebKit 库。以 `objdump -p | grep GLIBC_` 为准。
  - 【证实】tauri 启用 `webkit2gtk/v2_40`（`tauri@2.12.3 Cargo.toml:343-346`），运行期需 WebKitGTK ≥ 2.40。
  - 【推断】Rust 在 Linux 默认 full RELRO，符号启动即解析。过旧的 webkit 会在启动时报错，连 `--version` 都起不来；`readelf -d` 可验证。
- **runner 选择**：【推断】用 `container: ubuntu:22.04` 跑在 `ubuntu-24.04` 和 `ubuntu-24.04-arm` 上，躲开 22.04 runner 的退役。x64 和 arm64 同法，arm 上要实测。公共仓库 arm runner 为 4 核 16GB，免费 [19]。

### Linux musl

- **能找到的证据**
  - Alpine 3.22 community 有 `webkit2gtk-4.1` 和 `-dev`，x86_64 和 aarch64 都有（2.48.1-r1）[20]。
  - Tauri 官方 prerequisites 有 Alpine 页签，附 `apk add build-base webkit2gtk-4.1-dev …`，并提示 "Alpine targets the musl C library, so Rust builds link a number of system libraries statically"，列了 `*-static` 包，并说有些依赖没有静态包 [13]。
  - tauri#12296（Alpine 构建错误）至今 open，唯一评论说要静态版、很多不存在 [10]。
  - gtk-rs-core#387：rustup 的 musl 工具链默认 `crt-static`，GTK 程序启动即段错误（共享库没被加载，函数指针为 NULL）。用 `RUSTFLAGS="-C target-feature=-crt-static"` 或 Alpine 打补丁的 `rust` 包可避免 [12]。
  - `-crt-static` 的产物会动态链接 `libgcc_s`，运行期需要 libgcc（rust#82521）。
  - WebdriverIO 文档把 Alpine 标成不支持（第三方说法，无细节）。
  - 我没找到任何公开的 Tauri 2 musl 预编译发布。
- **产物与 CI**
  - 产物只能跑在装了 webkit2gtk-4.1 的 musl 系统。
  - CI：Alpine 容器里的 JS action 在 arm64 不被支持 [19]，应在宿主 checkout 后用 `docker run` 跑 Alpine 构建。
  - 建议在资产清单里标 optional。

---

## 5. 运行时依赖与降级

### Windows

- **没装 WebView2 时 exe 能否启动**
  - 【证实】MSVC 目标静态链接加载器（`webview2-com-sys@0.39.1 src/lib.rs:15-22`），exe 本身能启动。
  - 【推断】代理和 CLI 不受影响，需实验确认。
  - 创建 webview 时：非 debug 构建先弹对话框 "Could not find the WebView2 Runtime…"，再返回 `WebviewRuntimeNotInstalled`（`tauri-runtime-wry@2.12.1 lib.rs:4678-4693`）；setup 失败再触发 panic。建议预检。
- **谁预装了运行时**
  - MS 文档：Windows 11 自带；Windows 10 "绝大多数"已装，"少数设备没有"，建议处理这种边缘情况 [16]。
  - Tauri 文档说 "Windows 10 1803+ 预装"，口径更强，与 MS 不一致。
  - 对 LTSC 和 Server，两边都没有明确说法。
- **检测**：注册表 `HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}` 的 `pv`，或 `GetAvailableCoreWebView2BrowserVersionString` [16]。
- **安装模式在"不用 tauri-cli 打包"时还有没有意义**
  - `downloadBootstrapper`、`embedBootstrapper`、`offlineInstaller`、`skip` 是 WiX 和 NSIS 安装器的选项 [15]。没有 bundler 就没有安装器，这四个没有意义。
  - 唯一例外是 `fixedRuntime`。
    - tauri-build 会把它的 `path` 当资源拷到目标目录（`tauri-build@2.7.2 lib.rs:750-757`）。
    - 运行时会设 `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` 指向 `<exe目录>/<path>`（`tauri@2.12.3 app.rs:2483-2498`）。
    - 但负载约 180MB 以上，不适合单文件 CLI。
- **可行的做法**：自己检测，缺失时提示或静默运行 MS 的 bootstrapper。MS 文档给的命令是 `MicrosoftEdgeWebview2Setup.exe /silent /install`，非提权会装成 per-user [16]。
- **模拟"缺失"**：给 `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` 一个不存在的目录即可，Tauri 源码注释明说该变量会被 `GetAvailableBrowserVersionString` 读取。

### Linux

- **缺库时的表现**
  - `ld.so` 的职责是"找到并加载程序所需的共享库，然后运行它"。DT_NEEDED 库在启动时全部加载，"error while loading shared libraries" 发生在 main 之前，`--version` 也跑不起来 [23]。
  - 懒绑定只推迟函数调用的解析，不推迟库加载 [23]。`--as-needed` 对确实用到的库没用。
- **选项**（只列事实，不选）

  | 选项 | 事实 |
  |---|---|
  | L1：Linux 默认产物 headless，UI 另出 | 默认路径零风险；命名与其他平台不对称；`aproxy desktop` 报错并指引。Cargo 支持目标专属可选依赖，tauri 自己的 `tray-icon` 就是这样（`tauri Cargo.toml:356`），可让 `desktop` 特性在 Linux 上空转 |
  | L2：命名对称，安装器探测 | 手动下载、npm、binstall 仍可能拿到起不来的 UI 版 |
  | L3：双可执行文件，`aproxy desktop` exec 同伴 | 只有同伴链接 GTK 和 WebKit；多一个文件，或内嵌后自解压；偏离"单文件"；`aproxy` 本体仍可无库启动 |
  | L4：cdylib 用 libloading 在 `aproxy desktop` 时 dlopen | 同上但进程内；Tauri 在 cdylib 里要有自己的 std 和 tokio，复杂 |
  | L5：Implib.so 生成延迟 dlopen 桩 | 工具存在，x86_64 和 aarch64 的 glibc 目标受支持；不支持数据符号，"symbol versions are not handled at all"，首次调用的 dlopen 对异步信号不安全；README 没提 musl [22]；对数十个 GTK 和 WebKit 库没有 Tauri 先例 |

  - 我没找到 wry、tao 或 Tauri 有 webkit 的 dlopen 或可选加载机制。
  - 已有的 dlopen 先例只有边缘库：Linux 托盘走 `libappindicator-sys` 的 libloading（`libappindicator-sys@0.9.0 src/lib.rs:13-47`，缺库时 panic）、tao 的 x11-dl 和 dlopen2（libunity）。
- **其他**
  - 【证实】无显示时 `Builder::build` 内部 panic（第 0 节第 5 条）。
  - 【推断】Linux 的 CLI 冷启动会为 UI 版多付一次 GTK 和 WebKit 库加载。`aproxy status` 被 agent 频繁调用，这笔成本会被放大，见 §6。

### macOS

WebKit 是系统框架，无缺失问题。

---

## 6. 体积与常驻内存

**二进制体积**

- 来源：tauri-apps/benchmark_results（Tauri CI，2026-10-10，`tauri-recent-*.json`）[18]。
- 这是 hello world，用 tauri 默认特性，release 配置为 `panic=abort`、`opt-level="s"`、`lto=true`、`codegen-units=1`、`strip=true`。

  | 平台 | tauri hello world | 同期 wry hello world |
  |---|---|---|
  | Linux | 3,047,944 B | 507,704 B |
  | macOS | 2,884,464 B | 635,168 B |
  | Windows | 2,831,360 B | 345,600 B |

- 没有 Windows i686 和 arm64 的公开数据。
- 对比：aproxy headless 4.5–6.5 MiB。aproxy 用 `opt-level="z"` 且 `panic=unwind`，共享的依赖（tokio、serde 等）不会重复计。
- 前端资产嵌入后约 +0.2MB（brotli 估算）。
- 【推断】desktop 变体比 headless 大约 +3 到 +4 MiB，需构建对比确认。
- 其他：Tauri 自己统计 `cargo tree` 依赖数约 740–830。

**常驻内存**

- 没有公开数据能回答"链接了库但没初始化 webview 时多占多少"。
- 基准里的 `max_memory`（Linux hello world 410,988,544 B）是 `mprof -C` 含子进程的整个 webview 进程树峰值，不适用。
- 【推断】机制：
  - Linux：`ld.so` 要映射并重定位约 15–20 个直接 DT_NEEDED 库（webkit2gtk、jsc、soup3、gtk3、gdk3、pango、cairo、gio、glib、gobject、dbus-1 等），并运行它们的构造函数，对每次 CLI 启动都有成本。
  - Windows：多出 comctl32、dwmapi、shell32 等 DLL 导入，WebView2 在创建窗口前不加载。
  - macOS：多链接 AppKit 和 WebKit，系统共享缓存里的开销小。
- 需实验（§9 的 U3 和 U6）。

---

## 7. 自定义协议与安全

**协议与 IPC**

- 【证实】Windows 上前端是 `http://tauri.localhost`（`useHttpsScheme` 时 https），其他平台是 `tauri://localhost`（`tauri@2.12.3 protocol/mod.rs:32-39`）。
- IPC 走 `ipc://localhost/<cmd>`，Windows 对应 `http://ipc.localhost`。

**CSP**

- 【证实】CSP 要在配置里设（`app.security.csp`），Tauri 会在编译期给本地脚本加哈希、给样式和外部脚本加 nonce [17]。
- 官方示例有 `connect-src: ipc: http://ipc.localhost`。
- 不联网的草案（【推断】未测）：
  ```
  default-src 'none'; script-src 'self'; style-src 'self'; style-src-attr 'unsafe-inline';
  img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost;
  object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'
  ```
- 重要发现：现有 `desktop/dist` 的 JS 里有 6 处 Svelte 模板字符串带静态 `style="animation-delay:…"`。严格的 `style-src 'self'` 会拦截它们，nonce 对 style 属性无效。要么加 `style-src-attr 'unsafe-inline'`（CSS 里的 `url()` 仍受 `img-src` 和 `font-src` 管，不会因此联网），要么改成 CSS 变量或类。`.style.cssText` 走 CSSOM，不受影响。
- 保持 `dangerousDisableAssetCspModification` 为默认值，让 Tauri 注入 nonce。
- 可选 `freezePrototype:true`，注意会破坏改内置原型的库。
- 导航限制用 `on_navigation`。官方示例写的 `url.scheme()=="tauri"` 在 Windows 上不对，Windows 应判 `tauri.localhost` 主机。
- 【证实】Tauri 在 Linux 和 Windows 上会强制建 `app_local_data_dir()` 作为 webview 数据目录（`manager/webview.rs:559-566`），可显式设 `data_directory` 以收进 APROXY_HOME。

**capabilities 与权限**

- 【证实】能力文件放 `capabilities/`，默认全部启用；配置 `app.security.capabilities` 后只用列出的；按窗口 label 匹配 [17]。
- 核心插件命令（`plugin:window|…` 等）必须有 ACL 才放行；自定义命令默认对所有本地 webview 放行，除非定义了 app manifest（`webview/mod.rs:2080` 的 `plugin_command.is_some() || has_app_acl_manifest || !is_local`）。官方文档同说 [17]。
- 要把自定义命令收紧：build.rs 里 `Attributes::new().app_manifest(AppManifest::new().commands(&[...]))`，然后在 capability 里列 `allow-<命令>`。
- 最小做法：单一 capability 文件，`windows:["main"]`，只列用到的权限，不用 `core:default`；`app.withGlobalTauri` 保持 false。
- 注意 §8 里 titlebar 的两条命令此时也要出现在 capability 里。

**允许用户注入扩展**

官方没有扩展沙箱的概念，只有这些手段：

- **CSP**：扩展的 JS 和 CSS 受它约束，只能从 `script-src` 和 `style-src` 允许的来源加载。内联注入要有 nonce，所以加载器要让扩展文件来自 `'self'` 或登记的自定义协议（并写进 CSP）。
- **Isolation pattern** [17]
  - 在沙箱 iframe 里的 hook 对所有 IPC 消息做校验，消息用逐次启动随机的 AES-GCM 密钥加密。
  - 针对的是依赖供应链风险。
  - 它分不清调用方是 UI 还是扩展。
  - 隔离应用里 ES Module 加载不了（Windows 下沙箱 iframe 加载不了外部文件，Tauri 内联脚本）。
- **独立 webview 或窗口加独立 capability**：这是硬隔离，但视觉上不是同一页面。
- **扩展与 UI 同一 JS 域时**：【推断】扩展能直接调 invoke，JS 层的同意包装拦不住，真正的"写操作需同意"只能在 Rust 侧做。
- **invoke key**
  - 【证实】`__TAURI_INTERNALS__` 和每次启动随机的 invoke key 通过主框架专属的初始化脚本注入（`manager/webview.rs:163-189`），`on_message` 校验该 key（`webview/mod.rs:2011`）。
  - 同源策略下，沙箱 iframe 读不到 parent 的 key，也无法伪造消息【推断】。
  - 但官方文档说 "On Linux and Android, Tauri is unable to distinguish between requests from an embedded `<iframe>` and the window itself" [17]。
- **一个陷阱**：【证实】`is_local_url` 把用户注册的自定义协议视为本地（`webview/mod.rs:1978-2001`）。经自注册协议加载的扩展页面，在 ACL 看来是本地来源。

---

## 8. 两个 skill 的要点与对嵌入布局的影响

**tauri-custom-titlebar**（验证于 tauri 2.12.0 / wry 0.57.0 / tao 0.37.1 / windows 0.62.2；当前解析为 tauri 2.12.3 + 同一 wry/tao）

- **配置**
  - `decorations` 保持默认 true。
  - macOS 用 `titleBarStyle:"Overlay"`、`hiddenTitle`、`trafficLightPosition`。
  - 不需要 capability。
  - 平台专属配置 `tauri.<platform>.conf.json` 要重复整个窗口对象，因为 JSON merge patch 会替换数组。
- **Cargo**
  - `[target.'cfg(windows)'.dependencies] windows = { version="0.62", features=[Win32_Foundation, Win32_Graphics_Dwm, Win32_Graphics_Gdi, Win32_System_LibraryLoader, Win32_UI_Controls, Win32_UI_HiDpi, Win32_UI_Shell, Win32_UI_WindowsAndMessaging] }`，和 tauri 2.12.3 同为 0.62（`tauri Cargo.toml:420-421`）。
  - 另需 serde derive。
- **注册**
  - 命令 `titlebar_insets` 和 `set_caption_color` 加入 `generate_handler!`。
  - 在 `setup` 里调 `titlebar::install`。
  - 前端需 `@tauri-apps/api`。
- **嵌入布局调整**
  - 模块放进 aproxy-desktop 的 `cfg(windows)`。
  - 若按 §7 定义了 app manifest，这两条命令也要在 capability 里放行。skill 说"不需要 capability"，只在未定义 app manifest 时成立【推断】。
  - 它依赖 wry 私有的 `WRY_WEBVIEW` 容器类，`install` 找不到会返回 Err。嵌入布局里建议降级为原生标题栏，而不是让 setup 失败，因为 setup 失败会 panic。
  - 换 tauri 或 wry 后要重跑 skill 的验证脚本。
  - `cargo install` 要加 `--locked` 才用 aproxy 发布的 Cargo.lock，否则解析会漂。
  - `SetWindowSubclass` 和 `DefSubclassProc` 在 comctl32 v5 就有，关 v6 不影响它。

**tauri-windows-mica**（验证于 Windows 11 25H2）

- **配置**
  - 窗口 `transparent:true`、`visible:false`，页面脚本调 `show()`。
  - 计划里的"配置不写 `windowEffects` 和 `backgroundColor`、运行时 `setEffects`"可行。
  - 权限 `core:window:allow-show`、`allow-set-effects`、`allow-set-theme`。
  - 主题走 `theme` 或 `setTheme`，不要用 `micaDark` 或 `micaLight`。
- **Cargo**
  - 不需要额外 crate，window-vibrancy 0.8.1 已经是 tauri 在 windows 和 macOS 上的依赖（`tauri Cargo.toml:407-418`）。
- **没有 Mica 的 Windows 11**
  - 【证实】window-vibrancy 0.8.1 的 `apply_mica` 逻辑：build ≥ 22523 用文档化的 `DWMWA_SYSTEMBACKDROP_TYPE`；22000–22522 用未文档化属性 1029；更低返回 `Err(UnsupportedPlatformVersion)`（`windows.rs:104-121,283-291`）。
  - Tauri 把这个 Err 丢掉（`tauri vibrancy/windows.rs:16-52`，`window/mod.rs:444,2177`）。
  - 所以 Rust 端要直接调 window-vibrancy 才能拿到结果，须钉 `window-vibrancy = "=0.8.1"` 并限 `cfg(windows)`，否则 macOS 上会和 Tauri 带的版本重复符号 [11]。
- **macOS**
  - 【证实】`macOSPrivateApi` 自 tauri 2.12.1 起无操作，透明始终可用（`tauri-utils config.rs:3371-3373`）。
  - 配置里别写它，`tauri-build` 会校验 Cargo 特性与配置一致（`manifest.rs`）。
- **启动兜底**
  - `visible:false` 加页面 `show()`，页面脚本、CSP 或权限出错就永不显示窗口。
  - 【推断】Rust 侧加兜底超时 show 比较稳。

---

## 9. 必须靠实际构建确认的未知项与最小实验

所有实验用临时工作区加独立 `CARGO_TARGET_DIR`、debug 配置、`-j2`，避免吃满内存。涉及 aproxy 运行的一律用独立 `APROXY_HOME` 和非生产端口。

| # | 未知项 | 最小实验 | 判据 |
|---|---|---|---|
| U1 | 纯 lib 的 aproxy-desktop 回退路径是否真把资源带进 aproxy.exe；#16238 在你们布局下是否复现；关 v6 后 exe 能否无清单启动 | 临时工作区：`desk`（lib，build.rs 调 tauri-build，最小 dist 和图标）加 `app`（bin 加 `tests/smoke.rs` spawn `CARGO_BIN_EXE_app --version`）。矩阵：`staticVCRuntime`{默认, false} × 清单{默认回退, A 关 v6, B 根 build.rs} | `cargo build` 是否 LNK4003；`app.exe --version` 退出码；`dumpbin /imports` 有无 `TaskDialogIndirect`；`mt.exe -inputresource:app.exe;#1` 看清单；测试 exe 不含 comctl32 |
| U2 | `gen/schemas` 是否让 `cargo package` 校验失败；预生成并 include 是否能过 | `cargo package -p desk --allow-dirty`，再预生成后重试 | 是否出现 "Source directory was modified by build.rs"；`cargo package --list` 含 `dist/index.html` |
| U3 | Linux UI 版 DT_NEEDED、glibc 下限、缺库失败形态、启动与内存增量 | ubuntu:22.04 容器构建两个变体；`debian:12-slim`（无 GTK）里 `--version`；装 GTK 后对比 | `readelf -d | grep -E 'NEEDED|FLAGS'`、`objdump -p | grep GLIBC_`、`/proc/PID/smaps_rollup` 的 Rss 和 Pss、`status` 冷启动耗时 |
| U4 | Alpine/musl 带 UI 版可行性 | alpine:3.22：分别用 `apk add rust cargo` 和 rustup 加 `-C target-feature=-crt-static` 构建；Xvfb 下运行 `aproxy desktop`；arm64 在 `ubuntu-24.04-arm` 用 `docker run` | 是否链接成功、`ldd` 是否含 libgcc_s、窗口能否出现 |
| U5 | Windows 无 WebView2 的表现 | 设 `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` 为不存在目录，跑 `webview_version()` 探针和 `aproxy desktop`；另在无 WebView2 的虚机确认 `--version` 和 `start` | 对话框、panic、退出码；headless 路径不受影响 |
| U6 | 体积与内存增量（三平台） | 同一提交构建 headless 和 desktop（release，lto=fat） | 文件大小；Windows `WorkingSet64` 和 `PrivateMemorySize64`、`dumpbin /dependents`；macOS `vmmap`；CLI 冷启动 |
| U7 | MSRV 解析 | 临时工作区 `cargo +1.88 generate-lockfile`（resolver 3） | 选到哪个 tauri；想用 2.12 则 `rust-version` 需升 1.90 |
| U8 | CSP 与 Svelte 产物 | 用目标 CSP 头加载 dist，看 `securitypolicyviolation`；测 `style-src-attr` 在 WebKitGTK ≥ 2.40 和 WKWebView 的支持 | 违规条目；6 处内联 style |
| U9 | 沙箱 iframe 与自注册协议的 IPC 行为 | Tauri 窗口里放 `<iframe sandbox="allow-scripts">` 与 `src=自注册协议`，试 `__TAURI_INTERNALS__.invoke` 和原生 postMessage | 三平台的 `Origin` 与 ACL 结果 |
| U10 | 流水线 | `container: ubuntu:22.04` 跑在 24.04 与 24.04-arm 上能否跑完 checkout 和 artifact；`macos-latest`（7GB）上 fat LTO 是否 OOM；总时长 | 作业成功与否和耗时 |
| U11 | 无 node 的 `cargo install` 全流程 | 发布 alpha 后，在干净 Windows、Linux、macOS 上 `cargo install aproxy --locked` | 唯一能证明"用户机器无需 node"的验证 |
| U12 | cargo-deny 对 Tauri 依赖树 | 对临时工作区跑一次 | 补 licenses 白名单和 RUSTSEC 例外 |
| U13 | Linux 托盘缺库的行为 | 无 libayatana 时调 `TrayIconBuilder::build` | panic 能否 `catch_unwind` |

## 来源

- [1] Cargo book 构建脚本：https://doc.rust-lang.org/cargo/reference/build-scripts.html
- [2] Cargo book include/exclude：https://doc.rust-lang.org/cargo/reference/manifest.html#the-exclude-and-include-fields
- [3] Cargo book 发布（10MB）：https://doc.rust-lang.org/cargo/reference/publishing.html
- [4] Cargo book 目标（test/doctest，集成测试构建 bin）：https://doc.rust-lang.org/cargo/reference/cargo-targets.html
- [5] cargo `src/ops/cargo_package/verify.rs`：https://github.com/rust-lang/cargo/blob/master/src/ops/cargo_package/verify.rs
- [6] tauri#13419：https://github.com/tauri-apps/tauri/issues/13419
- [7] tauri#13948：https://github.com/tauri-apps/tauri/issues/13948
- [8] tauri#13954：https://github.com/tauri-apps/tauri/issues/13954
- [9] tauri#16238：https://github.com/tauri-apps/tauri/issues/16238 ，PR #16247：https://github.com/tauri-apps/tauri/pull/16247
- [10] tauri#12296（Alpine）：https://github.com/tauri-apps/tauri/issues/12296
- [11] tauri#15478（window-vibrancy 版本冲突）：https://github.com/tauri-apps/tauri/issues/15478
- [12] gtk-rs-core#387：https://github.com/gtk-rs/gtk-rs-core/issues/387 ；rust#82521：https://github.com/rust-lang/rust/issues/82521
- [13] Tauri prerequisites：https://v2.tauri.app/start/prerequisites/
- [14] Tauri Debian 分发：https://v2.tauri.app/distribute/debian/
- [15] Tauri Windows 安装器：https://v2.tauri.app/distribute/windows-installer/
- [16] MS Learn WebView2 分发：https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution
- [17] Tauri CSP https://v2.tauri.app/security/csp/ ，capabilities https://v2.tauri.app/security/capabilities/ ，isolation https://v2.tauri.app/concept/inter-process-communication/isolation/ ，security https://v2.tauri.app/security/ ，Vite 指南 https://v2.tauri.app/start/frontend/vite/
- [18] 基准数据 https://github.com/tauri-apps/benchmark_results ；bench 示例 https://github.com/tauri-apps/tauri/tree/dev/bench/tests/helloworld/src-tauri
- [19] GitHub runner 表 https://docs.github.com/en/actions/reference/runners/github-hosted-runners ；弃用 https://github.com/actions/runner-images/issues/14254 ；`ubuntu-latest` 改 26.04 https://github.com/actions/runner-images/issues/14748 ；Alpine arm64 https://github.com/actions/runner/issues/1637
- [20] Alpine 包 https://pkgs.alpinelinux.org/packages?name=webkit2gtk-4.1*&branch=v3.22&repo=community ；Ubuntu 包搜索 https://packages.ubuntu.com/search?keywords=libwebkit2gtk-4.1-0&searchon=names&suite=all&section=all ；Debian 包搜索 https://packages.debian.org/search?keywords=libwebkit2gtk-4.1-0&searchon=names&suite=all&section=all
- [21] tokio `Handle`：https://docs.rs/tokio/latest/tokio/runtime/struct.Handle.html
- [22] Implib.so：https://github.com/yugr/Implib.so/blob/master/README.md
- [23] ld.so(8)：https://man.archlinux.org/man/ld.so.8.en
- crate 源码浏览：`https://docs.rs/crate/<名>/<版本>/source/`

## 涉及的仓库文件（只读）

- `.github/workflows/release.yml`、`Cargo.toml`、`build.rs`、`src/main.rs`、`deny.toml`
- `tests/`（8 个文件使用 `CARGO_BIN_EXE_aproxy`）
- `desktop/package.json`、`desktop/vite.config.ts`、`desktop/dist/`（原型）
- `.agents/review/2026-10-11-0.2.0-audit/release-tests-docs.md`（RT-05）
- 用户本机 `~/.claude/skills/` 下的 tauri-custom-titlebar 与 tauri-windows-mica 两个 skill（不在仓库里）

crate 源码是下载到系统临时目录读的，不在仓库内；行号以文中标注的版本为准。
