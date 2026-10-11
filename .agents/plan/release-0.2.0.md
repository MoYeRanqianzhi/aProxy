# 0.2.0 计划

> 状态（2026-10-11）：方向已由用户定下（见记忆 [0.2.0-direction](../memory/2026-10-11-0.2.0-direction.md)）；
> 四路只读审查进行中，报告写到 `.agents/review/2026-10-11-0.2.0-audit/`（dataplane、control-plane、
> config-cli-install、release-tests-docs）；实现尚未开始。审查收齐后先汇总、排序，再更新本计划。

## 目标

0.2.0 是去兼容、改契约的最后一次廉价机会：发布后 IPC、磁盘布局、config.toml / settings.json、CLI、HTTP
错误体、信封都要长期稳定。所以先把这些契约重新设计到满意，再在上面做 i18n、skill 命令与 UI。

## 工作流（顺序暂定，审查结果回来后细化）

### W1 去除兼容
- 删 `src/compat_0_1_0.rs`（S1–S5）、`continue_install` 里的 relaying 豁免、`tests/compat_v0_1_0.rs`、CI 的
  upgrade-from-0-1-0 任务与 `tests/upgrade_from_0_1_0.rs`。
- 0.1.0 用户执行 `aproxy upgrade` 拿到 0.2.0 时，必须在改动任何东西之前失败，不能半途坏掉（实例失联、
  二进制已换）。审查 config-cli-install 会读 `git show v0.1.0:src/install/...` 给出结论与不靠兼容代码的办法。
- CHANGELOG 写清 0.1.0 → 0.2.0 的迁移步骤（停实例、重新安装）。

### W2 契约重审与定稿
- 依据审查报告逐项决定：IPC（UI 要的只读枚举、事件订阅、日志流、能力声明、错误码、写类 op 的访问控制）、
  `~/.aproxy` 磁盘布局、config.toml 与 settings.json 字段、CLI 子命令与参数、退出码与 `--json`、发给客户端的
  错误体形态、信封协议。
- 定稿后重写记忆 ipc-v1-upgrade-contract 的永久接口清单（0.2.0 发布时冻结）。

### W2b `install --force`（用户 2026-10-11 新增）
- 用途：正常更新失败时的兜底，「强制关掉进程，然后安装新版本，无交接过程」。这样以后无论兼容改成什么样，都能装上。
- 拟定流程（由**当前**二进制执行，只依赖它自己写下的记录，不需要新版本读懂任何东西）：下载并校验目标（或
  `--from <文件>`）→ 先停看护者（否则它会用旧二进制把实例拉回来）→ 结束进行中的安装者并删掉 install.state →
  停全部实例（先优雅、超时后强杀；身份按 pid + 进程创建时间核验，绝不按名称）→ 交换二进制 → 用新二进制
  `--version` 自检 → 尽力用新二进制的 `restore` 拉回实例；拉不回就列出原先在跑的实例及其启动方式，请用户手动启动。
- 会切断经 aproxy 转发的在途请求（包括用户自己的 agent 会话），所以要确认（非交互用 `--yes`），skill 里写明这一点。
- 普通更新失败时，提示里给出 `aproxy install --force` 作为退路。管辖检查照常（不覆盖包管理器管辖的安装）；降级仍需
  `--allow-downgrade`。`upgrade --force` 同义。
- 与 W2 的关系：强制路径读的是当前版本自己的 `.pid`、claim、install.state，这几种记录的「找到并结束这个进程」所需
  字段（pid、进程创建时间、角色）要列入永久接口，供以后任意版本的强制路径使用。

### W3 i18n
- 规则（用户定）：给用户看的随语言环境（en / zh-CN），给 agent 看的或不可见的固定英文。
- 语言判定：`APROXY_LANG` > settings.json `language` > 系统界面语言（Windows 用
  `GetUserPreferredUILanguages`，不用区域格式；unix 按 `LC_ALL` > `LC_MESSAGES` > `LANG`，`C`/`POSIX` 为英文）> 英文；
  `zh*` 为中文，其余英文。windows-sys 0.52 已有这些 API（加 `Win32_Globalization` feature）。
- 消息机制待定：内联 `tr!(en:, zh:)`（编译期检查参数与漏译，零依赖）与「每种语言一个目录文件 + 构建期生成
  带类型参数的函数」之间选。UI 前端也要目录文件，倾向两边共用一套目录；W3 开工时定。
- `--help`：clap 派生属性接受运行时表达式（`#[arg(help = ...)]`）；clap 内置标题用 `next_help_heading`、
  `subcommand_help_heading`、自定义 help 参数覆盖；clap 自己的解析错误写死英文，第一期保留英文。
- 经 IPC / HTTP 传出、再被显示的错误改为错误码 + 参数，由显示的一端本地化。
- 测试统一 `APROXY_LANG=en`，断言改英文。
- 先建基础设施，再随各模块重构迁移该模块的字符串，避免翻译要删的代码。

### W4 `aproxy skill` 命令（用户需求）
- `skill install` / `skill upgrade`：只下载 skill；删掉 `install --skills-only`。默认取与当前二进制同版本的
  skill（skill 描述的是这个二进制的行为）；`aproxy install` 仍随二进制更新 skill。
- `skill list`：已装 skill、版本、与二进制版本是否一致、位置；没装就提示怎么装。
- `skill show <name> [<file>]`：默认输出 SKILL.md，也能输出 references 等文件，给读不了目录的 agent 用。
- 安全：`show` 只能读 skill 清单里登记的文件。拟在发布的 skills 包里带清单（版本、文件列表、哈希）作白名单；
  名字与文件路径必须精确命中清单；拒绝绝对路径、`..`、盘符 / UNC / ADS；逐级 `symlink_metadata` 拒绝符号链接
  与 junction，规范化后再核对前缀；只读普通文件、有大小上限、UTF-8。
- 不把 skill 嵌进二进制；本地没有就提示 `aproxy skill install`，非零退出。
- 所有 `--help` 末尾加一行固定英文提示（给 agent 看），指向 `aproxy skill list` / `show`。
- SKILL.md 里写明打不开 references 时用 `aproxy skill show`。
- 待审查补充：release.yml 组 skills 包的方式、清单生成、版本对应。

### W5 UI（Tauri 2，编进本体）
- 构建：cargo feature `ui` 进 default features；`--no-default-features` 出无 UI 版本，不得多出任何依赖。
- 进程模型：`aproxy desktop`（用户定的启动方式）起独立的 GUI 进程（窗口 + 托盘），守护进程永不加载 GUI；无 UI 版本也认得 `desktop`（帮助里隐藏），执行时说明这个版本没有桌面界面、该装哪个版本，非零退出——不让用户看到 clap 的「unrecognized subcommand」；GUI 只经 IPC 与实例通信、
  按显式 home 寻址，不开新网络端口。
- 窗口外观按两个 skill 的做法（实现时用 Skill 工具调用它们，照 recipe 复制 assets）：
  - tauri-custom-titlebar：Windows 保留原生框架与标题栏按钮（Snap Layouts、系统菜单），`windows_titlebar.rs`
    原样使用；标题栏高度取 `--titlebar-height`；可交互元素 `app-region: no-drag`；按钮颜色只用
    `setTitlebarColor`。Linux 用原生装饰或 `decorations: false` + `enableDragRegion()`。
  - tauri-windows-mica：**默认关闭，在 UI 设置里开启，可运行时切换**。窗口始终 `transparent: true`、
    `visible: false` 并由页面调 `show()`，配置里不写 `windowEffects` 与 `backgroundColor`；页面用 mica.css 的
    令牌。关闭时 `data-mica="off"`（不透明回退）；开启时 `setEffects(mica)` + `data-mica="on"` +
    `setTitlebarColor(null)`；关闭时先设 `data-mica="off"` 再 `clearEffects()`（反过来窗口会透明），并把标题栏
    颜色设回页面色。主题用 `setTheme` / `theme`，不用 `micaDark` / `micaLight`。权限：`core:window:allow-show`、
    `allow-set-effects`、`allow-set-theme`。
  - **没有 Mica 的 Windows 11**：是否支持由 Rust 端判定，不只看 UA-CH——按系统 build（22523+ 走文档化的
    `DWMWA_SYSTEMBACKDROP_TYPE`；22000–22522 只有未文档化属性、skill 未验证，倾向按不支持处理）并直接调用
    window-vibrancy 拿到结果（Tauri 会吞掉错误）。不支持时设置里的开关禁用并说明原因；系统关了透明效果、
    节电模式或高对比度时 Mica 显示为纯灰，这是系统行为，设置里给出提示（`EnableTransparency` 注册表值）。
  - 验证脚本：titlebar 的会实际移动鼠标约 15 秒（运行前先告诉用户），mica 的会把窗口置前约 8 秒；人工检查项
    （切换系统主题、透明效果）交给用户。skill 验证于 tauri 2.12.0 / wry 0.57.0 / tao 0.37.1，升级 Tauri 后重跑。
- UI 文本走 i18n（与命令行同一套语言判定）。
- **原型先行（用户 2026-10-11）**：先在 `desktop/` 做浏览器里可看的高保真原型（假数据，不接后端），用户只审风格与
  美观度，要求见记忆 [ui-design-style](../memory/2026-10-11-ui-design-style.md)；审核通过后才真实接入，并解决
  海量日志的性能（虚拟滚动等）。日志行高统一、详情在侧面板打开，为虚拟滚动留路。
- **可自定义界面（用户 2026-10-11 同意）**：用户自己改界面拦不住也不该拦（WebView2 远程调试、替换前端资源都能做到），
  所以完整注入作为正式功能提供；要守住的是「别人做的扩展经官方渠道装进来时默认拿多少权限」。分三层：
  - 主题：可复制分享的声明式字符串 `aproxy-theme-v1:{...}`（主色、文字、表面、对比度、圆角、字体、动效强度、状态语义色、
    浅/深变体），设置里导入/导出，不含脚本；界面的颜色、圆角、字体、阴影、动效全部走 CSS 变量，根元素暴露稳定的
    状态属性（data-theme、data-variant、data-mood 等），主题与扩展不依赖内部 DOM。
  - 吉祥物：组件契约——输入 MascotState（mood、重试次数、尺寸、减少动效、配色），每个吉祥物实现同一接口；内置几种
    放注册表，设置可选或不显示；默认是不倒翁，也作软件图标候选（exe 图标编译时固定，托盘图标可随所选吉祥物变）。
  - **安全边界在实例与 IPC，不在 UI**（用户 2026-10-11 纠正：UI 只是 UI，自己不持有 key、不经手流量，敏感内容都在
    实例里）：IPC 永不返回明文密钥——key 在实例侧打码，配置里的密钥字段只写不读；请求体只在点开某条日志时按需取；
    UI 的 webview 用严格 CSP 禁止一切外部网络（数据只走 IPC），注入的代码看得到也发不出去。
  - 扩展包（本地文件夹 + 清单）：读不区分权限（扩展能看到的就是 UI 能看到的）；只管「写」——停止/重启实例、改配置
    （改 base_url 等于把流量导去别处）、执行安装——扩展第一次调用这类操作时请用户同意一次。吉祥物仍在隔离 iframe 里
    运行，目的是稳定（坏掉的吉祥物拖不垮界面）。另给稳定的事件与状态接口，减少扩展依赖内部结构。
  - 用法做成 skill（给 agent 用来帮用户做主题、吉祥物、扩展）。参考：Codex 桌面版的 `codex-theme-v1:` 主题字符串与
    社区经本机 CDP 注入 CSS 的工具（其中有的只收数据、CSS 与图片，直接拒绝主题脚本）；用户说明只是受其启发，具体
    自行设计。
- 前端栈：Vite + Svelte 5 + TypeScript（产物嵌进二进制，Svelte 编译后运行时小、自带过渡与弹簧动效；不用大型组件库
  与图表库，字体本地打包）。这是主代理的选择，用户可改。
- 待定：
  - **所有平台都发两个版本（用户定）**：现有 11 个构建（Windows x64 / x64-v3 / x86 / arm64，Linux gnu x64 /
    x64-v3 / arm64，musl x64 / arm64，macOS arm64 / x64）各出带 UI 与无 UI 两份，共 22 个。由此带来的事：
    - Linux 的 UI 版动态链接 webkit2gtk / GTK，缺库的机器上连代理都起不来；install 与一键脚本要选对版本，
      升级时保持原版本（二进制要能报告自己是哪个版本，例如 `--version` 带上变体）。
    - gnu 的 UI 版不能沿用 cargo-zigbuild 压低 glibc 下限，glibc 下限由 webkit2gtk 决定，拟在 ubuntu-22.04
      （及其 arm runner）上构建。
    - musl 的 UI 版不可能静态链接：要在 Alpine 容器里对 musl 版 webkit2gtk 动态链接，只能跑在装了 webkit2gtk
      的 musl 发行版上。发布说明要写清。
    - **命名（用户 2026-10-11 选定 headless）**：带 UI 的沿用现有名字——GitHub 资产 `aproxy-<target>[-v3][.exe]`、
      npm `@meowo/aproxy` 与 9 个 `@meowo/aproxy-<平台>`、crates.io `aproxy`（feature `desktop` 默认开，binstall
      的默认命名正好对应它）；无 UI 的叫 headless——资产 `aproxy-headless-<target>[-v3][.exe]`、npm
      `@meowo/aproxy-headless` 与 9 个 `@meowo/aproxy-headless-<平台>`、cargo 用 `--no-default-features`，不另建 crate。
      `npm/build-and-publish.sh` 已有按组切换包名与资产前缀的结构（format 组），headless 加一组即可；JS 转发器按主包
      名推导子包前缀，不用改。两个主包都提供 `aproxy` 命令，不能同时全局安装。
    - **npm 种子已发（2026-10-11）**：10 个 headless 包以 0.0.0 占位版本发布；**可信发布绑定待所有者完成**
      （GAT 做不了，见记忆 trusted-publishing-p0 的 2026-10-11 一节）。crates.io 不需要新配置，已核对现有绑定都在。
    - 0.1.0 安装器的干净失败另有一条与命名无关的路：0.1.0 的 `probe_version` 取 `--version` 输出里第一个以数字
      开头的词，且必须与目标版本完全相等，否则在交换前中止（`git show v0.1.0:src/install/staging.rs`）。0.2.0 的
      `--version` 本来就要带上变体（install 升级时保持变体），若格式写成 `aproxy v0.2.0 (desktop)` 之类，0.1.0
      的四条下载渠道都会在交换前失败。待审查 config-cli-install 的断点分析印证后定。
    - 发布流水线时长与产物体积显著增加。
  - aproxy.exe 是控制台程序，从快捷方式启动 `aproxy desktop` 会闪控制台窗口：拟让 `desktop` 分离出自身后父进程退出，
    或在独占控制台时释放它，实现时实测。
  - crates.io：默认 feature 含 UI，`cargo install aproxy` 需要包里带构建好的前端，Linux 还要 GTK 开发库；
    无 UI 用 `--no-default-features`。
  - 前端技术栈（体积会编进二进制）、UI 偏好（Mica 开关、主题）存哪里（倾向 settings.json 的 `ui` 段，随 W2 定）、
    发版矩阵与体积、macOS（不在 0.2.0 范围）。

### W6 审查发现的修复与重构
- 待审查报告。

### W7 发布
- 版本 0.2.0（是否先发预发布版待定）、CHANGELOG、README / docs / skill 同步、skill references 是否留存
  0.1.0 份（按 aproxy-cli-skill-versioning 记忆判断）、CI 与发版矩阵。

## 下一步

1. 收齐四份审查报告，汇总成一张发现清单（P0 / P1 / P2），与本计划的工作流对齐、排序。
2. 先做 W1（独立、确定），同时出 W2 的契约设计稿。
3. W2 定稿后再动 W3 基础设施、W4、W5。
