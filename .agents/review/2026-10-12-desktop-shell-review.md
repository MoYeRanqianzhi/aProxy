# 桌面外壳审查（alpha.1 第 3 步，推送前）

> 审查对象：提交 53afebd..a7cf72f，即 alpha.1 第 3 步 Tauri 2 桌面外壳的 10 个提交；推送前做的只读对抗式审查。
> 日期：2026-10-12

第 3 步桌面外壳（53afebd..a7cf72f，10 个提交）推送前审查：没有必须修的问题，应该修 4 条，其余是可以不修与相邻事项。

## 范围与方法
- 审的是已提交版本。c8da9f2 之后的两个提交只改了前端与注释，Rust 源与 a7cf72f 相同，行号都以 a7cf72f 为准。工作区里 aproxy-desktop/Cargo.toml 未提交的 `/gen/schemas/**` 不在范围内，没有审。
- 全程只读：没编译、没跑测试、没启动 aproxy。用到的手段：git；grep；cargo tree、metadata、`package --list`、`fmt --check`；在自建临时目录里跑 `cargo generate-lockfile`（只解析不编译，临时目录已删）；用 llvm-readobj 读已有构建产物（没有运行）；读注册表缓存里依赖的源码；rustsec.org。
- 契约文件：`git diff --stat 53afebd..a7cf72f -- src/ledger.rs src/install/force.rs tests/force_contract.rs` 为空，C1–C8 没有改动。【命令验证】

## 一、必须修
无。

## 二、应该修

1. toml 下限：Cargo.toml:77-81；README.md:57、README_EN.md:117（同类还有 CHANGELOG.md:27、compatibility.md:111）
- 问题：不带 `--locked` 的 `cargo install aproxy` 会解析出 toml 0.8.2、toml_datetime 0.6.3、proc-macro-crate 2.0.2，这个组合 CI 从没测过。Cargo.toml 注释里的手动 `cargo update --precise` 只保护仓库自己的 Cargo.lock，管不到用户的无锁安装。
- 证据【命令验证】：把三个成员的清单（不含 aproxy-format）配上空源文件放进临时目录，跑 `cargo generate-lockfile`，aproxy 得到 toml 0.8.2。把要求改成 `toml = "0.8.23"` 再解析，得到 toml 0.8.23、proc-macro-crate 2.0.0，解析成功。索引缓存显示：proc-macro-crate 2.0.1/2.0.2 写死了 `toml_datetime = "=0.6.3"` 和 `toml_edit = "=0.20.2"`，2.0.0 只要求 `^0.20.2`；glib-macros 0.18.5 要求 `proc-macro-crate "2.0"`。
- 波及范围【推断】：锁文件级的解析按全部特性、全部目标进行，所以 Windows 和 `--no-default-features` 的 headless 安装一样会拿到 0.8.2。
- API 对照【读代码】：aproxy 用到的只有三样，toml-0.8.2 源码里都有（lib.rs:168、171；de.rs:41、84；ser.rs:66、122）：
  - `toml::from_str`：src/config.rs:824、920，src/commands/config.rs:53、206，src/doctor.rs:349，测试在 src/config.rs:2050
  - `toml::to_string_pretty`：src/config.rs:945
  - 两个 Error 的 Display
- 结论：应当能编译（未经编译验证）。实际差别会落在报错文字、to_string_pretty 的排版，以及两年间的解析修复上。Linux 上的 system-deps 6.2.2 也会跟着用上 0.8.2，它用到的 API 0.8.2 应该都有（推断）。
- 建议：
  - 写 `toml = "0.8.23"`，把实测过的版本定为下限。这样解析器不会再退回 0.8.2，`--precise` 那一步也可以删掉，注释相应改写。
  - 文档里的 `cargo install aproxy` 都加上 `--locked`，包括 CHANGELOG.md:27 和 compatibility.md:111 的 `--force` 写法。另一个理由：不加锁时 tauri 可能浮到更新的 2.x，而 titlebar.rs 依赖 wry 的私有窗口类。

2. 导航白名单比注释说的宽：aproxy-desktop/src/lib.rs:340-346（在 271 行使用）
- 问题：`is_app_url` 只比 host，不比端口，而且 http 和 https 在所有平台都放行。于是 `http://tauri.localhost:8080/`、`https://tauri.localhost/` 都能过。Linux/macOS 上嵌入页面的地址是 `tauri://localhost`，那里任何 `http(s)://tauri.localhost[:端口]` 都是真实的回环网络地址。
- 为什么是问题：CSP 不管顶层导航，这个函数就是「窗口只停在嵌入页面、webview 不联网」的唯一闸门。要利用它，主页面里得先跑起不受信的脚本。现在没有这样的脚本：主题只是数据，而且校验严格；吉祥物在不透明源的沙箱里。所以没列为必须修。W5 的扩展上线后，这里就成了实打实的边界。
- 建议：按平台精确比对源。Windows 只认 `http` + `tauri.localhost` + `port().is_none()`；其余平台只认 `tauri://localhost`。【读代码】

3. 释放控制台之后的标准句柄：src/commands/desktop.rs:184-188（调用处 58-60）
- 问题：FreeConsole 之后，进程里记着的 stdin/stdout/stderr 还是旧句柄值。分两种情况：
  - 如果这些句柄已经关闭，数值可能被之后打开的句柄复用，比如台账的临时文件、WebView2 的管道。init_force_logging 把 tracing 写到 stdout，Tauri 的 log 也经它出去，这些文字就会写进不相干的对象。
  - 如果句柄还开着、只是断开了，写入返回的不一定是 ERROR_INVALID_HANDLE。那样 std「无效句柄视为写入成功」的处理就不成立，第三方代码里的 `eprintln!` 会 panic。
- 184-185 的注释把「静默成功」当成事实，可它取决于 FreeConsole 怎样处理句柄。【推断，未验证】
- 建议：FreeConsole 成功后，把三个标准句柄都 `SetStdHandle(..., null)`。std 遇到空的标准句柄会按 ERROR_INVALID_HANDLE 当成功处理，「静默」就由构造保证。windows-sys 0.52 的 Win32_System_Console 特性已经开着。

4. 登记之后的失败路径没走 ledger::exit：src/commands/desktop.rs:63-66、124-127
- 问题：`fail` 用的是 `std::process::exit(1)`。110 行已经登记了台账，之后运行时建不起来时走的就是它，条目会留下。这违反了 ledger.rs:296-298 的写者约定：「登记过的进程一律经这里退出」。
- 影响很小：is_alive 会按启动标记认出死条目，强制安装也会清掉。
- 建议：`fail` 改成调用 `ledger::exit(1)`。没登记时 `remove_own` 什么都不做，前面的调用点不受影响。【读代码】

## 三、可以不修
- a. 没有控制台（Absent）时不弹对话框（desktop.rs:58-62）。父进程用 DETACHED_PROCESS 启动时，错误全打到无效的 stderr，用户什么也看不到。可以让 Absent 也走对话框。【读代码】
- b. 事件循环里的 panic 没人接（lib.rs:196）。Tauri 2.12.3 把 setup 放在 run_return 的 Ready 事件里（app.rs:1442-1445），build() 外面那层 catch_unwind 管不到。从快捷方式启动时控制台已经释放，panic 信息无处显示；台账条目会随栈展开删掉，退出码 101。可以在 desktop 路径装一个 panic 钩子，dialogs 为真时弹框。【读代码】
- c. 单实例检查（70-86）和登记（110）之间不是原子的，快速连开两次可能出现两个窗口。alpha 阶段可以接受。【读代码】
- d. 单实例检查读到已死的 desktop 条目只跳过、不删，要等强制安装来清。可以顺手调用 `ledger::remove_if_unchanged`。【读代码】
- e. MessageBoxW 没有属主窗口，也没带 MB_SETFOREGROUND（lib.rs:217-223）。从终端拉起的子进程没有前台权限，对话框和窗口可能落在终端后面。【推断，未实测】
- f. headless 构建里 `aproxy desktop --help` 仍然说「打开桌面界面…」（cli.rs:184-189）。可以用 `cfg_attr` 给 headless 换一段 about。【读代码】
- g. headless 说明（desktop.rs:26-28）在 Linux 上没提 WebKitGTK。照着做也安全：强制安装会先跑新二进制的 `--version`，缺库时以 10 结束、什么都不改。只是让用户多绕一圈。【读代码】
- h. 产物里没有 proto/ 的代码：没有 `__aproxyProto`，也没有 proto/parts 分块，只剩 App 模板里 `proto.MascotLab` 一类属性名。但 i18n 表里 `proto.*` 的中英文案仍然打了包；`mascots` 页还在路由表里（state.svelte.ts:13、24），生产环境里是空白页。【命令验证：grep 的是本机 15:06 构建的 dist，不是 CI 产物】
- i. 文档措辞两处：
  - README「桌面界面（预览）」一节和 CHANGELOG.md:37 都说「不启停、不改动任何实例」。但 desktop.rs:112-120 在有残留安装状态时会静默拉起 `install --continue`，续作可能滚动重启实例。这是和其他命令一样的兜底，措辞可以留个余地。
  - docs/architecture.md:543 说「不带控制台的子进程」。实际上子进程带着一个看不见的控制台，随即释放；desktop.rs:47-49 的注释写得是对的。【读代码】
- j. 两处理由写得不准：
  - deny.toml:25 的理由只提了 glib-macros；Cargo.lock 里 gtk3-macros 0.18.2 也依赖 proc-macro-error。【命令验证】
  - aproxy-desktop/Cargo.toml:86-87 拿 macOS 上的符号重复，解释一个只在 Windows 上的依赖为什么钉版本。真正的理由是 lib.rs:393-395 说的「与 Tauri 用同一个版本」。【读代码】
- k. skill 的 commands.md desktop 一节可以给 agent 补两句：
  - Linux/macOS 上命令会一直占着前台，直到窗口关掉；替用户打开时放到后台。
  - Windows 从终端运行时退出码 0 只说明窗口进程拉起来了，之后的失败只出现在对话框里。
- l. eb3f730：Titlebar.svelte:49 说「非 Windows 上按 32px 算」，但 max 会把它抬到 44px。Linux/macOS 原生标题栏下面那一行工具栏因此从 32px 变成 44px，是有意的吗？
- m. CI 里 desktop 变体的 `npm ci` 会装上只给 `npm run icons` 用的 @resvg/resvg-js 和它的原生可选包（devDependency）。package-lock 核对过，没有安装脚本。
- n. CSP 管不到 WebRTC、DNS 预取这类出口，留到 W5 设计扩展时再考虑；现在主页面里没有不受信脚本。【推断】

## 四、逐条结论

1. headless 不含 tauri：通过。【命令验证】
- `cargo tree -p aproxy --no-default-features -e normal --target all -i tauri --locked --offline` 报 "package ID specification `tauri` did not match any packages"。
- 用 `-e all --target all` 列出全部依赖，再 grep aproxy-desktop、tauri*、wry、tao、webview2-com、webkit2gtk、gtk、glib、muda、window-vibrancy，结果为空。
- 新 Cargo.lock 是 53afebd 那份的严格超集，没有任何包换版本或消失。headless 依赖图（全部边、全部目标，208 个包）里的每个包、每个版本都在旧锁里：headless 既没多出依赖，也没变版本。

2. main.rs 改手建运行时：其余命令的行为不变。【读代码 + 宏源码核对】
- tokio-macros 2.7.2 对 `#[tokio::main]` 的展开（entry.rs:507-519）就是 `Builder::new_multi_thread().enable_all().build().expect(..).block_on(body)`，和 main.rs:62-66 一致。线程数、IO/时间/信号驱动、Ctrl-C、退出码、panic（展开，101）都相同；代码页仍在解析参数之前设置。
- 唯一差别：运行时现在建在 `--version`、`--help` 和用法错误之后，C5「先于一切副作用」反而更严格了。
- desktop 的分派排在 `--daemon-child` / `--daemon-watchdog` 之前，但这两类进程从不会带着 desktop 参数被拉起，没有实际影响。

3. desktop.rs / aproxy-desktop：【读代码】
- 单实例：靠台账加 is_alive 按启动标记核验，正确。
- 退出码：已在运行以 0 退出，打不开以 1 退出，headless 以 1 退出，都对。
- 台账：正常关闭和 run 返回错误都经 ledger::exit 删除条目；panic 时栈展开，守卫删除；被杀或崩溃会留下死条目，但不挡下次启动。
- 控制台：从终端（Shared）启动时，拉起一个 CREATE_NO_WINDOW 的分离子进程，子进程看到的是 Exclusive，释放控制台、改用对话框；从快捷方式（Exclusive）启动时立即释放、改用对话框。逻辑成立。
- headless：命令在帮助里隐藏但能解析，说明后以 1 退出，musl 不给换装命令；有单元测试守着。
- 需要改的见应该修 3、4，以及可以不修 a–g。

4. Tauri 配置与安全：基本已经最小。【读代码 + 依赖源码核对】
- CSP：default-src 'none'；脚本、样式、字体只许 'self'，图片另许 data:；connect 只有 IPC；frame 只许 'self'；object、base-uri、form-action 都是 'none'。
- 没有远程 URL，没有 devUrl，withGlobalTauri 为 false，没开 asset 协议，也没关 Tauri 对 CSP 的改写。
- 只有一个 capability（main 窗口，只对本地源）。6 条权限正好对应前端实际的调用：show、setTheme、setEffects、clearEffects，加三个自定义命令；自定义命令经 app manifest 纳入了 ACL。
- window.open 默认被拒：wry 0.57.0 的 webview2/mod.rs:848-849 在没有处理器时直接 SetHandled(true)；tauri-runtime-wry 2.12.1 只有在提供了处理器时才挂上去。
- 数据目录：显式指定后 Tauri 不再强制用 app_local_data_dir（tauri 2.12.3 manager/webview.rs:559-564）；WebKitGTK 的数据和缓存都落在给定目录（wry web_context.rs:35-39）。
- 吉祥物沙箱页单独加 `script-src blob:` 的做法成立。
- 缺口见应该修 2。

5. build.rs 与打包：
- 缺 dist 时用 `cargo::error` 给出构建步骤，不拿占位页面凑数（build.rs:17-26）。【读代码】
- `cargo package --list -p aproxy-desktop` 包含 build.rs、capabilities、dist/**、icons、permissions/autogenerated、src、tauri.conf.json、Cargo.lock，不含 ui/ 和 gen/。【命令验证，用的是本机的 dist】
- gen/ 被 .gitignore 忽略；发布时的校验构建会因为 build.rs 写 gen/ 而失败，Cargo.toml 和 architecture.md 都写明留给第 4 步处理。
- 附带两点：
  - root 包的 include 在本机会把 `.claude/skills/aproxy-format/scripts/__pycache__/*.pyc` 带进包。这是早就存在的问题，不在本步范围；CI 的干净检出里没有这个文件。
  - aproxy-desktop 首发要由所有者在本地发布，而 dist 不进 git、包里的 dist 也没有来源记录。首发前应从对应提交重新 `npm ci && npm run build`，再用 `--list` 核对一遍。

6. CI：
- 两种变体在 windows 和 ubuntu 上都跑 `clippy -D warnings` 和 test，也都做 `--version` 冒烟。
- fmt 只在 windows desktop 跑一次，这没问题，格式与特性无关；本机 `cargo fmt --all -- --check` 通过。
- headless 排除了 aproxy-desktop，不装 Node 和 WebKitGTK；依赖树断言放在 ubuntu headless 上，覆盖全部目标平台。
- rust-cache 按变体分了 key。upgrade-from-0-1-0 改用 headless 是合理的。macos 测试任务补上了前端构建，不补的话这个任务会坏。macos-build 只要求两种变体都编得过并做冒烟，也合理。
- 冒烟脚本的正则与 VERSION_LINE（src/lib.rs:112-121，cpu 段为空或 "; x86-64-v3"）一致。
- 本机已有的 desktop 调试产物 target-desktop/debug/aproxy.exe（15:09 构建，Rust 源与 a7cf72f 相同）从 comctl32 只导入 SetWindowSubclass、RemoveWindowSubclass、DefSubclassProc，没有 TaskDialogIndirect，并带有 MANIFEST、VERSIONINFO、ICON 资源。System32 里 5.82 版的 comctl32 按名字导出这三个函数。所以 24df89d 的说法属实。【命令验证，没有运行该文件】
- 提醒：job 改名成了 "windows (desktop)" 等。如果分支保护按名字要求必需检查，需要同步更新（没查）。

7. toml 下限：见应该修 1。

8. deny.toml：两条豁免的理由属实；按编号豁免已经是 cargo-deny 能给的最小粒度。
- RUSTSEC-2024-0429 仍然有效（rustsec.org）。本机注册表缓存里的全部 crate 源码中，只有 glib-0.18.5 自己出现 VariantStrIter / array_iter_str；搜索范围包括 tauri 2.12.3、wry 0.57.0、tao 0.37.1、muda 0.20.0、webkit2gtk 2.0.2、gtk 0.18.2。glib 0.18.x 最新就是 0.18.5，没有带修复的版本。
- RUSTSEC-2024-0370 是不再维护的过程宏依赖，只在编译期运行。
- gtk-rs GTK3 一族当年那批「不再维护」通告已经撤回：rustsec.org 上 gtk 的 RUSTSEC-2024-0415、gtk3-macros 的 RUSTSEC-2024-0419 都标为 withdrawn，所以不需要再豁免。
- 许可证：cargo metadata 列出 493 个包，按白名单逐个求 SPDX 表达式，全部能满足，也没有缺 license 字段的。用的是我自己写的简化求值器，不是 cargo-deny。

9. 文档与 skill：
- README、README_EN、CHANGELOG、architecture.md 对新行为的描述与代码一致，措辞问题见可以不修 i。
- skill 符合 AGENTS.md 的要求：英文，写给操作工具的 agent，并讲清理由。例如：窗口开在用户屏幕上，所以只在用户要求时运行；不要从窗口读实例状态；换装前先问用户。SKILL.md 只放一行，细节放在 commands.md，合乎渐进披露。
- 补充建议见可以不修 k。

eb3f730 三处：
- 主题解析：parseTheme 只把认得的键拷进新对象，未知键直接丢弃，所以提交说明里「旧版本照样忽略 titlebarMin」属实。titlebarMin 必须是有限的 number、在 36–64 之间，取整后以数字拼进 `--titlebar-min: Npx`，没有注入的可能。颜色只认 #rgb、#rrggbb 和 oklch(L C H)，并且会规范化重组。name、author 限 40 字，以文本渲染。JSON 里的 `__proto__` 只会成为对象自身的属性，不会污染原型。【读代码 theme.ts:290-347、388-392】
- max() / min()：主题下限是 36，栏高最矮 36。【读代码，未实际渲染】
  - 各控件最矮时的高度：搜索框 min(32, bar−10) ≥ 26，kbd min(20, bar−22) ≥ 14（kbd 自带 line-height:1），标签 min(26, bar−14) ≥ 22，都不超过栏高。
  - 原生按钮比下限高时，栏跟着变高，子元素仍被各自的上限封顶。
  - 最大化时，`--titlebar-top` 加在 padding 上（content-box），可见的栏高仍然不低于下限。
  - `--titlebar-min` 是 CSS 像素，与 DPI 无关。
  - 最小化时 titlebar_insets 可能算出 0 或负数，有 max() 兜底，反而比原来更稳。
- 原生按钮区：
  - 右侧 padding 让出整个栏高；按钮下面那一截露出页面底色，而且可以拖动。
  - Mica 关闭时，按钮后面涂的是 `<html>` 的底色（--paper），标题栏底色 --chrome-bg 也是 --paper，两者没有色差。
  - 这与 tauri-custom-titlebar skill 的「最小高度变体」一致（frontend.md:89）。skill 实测的是 40px，这里默认 44、主题最高可到 64，超出了实测范围，但机制相同。
- a7cf72f：只改了注释，与脚本的默认值（tumbler + sticker，icons.mjs:36-37）一致。

## 五、相邻事项（不在这 10 个提交里，建议记一笔）
- 普通安装不区分 desktop/headless：src/install/download/crates.rs:144 的 cargo 通道总按默认特性编译，github 通道的资产名也只区分 CPU 档。本步之后 desktop 特性不再是空的：headless 用户经 cargo 通道升级会被换成带界面版，在 Linux 服务器上会因为缺 WebKitGTK 开发包而编译失败。计划里第 4 步的变体探测会覆盖这一点，但 TODO 第 4 步那一条没写出来。
- 推送 main 不会触发 release，但在第 4 步完成之前别打 v* 或 format-v* 的 tag（TODO 已经记了）。

## 六、我没能验证的
- 一切编译或运行的结果：Linux 带界面版能否链接、能否启动；ubuntu 和 macos 的 CI 能否通过；toml 0.8.2 下能否编译、行为差多少；release（LTO）构建是否同样带上清单和资源。
- FreeConsole 之后标准句柄的确切语义，也就是应该修 3 的风险是否真实存在。
- 沙箱 iframe 是不透明源，它的 CSP 'self' 能否匹配 mascot-sandbox.js。Chromium 按文档 URL 计算，WebKitGTK 未知，所以 Linux 上自定义吉祥物可能加载不了。另外，沙箱 iframe 能否调到 IPC 也没验证；即使能，也只有 6 条无害的权限。
- 对话框和窗口从终端拉起时能否到前台；64px 栏高的实际观感；Linux 上从 .desktop 启动时，错误是否有地方可见。
- cargo-deny 的完整结果：我只核对了 gtk 和 gtk3-macros 的通告已撤回、glib 的通告仍有效，许可证用的是简化求值器。以推送后 Review 工作流的结果为准。
- WebView2 引擎自身的网络活动（更新、遥测），以及分支保护是否按名字要求 CI 检查。

## 主代理的处理决定（2026-10-12）
- 应该修 1–4 全部修。
- 可以不修中，a、b、d、e、f、g、h、i、j、k 修；l 是有意为之，只改注释；c 在 alpha 阶段接受；m 不改；n 留给 W5。
- 相邻事项「普通安装不区分变体」并入第 4 步。
- 以上由 step3-desktop 实施。
