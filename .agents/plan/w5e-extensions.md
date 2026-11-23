# Desktop extension packages, host protocol and authorization

## 2026-11-23 用户改了方向：不做沙箱，安装前把风险讲清楚（取代下文的 §3–§6 与 §8）

用户裁定（2026-11-23）：扩展包与主题包这类风险属于社会工程学，技术手段挡不住；不做沙箱、不做逐项写授权，
把力气花在**安装前的提醒**上。下文 §3–§6（iframe 沙箱、外壳文档、租约、逐项授权与审计）与 §8（真实窗口的隔离验证）
作废，脚本扩展的编译开关不再等隔离验证。分支 `codex/t10-extensions` 上 Revision 3 的 12 个修复（`ffe02952..9ea0f241`，
Gate F 已过）加深的正是这些，不合入，留作参考。master 上已有的 t10 第一轮代码按下面的设计简化。

**包。** 主题包与扩展包用同一种包（一个文件，或解开的目录），`manifest.json` 里 `kind` 是 `theme` 或 `extension`：
- 公共字段：`id`、`name`、`version`、`author`、`description`，以及 `repository`（开源时填 GitHub 仓库地址；不填即闭源）。
- `theme`：主题字段（现在的 ThemeSpec：浅色与深色的颜色、圆角、字体、动效）＋可选的 `theme.css`（任意样式）＋可选的图片
  （背景图、静态吉祥物图）。没有脚本。
- `extension`：一个 ES module 入口＋可选的资源。直接在主窗口里运行，不进 iframe。

**安装入口。** 设置页「安装…」选文件，或把文件拖进窗口。Rust 把包装进 `<home>/desktop/extensions/<id>/`，可以列出、
启用、停用、卸载。只保留把文件写对所需的检查（不写出目标目录、大小有上限），这不是沙箱。

**安装前的检查与弹窗。** 每条都是弹窗，要点「确认」才进入下一个；任何一个点了取消就不装。
1. 总是有：信任提醒。写明名称、作者、版本、种类、包里有什么，以及扩展在窗口里运行，能做用户在窗口里能做的一切
   （读请求内容、改配置、启停实例）；只安装来自可信来源的包。
2. 没有 `repository`：额外提示闭源的风险。
3. 有 `repository`（GitHub 地址）时，Rust 经 GitHub API 查（网络走更新下载用的同一套代理设置）：
   - 仓库信息：star、fork、创建时间、最近推送、是否已归档、许可证。这些数字写在弹窗里给用户判断。
   - 哈希验证：取 tag `v{version}`（没有就取 `{version}`）的 release，找与安装文件同名的 asset，比较 GitHub 给的
     `digest`（sha256）与本地文件的 sha256。不一致，或找不到这个 release 或 asset，都算没通过，**额外弹窗**。
   - star 低于阈值（先定 50）：**额外弹窗**。
   - GitHub API 连不上、被限流、仓库不存在，或地址不是 GitHub：上面几项都没查成，**额外弹窗**说明没能校验。

**包的标准（2026-11-23 定）。** 主题与扩展用同一种包：一个 zip 文件（根目录有 `manifest.json`），或解开的目录。
- `manifest.json`：`schema`（1）、`kind`（`theme` | `extension`）、`id`（小写字母、数字、`.`、`-`，3–100 个字符；建议
  `io.github.<owner>.<name>` 这样的反向域名；也是安装目录名）、`name`、`version`（semver；GitHub release 的 tag 是
  `v<version>` 或 `<version>`）、`author`、`description`（`name` 与 `description` 可以是字符串，或按语言的对象）、
  可选 `repository`（`https://github.com/<owner>/<repo>`，不填即闭源）、可选 `license`（SPDX 标识）。
- 主题：根目录 `theme.json`（ThemeSpec）、可选 `theme.css`、可选 `assets/`（图片 png、jpg、webp、gif、avif、svg 与字体
  woff2、woff、ttf、otf），可选 `README*`、`LICENSE*`。别的文件一律不收：主题是数据，不带代码。
- 扩展：根目录 `main.mjs`，一个自包含的 ES module，导出 `activate(api)`、可选 `deactivate()`。它用 `blob:` 地址加载，
  相对 import 解析不了，所以要打成一个文件；建议不压缩，方便审查。可选 `assets/`（经 `api.assets` 取用）、`README*`、
  `LICENSE*`。manifest 可选 `formats`（配对的 format 程序名，即 hello 回复里的 `name`）。
- 公开说明（建议，待用户确认）：扩展的 manifest 可选 `network`（会连接的主机名）与 `writes`（会改的东西：`config`、
  `instances`、`updates`、`formats` 的子集）。只是说明、不做限制：安装弹窗里显示，审查 skill 拿它对照代码，对不上就是
  危险信号。

**桌面设置文件（用户 2026-11-23）。** 界面偏好从 `preferences.json` 改成可以手改的 `<home>/desktop/desktop.toml`：

```toml
lang = "system"          # system / en / zh-CN
mode = "system"          # system / light / dark
palette = "standard"     # soft / standard / vivid
mica = false
mascot = "tumbler"
theme = "builtin"        # builtin，或已安装主题包的 id

[extensions]
enabled = ["io.github.alice.keys-panel"]       # 启用的扩展，按这个顺序加载

[extensions.settings."io.github.alice.keys-panel"]   # 扩展自己的设置，内容由扩展定
refresh_secs = 5
```

- 窗口写它时只改动到的键，保留注释、顺序与不认识的键（与 config.toml 的写法相同，用 toml_edit）。
- 手改之后，窗口重新获得焦点时重读并生效，不用重开窗口。
- 语法错了：窗口沿用上一次读到的值，在设置页写明哪一行错了，修好之前不写这个文件，不覆盖用户的手改；某个键的值不对
  只忽略那个键并提示。
- 置顶的请求仍在 `bookmarks.json`（机器数据，不给手改）。`preferences.json` 没有随任何发布出去（W5b 在 alpha.4 之后才
  合入），直接换掉，不做迁移。

**每个扩展的数据位置（用户 2026-11-23）。**
- 包本身：`<home>/desktop/extensions/<id>/`，装好后只读，升级时整体替换。扩展不往里写：写了，装着的字节就和 release 的
  哈希对不上，也没法干净地升级。
- 设置（给人改的）：`desktop.toml` 的 `[extensions.settings."<id>"]`，经 `api.settings` 读写；手改之后扩展收到变化通知。
- 数据（给程序存的）：`<home>/desktop/extension-data/<id>/`，经 `api.storage`（JSON 键值）与 `api.files`（这个目录里的
  文件，名字不带路径）读写。升级时保留，卸载时问要不要一起删。
- 别的地方都不是标准：`localStorage`、`sessionStorage`、IndexedDB、cookie、Cache Storage 与 service worker 和窗口页面
  共用一个来源，扩展写进去，用户看不到、删不掉，还碰得到界面与别的扩展的数据。不做限制，但审查 skill 把它当危险信号。

**安装提醒就是页面里的弹窗（用户 2026-11-23 裁定）。** 曾提议改由 Rust 弹系统原生对话框，防止已装的扩展或主题跳过或
藏起后面的提醒；用户否决：装进一个危险的包本身就已经被攻陷了，防御只在安装之前有效，原生对话框挡不住什么，反而牺牲体验。
提醒的文字要具体，例如「它能改实例的上游地址，把你的请求和 API key 发到别处」，比「能做你在窗口里能做的一切」更能让人停下来。

**format 的数据目录（建议，待用户确认）。** 每个 `[[format]]` 条目一个目录（home 下新增的目录，名字由条目的 `name`
安全地映射；新增目录要对照 C1–C8 冻结的 home 布局，确认只是新增），aProxy 创建它，并把绝对路径放进环境变量
`APROXY_FORMAT_DATA`，交给这个条目的每个进程（worker 与 control）。需要落盘的 format（worker 与 control 共享的状态、
学到的数据、缓存）放这里；审查 skill 把写到别处当危险信号。注意 format 进程继承实例的全部环境变量（`env` 表只是追加，
`src/transform/pool.rs:814` 没有 `env_clear`），也能读用户能读的任何文件，包括 home 里明文存着 key 的 config.toml 与
settings.json：这正是审查 format 时要盯的地方。

**审查用的三个 skill（用户 2026-11-23）。** `aproxy-audit-format`、`aproxy-audit-theme`、`aproxy-audit-extension`，放在
`.claude/skills/`、随发布出去，给替用户操作 aProxy 的 agent 用：装之前或装之后审一个包或 format，也给作者在发布前自查。
它们查的就是上面这些标准，加上一组通用规则（闭源是风险，编译好的闭源 format 是黑箱；混淆或加密的代码是风险，开源仓库里
讲清楚、而且能核对的才算可以；不符合数据标准本身就是危险信号）。标准在实现时若有改动，三个 skill 跟着改：t10b、t10c
合入时主代理逐条核对一遍。评测集放 `.agents/skill-evals/<name>/`。
2026-11-23 初稿已提交（每个 SKILL.md 加 `references/standard.md` 与 `references/signals.md`；清单里 114 条正则用 Python 编译过，
几条 ripgrep 专有写法用 ripgrep 试过）；评测只写了提示词（每个 skill 三条，含一条干净的、测误报），夹具与运行等用户同意。

**只有两种包：主题和扩展（用户 2026-11-23 补充）。** 主题包只放数据（颜色、CSS、图片），简单、可以分享。扩展是代码，
可以同时做功能和外观，两者分不清也不必分：没有单独的「主题型扩展」，任何扩展都能用下面的外观接口。
- 外观接口：界面状态（当前页面、吉祥物心情、重试与在途数、实例状态）的订阅、主题令牌（颜色变量）的读取、在窗口里挂
  自己的图层（背景层、标题栏装饰之类）的挂载点。扩展的样式叠在主题包之后。
- 扩展可以从网络加载脚本、样式、图片、字体和数据。因此窗口的 CSP（`aproxy-desktop/tauri.conf.json`，现在是
  `default-src 'none'`、`script-src 'self'`、`style-src 'self'`、`img-src 'self' data:`、`connect-src` 只到 IPC）要放开：
  脚本、样式、图片、字体、媒体、连接都允许 `https:`（连接加 `wss:`）以及 `blob:`/`data:`，样式允许 `'unsafe-inline'`
  （扩展会注入 `<style>`）。扩展的入口模块由 Rust 读出字节，前端用 `blob:` 地址加载。
- 安装提醒里写明：这个扩展会执行代码、可能连接网络。

**扩展与 format 互通（用户 2026-11-23 补充）。** format 的开发者可以同时发一个扩展，让人在界面里直接控制这个 format
（看它的状态、切换它的开关、调它的参数）。control 是一个**独立的进程**，和处理请求、返回的 worker 完全分开：
- **怎么启动。** format 在 hello 回复里可以给一个可选字段 `"control": {"args": [...]}`（例如
  `["keys", "control", "--config", "keys.toml"]`）。aProxy 需要时用同一个 `command`、同样的环境变量加这组参数起 control
  进程。不给这个字段就没有 control。用户不用改 config.toml。
- **怎么说话。** 沿用协议 1 的分帧：先 hello（`stages: ["control"]`），之后一问一答
  `{"v":1,"seq":n,"stage":"control","method":"<名字>","params":{…}}` → `{"v":1,"seq":n,"result":{…}}` 或协议已有的错误形状。
  `method` 与 `params` 的含义由 format 作者自定，aProxy 只转交、不解释。行长按 hello 里 `limits.control` 一组。
- **为什么分开。** 请求 worker 同一时间只处理一条消息，池满时要排队；control 若借用它们，会在流量大时等上最多
  `timeout_secs`，还会占走请求的名额。独立进程不占 `pool_max`、不排请求的队，挂了也不影响请求；期限另定（短，例如 5 秒），
  超时只换掉 control 进程。control 进程可以常驻（空闲一段时间后退役）。
- **生效时机由 format 作者决定。** control 进程与请求 worker 是不同的进程，互相看不到内存。改动要影响请求，format 自己
  选办法：写回自己的配置文件、放进共享文件让 worker 读，或者干脆告诉用户「重启实例后生效」。aProxy 不规定，协议文档里
  写明这一点。官方角色只在进程启动时读一次 TOML：persistent 模式下改了文件要等 worker 换新（干净的做法是重启实例），
  spawn 模式下一条消息就会读到新文件。
- **路由。** 实例的 IPC 加一个操作：按 `[[format]]` 的 `name` 转交一次 control 调用，带大小上限与超时。桌面加一个命令
  （以 Run 指定实例），给扩展的 `api` 加 `formats.list(run)`（这个实例配置了哪些 format、各自的名字、程序名与版本、
  有没有 control）与 `formats.call(run, name, method, params)`。
- **配对。** 扩展的 manifest 可以写 `formats: ["<程序名>"]`（hello 回复里的 `name`），说明它是给哪个 format 做的界面；
  实例页列 format 时，对有配对扩展的显示它挂进来的控制面板（扩展通过挂载点放进去）。
- **文档。** 协议的改动和「什么时候生效」写进 aproxy-format skill 的 `references/protocol.md` 与 `guide.md`（英文，主代理写；
  Codex 不改 `.claude/`）。官方 aproxy-format 的各角色暂不实现 control，以后按需加。

**重启 format（用户 2026-11-23）。** 重启实例当然会重启它的全部 format；另加一个更细的重启，只重启 format，
可以全部，也可以指定某个 `[[format]]` 条目（按 `name`）：
- 语义：选中条目的空闲 worker 立即收到 EOF 退役；正在处理消息的 worker 回完这一条后退役、不再回池；进行中的流留在原来的
  worker 上直到 close 或 abort 再退役；新消息一律交给新进程（新进程会重新读它自己的配置文件）；熔断清零；control 进程一并
  重启。之后像启动时一样握手一次，把结果（成功，或握手失败的原因）报告给调用方；失败时这个条目照 `required` 的规则处理，
  实例不退出。在途请求不受影响。
- 不重读 config.toml：`[[format]]` 本身的改动（command、args、stages 之类）仍要重启实例，输出里说一声。
- 入口：实例 IPC 一个操作；`aproxy restart <目标> --format all`（全部）与 `--format <name>`（可重复）。`all` 因此成为保留字：
  `[[format]]` 的 `name` 不能是 `all`（`[[format]]` 是 0.2.0 新加的，没有旧配置要兼容；和别名保留 `all` 同理）；桌面命令与实例页的按钮
  （每个条目一个，外加「重启全部 format」）；扩展 `api` 的 `formats.restart(run, names?)`。
- 官方 aproxy-format 不做热重载（用户 2026-11-23）：只在进程启动时读一次角色文件，改了文件用上面的重启让它生效。文档按这个写。

**t10b 任务书定下的细节（主代理 2026-11-23；以任务书 `.agents/codex.local/t10b-packages.md` 为准）。**
- 包里的文件经自定义协议 `aproxy-pkg` 交给页面（Windows 上 `http://aproxy-pkg.localhost/<id>/<路径>`），不再用 `blob:`。
  原因：主题的 `theme.css` 里 `url(assets/…)` 这类相对地址要能解析，`blob:` 做不到。`main.mjs` 也从这个地址加载，
  于是它可以相对 import `assets/` 下的模块，「打成一个文件」不再是技术限制，只是审查时的建议。审查 skill 的标准页里
  「从 `blob:` 加载、必须自包含」那句要在 t10b 合入后改掉，且要求审查包里全部脚本文件。
- 检查分两条命令：`package_check_*` 只做本地的读包、校验、算 sha256，立刻返回；`package_verify` 再去查 GitHub。页面先弹
  第一个提醒，同时等 GitHub。哈希比对按 digest 在该 release 的全部 asset 里找（浏览器会给下载改名），同名但 digest
  不同算「不一致」。**只有 digest 对上，仓库的 star 等数字才说明这个包**：没对上时谁都可以在 manifest 里写一个热门仓库。
- `desktop.toml` 语法错时 `read_desktop_state` 给 `preferences: null` 加问题（行、列），页面保留现有的值；写入一律拒绝、
  文件原样不动。
- manifest 的 `network` 与 `writes` 先按可选字段校验并返回（安装弹窗可以显示），用户还没确认；不要的话删掉也简单。
- 安装记录 `<home>/desktop/packages.json`（来源、sha256、安装时的 GitHub 事实），设置页用来显示「从哪来、当时核对得
  怎么样」；手工拷进来的包没有记录，页面要说明它没经过安装检查。

**接口暴露。** 扩展模块导出 `activate(api)`。`api` 是界面 store 的薄包装：读（实例卡片、最近请求、分页日志、请求详情）、
订阅变化、写（启动、停止、重启实例，读写配置，检查与安装更新）、外观接口、format 控制，以及 `api.settings`、`api.storage`、
`api.files`、`api.assets` 与在左侧导航加一页的 `api.ui.addPage`。写操作不另加授权，安装时的提醒已经说清楚了。

**前端已做（主代理 2026-11-23，`596bb92c`）。** 契约类型（`contract.ts` 的包一节，按 t10b 任务书 2.11 先写；
Codex 交回 Strings 后对齐 Fault 码）、演示后端（`lib/backend/demo/packages.ts`：按文件名挑安装场景，审核工具条可模拟）、
安装流程与逐条提醒（`lib/packages.svelte.ts`、`components/InstallDialog.svelte`、拖放 `DropOverlay.svelte`）、设置页
「主题与扩展」（`PackagesPanel.svelte`、`UninstallDialog.svelte`）、扩展宿主与接口第 1 版（`lib/extensions/`：
页面、两个挂载点 `overview.top`/`instance.top`、吉祥物、设置、键值、文件、包内资源；`api.formats` 等 t10c）、
主题包（theme.json 解析成令牌、theme.css 经包地址挂上）、`desktop.toml` 的语法错误横幅。旧的沙箱吉祥物、授权确认框、
旧数据层已删。浏览器里逐个场景走过（干净、闭源、对不上、关注少、连不上、不在 GitHub、主题、升级、读包失败），
`npm run check` 0 错误，`npm run build` 通过。还没在真实窗口里验证：包地址的 CORS 与模块加载、CSP 放开后的样式注入、
Tauri 拖放交来的路径，都等 t10b 合入后在 W5d 里看。

**t10b 合入之后（主代理 2026-11-23）。** Rust 一侧已合入 master（见 [codex-tasks-0.2.0](codex-tasks-0.2.0.md) 的 9b）。前端按下面
英文契约的「Faults to localize」对齐（`3e47277e`）：包与扩展数据的错误都是 `desktop_package`，按 `reason` 选句子，带包里的
路径、manifest 的字段或超出的上限；`desktop_toml` 的细节就是那条 TomlIssue；`desktop_state_io` 的文件多了 `preferences` 与
`packages`；已装包的问题（`EntryIssue`：manifest、entry、io）各有修复的说法。真实窗口验证按英文契约末尾「Real-window
qualification」的四步做，用 t10c 开工前构建的 `.agents/codex.local/desktop-t10b/aproxy.exe`，只在临时 home 里运行。
下面的英文契约是 Codex 在 t10b 里写的；t10c 会在文件末尾再追加一节英文，两节等 t10c 合入后一起改写成中文，与代码重复的
类型清单改为指向 `aproxy-ipc/src/desktop/packages.rs` 与 `aproxy-desktop/ui/src/lib/backend/contract.ts`。

**真实窗口验证（W5d 第一轮，主代理 2026-11-23）。** 用 `3e47277e` 构建的桌面程序，在临时 home 里运行，WebView2 开本地调试端口，
经 CDP 在页面里操作与检查（测试包与脚本在本机临时目录，不进仓库）。通过的：
- CSP 头：`style-src` 带 `'unsafe-inline'` 且没有 nonce 或哈希，`script-src` 保留 Tauri 的脚本哈希；页面注入的 `<style>` 生效。
- 安装流程：中文文件名的 ZIP 经文件选择框装入，来源记录里文件名、sha256、大小都对；在第一个、第二个提醒处取消都不留任何东西
  （暂存目录也清掉）；升级弹窗显示「已装 1.0.0，这次装 2.0.0」；写了公开仓库但没有对应 release 的包先给信任提醒、再给
  「找不到对应的发布」并说明 star 说明不了这个包；手工拷进来的包显示「未经检查」、默认停用。
- 主题：theme.css 经包地址加载，相对路径的背景图与字体按 theme.css 的位置解析并加载（字体 loaded，说明 CORS 头在）；包资源的
  响应头是 ACAO `*`、nosniff、no-cache、MIME 正确，缺文件回 404。
- `desktop.toml`：手改 mode 后重新获得焦点就生效；写入语法错误后显示行列、外观保留、改设置被拒且文件逐字节不变，错误的
  形状是 `desktop_toml` + TomlIssue；改好后横幅消失。外部改动 enabled 后重新获得焦点，页面卸下了扩展。
- 扩展：相对 import、shadow root 里的样式、二进制文件写入读回、设置、`api.assets.url`、模块地址的激活计数都对；升级后设置与
  文件保留；卸载不删数据时数据目录与设置表都留着，用目录路径（拖放走的同一个命令）重装后数据还在；选删除数据则全部清掉。
- 主窗口：`window.open` 返回 null 且没有新窗口，blob 下载没有落盘，顶层导航到外部网站被拦。

发现并处理的：
- **升级后子模块不更新**：入口模块带了版本查询串，可它相对 import 的模块地址不带，模块表按地址复用了旧版本（v2 的 main.mjs
  配着 v1 的 lib.mjs）。主题的相对图片与字体同理。改法：包地址第一段可写成 `<id>~<代次>`，Rust 只按 id 找文件；页面每次激活换
  一个代次，所有相对地址随之变新。Rust 一侧写进 t10c 任务书的 Revision 1.1，页面一侧（选代次、`api.assets.url`、主题样式表）
  等它合入后由主代理做。
- 安装弹窗在摘要没对上时也列出了仓库的 star、创建时间与许可证（设置页「安装时的核对」也附了 star）：改为只在摘要对上时显示
  （`34d82fa2`）。
- 没有实例时总览不渲染 `overview.top`，扩展挂的内容看不到：空状态也渲染这个挂载点（`34d82fa2`）。这两处只过了类型检查，
  浏览器开发服务器因内存不足被系统停掉、未经许可不重开，等下一次构建在真实窗口里复看。

原生拖放（用户 2026-11-23 允许占用鼠标后补做）：从资源管理器把 ZIP 与解开的目录拖进窗口，页面收到 drag-enter、drag-over、
drag-drop 三个事件并带着本机路径，覆盖层「松开即可安装」正常显示，随即弹出对应的安装提醒（升级、同版本重装）；在总览页、以及
页面重新加载（监听拆掉再注册）之后同样能接住。第一次拖没反应是因为起点被别的窗口盖住，不是产品问题。

还没验证的：摘要对上的情形（需要真实的 release，Rust 测试用本地假 GitHub 覆盖了）；macOS 与 Linux 的 WebKit。

**分工。** Codex 做 Rust 侧：包的读取与安装、列出、启停、卸载，包文件的 sha256，GitHub 查询，format 的 control 进程（hello 回复里的 control 参数、独立进程的启动与退役、实例 IPC、桌面命令），相应的桌面命令（安装分两步：
先「检查」返回清单摘要与可信度事实，再带检查时的句柄「安装」），并删掉 master 上 t10 的沙箱、租约、授权与审计代码；按上面的标准校验 manifest 与文件；`desktop.toml` 换掉
`preferences.json`；扩展的设置、键值与文件三组命令；（用户同意后）format 的数据目录。
主代理做前端：弹窗、设置页的列表、主题包的样式与图片加载、扩展加载与 `api`。

**Rust contract for 0.2.0.** Theme and extension packages are local installations. A theme is data; an enabled extension runs JavaScript in the main page with the same API as the page. Neither `network` nor `writes` limits its behavior. Present each applicable warning separately, before calling install. There is no sandbox, per-write consent, audit log, or format-control process in this interface.

### What authors put in a package

Choose a ZIP (stored or deflate) or an unpacked directory. `manifest.json` is at its root. A ZIP may instead have one enclosing folder containing the manifest and the entire package. Ignore `__MACOSX` components and `.DS_Store`, `Thumbs.db`, and `desktop.ini` files. They do not enter the inventory or the installation.

`manifest.json` is a UTF-8 JSON object, optionally preceded by a UTF-8 BOM: at most 64 KiB and 16 nested containers. Repeated keys anywhere, including inside an ignored field, are invalid. Unknown fields remain in the installed document but are not part of the returned manifest.

| Field | Rule |
|---|---|
| `schema`, `kind` | Required: integer `1`, and `theme` or `extension`. An unsupported integer schema is distinguishable from a malformed field. |
| `id` | Required: 3–100 ASCII `a-z0-9.-`, first and last alphanumeric; no `..` or Windows device stem. |
| `name`, `description` | Required: cleaned text (1–64 and 1–500 Unicode scalars respectively) or a map of 1–16 language tags to such text. Tags match `[A-Za-z]{2,3}(-[A-Za-z0-9]{1,8}){0,3}`. |
| `version`, `author` | Required: SemVer 2.0.0 without a leading `v`, at most 64 bytes; cleaned author text of 1–64 scalars. |
| `repository` | Optional HTTPS URL, ≤300 bytes, without userinfo, query or fragment. For GitHub use `https://github.com/<owner>/<repo>`: owner matches `[A-Za-z0-9](-?[A-Za-z0-9]){0,38}`; repo matches `[A-Za-z0-9._-]{1,100}` but is not `.` or `..`. One trailing slash and `.git` are stripped. Omit for closed source. |
| `license` | Optional 1–100 ASCII letters, digits, periods, spaces or `+ - ( )`. |
| `formats` | Extension only: ≤16 distinct `[A-Za-z0-9._-]{1,64}` program names. |
| `network` | Extension only: ≤64 distinct lowercase hostnames, optionally prefixed by `*.`; ≤253 bytes including prefix. DNS labels use letters/digits and interior hyphens, 1–63 bytes each. |
| `writes` | Extension only: distinct `config`, `instances`, `updates`, `formats`. All three extension-only fields are invalid on themes even if empty. |

Cleaning strips C0/C1 and DEL, U+061C, U+200B, U+200E/F, U+202A–202E, U+2060, U+2066–2069 and U+FEFF, then trims whitespace. Display the returned strings as text, not markup.

- **Theme:** `theme.json` is required (UTF-8 JSON object, optional BOM, no duplicate keys, 64 KiB). Rust returns the object; the page validates `ThemeSpec`. Optional `theme.css` is UTF-8, ≤1 MiB. Nested `assets/` may contain only PNG/JPG/JPEG/WebP/GIF/AVIF/SVG or WOFF2/WOFF/TTF/OTF (case-insensitive suffix).
- **Extension:** `main.mjs` is required (UTF-8 without NUL). Nested `assets/` can contain arbitrary file types. Relative imports and asset URLs resolve against the module URL.
- Both allow root documentation files beginning `README`, `LICENSE`, `LICENCE` or `COPYING`, case-insensitively. No other root entries.
- Maximum ZIP input 64 MiB, 2,000 files, 64 MiB per file and 256 MiB total uncompressed. The actual bytes read must agree with the entry's size; exceeding any limit aborts staging. Encrypted entries, ZIP symlinks and unsupported compression are rejected. Directory walks refuse symbolic links, reparse points/junctions and special files.
- Use slash-separated relative paths of at most 200 UTF-8 bytes and eight components. A component is 1–100 bytes, neither `.` nor `..`, contains no controls, backslash or `: * ? " < > |`, and cannot end in a dot or space. Before the first dot, `con`, `prn`, `aux`, `nul`, `com1`–`com9` and `lpt1`–`lpt9` are reserved regardless of case. Distinct paths, including intermediate directories, must remain distinct after Unicode lowercasing.

### Installation and the facts to show

A new check discards the previous staged copy, validates locally, and returns the inventory and source facts **before** any network lookup. The 30-minute token refers to the staged bytes, not the original path. `installed` is the previous version and enabled status: compare SemVer precedence for a downgrade. An ID cannot be upgraded across kinds. Cancel with `package_discard`; unknown and expired tokens are harmless there.

`package_verify` is optional and separate. It uses `GET /repos/<owner>/<repo>` and `/releases/tags/v<version>`, falling back to the unprefixed tag only on 404. A directory has no file digest and does not trigger a release query. Same-origin API redirects preserve a rename; case-only spelling changes are not renames. Requests send `User-Agent: aproxy-desktop/<version>`, `Accept: application/vnd.github+json` and `X-GitHub-Api-Version: 2022-11-28`, without a GitHub credential. The configured download proxy takes precedence over the environment. Connection deadline 10 seconds, each request 15 seconds; response budgets are 1 MiB for a repository and 8 MiB for a release. Rate exhaustion is 429, or 403 with `x-ratelimit-remaining: 0`. A release error does not erase the repository metadata already available.

**Repository facts describe these package bytes only when a release asset digest matched.** Anyone can put a popular repository in a manifest. Compare the hash of the whole original ZIP to *all* release asset digests, not to the hash of the extracted tree. A different local filename is fine for a match. A same-name different digest is a mismatch; absent digests are not mismatches. With no match, never present stars, license or author identity as authentication. Rust reports facts, not thresholds. The page handles full-power/code/network warnings and separate confirmations for closed source, unavailable facts, low stars or digest problems; cancelling any warning stops installation.

`package_install` consumes a token. Packages live at `<home>/desktop/extensions/<id>/`; `.staging-*` and `.old-*` hold unpublished or retired copies, never loadable IDs. The first list cleans leftovers but preserves its own live check. Installation, retirement and state edits share the desktop home lock. A swap keeps the prior directory until bookkeeping succeeds. On first extension install `enable` sets its load state; upgrades keep settings, storage, files and enable order. One theme is selected separately via `set_preferences`.

`packages.json` is a private `{version:1,packages:{"<id>":record}}` document beside `desktop.toml`. Each record has `installedAt`, `source`, `github` (null before verification) and an optional kind hint. Missing/corrupt/unknown-version records are not provenance. Listing rereads the manifest and required entry, not every asset. A hand-copied installation has no source or date; a broken one remains visible with a short issue but cannot be enabled. If even its kind is unknown, the list conservatively says `extension`, never `theme`. Uninstall resets an active theme to `builtin`, removes the ID from the load order and, **only** when `removeData` is true, removes its settings table and data directory.

### Window contract

`None` below means an empty JSON argument object. Results and keys are camelCase; discriminant values are kebab-case. Times are Unix milliseconds. The calls returning `null` have no payload. Existing `Bookmark` is unchanged.

```ts
type PackageKind = 'theme' | 'extension';
type Text = string | Record<string, string>;            // cleaned; an object maps language tags to text
interface PackageManifest {
  schema: 1; kind: PackageKind; id: string; name: Text; version: string; author: string;
  description: Text; repository: string | null; license: string | null;
  formats: string[]; network: string[]; writes: ('config' | 'instances' | 'updates' | 'formats')[];
}
type PackageSource =
  | { kind: 'file'; name: string; sha256: string; bytes: number }
  | { kind: 'directory'; name: string };
interface PackageCheck {
  token: string;
  manifest: PackageManifest;
  files: { path: string; bytes: number }[];               // sorted by path
  totalBytes: number;
  source: PackageSource;
  installed: { version: string; enabled: boolean } | null; // same id already installed (same kind)
  theme: Record<string, unknown> | null;                   // parsed theme.json of a theme
}
type GithubFacts =
  | { state: 'no-repository' }                             // closed source
  | { state: 'not-github'; url: string }
  | { state: 'repo-not-found' }
  | { state: 'unreachable'; reason: Unreachable; status: number | null }
  | { state: 'checked'; repo: RepoFacts; release: ReleaseFacts };
type Unreachable = 'network' | 'timeout' | 'rate-limited' | 'http';
interface RepoFacts {
  fullName: string; url: string; stars: number; forks: number; createdAt: number; pushedAt: number;
  archived: boolean; isFork: boolean; license: string | null; renamed: boolean;
}
type ReleaseFacts =
  | { state: 'match'; tag: string; asset: string; url: string }
  | { state: 'mismatch'; tag: string; asset: string }
  | { state: 'no-asset'; tag: string }
  | { state: 'no-digest'; tag: string }
  | { state: 'no-release' }
  | { state: 'not-a-file' }
  | { state: 'unreachable'; reason: Unreachable; status: number | null };
interface PackageEntry {
  id: string; kind: PackageKind;
  manifest: PackageManifest | null;                        // null when the installed manifest no longer validates
  enabled: boolean;                                        // extensions; always false for themes
  installedAt: number | null; source: PackageSource | null; github: GithubFacts | null;
  issue: PackageIssue | null;
  theme: Record<string, unknown> | null;
  dataBytes: number;                                       // extension-data/<id>/, for the uninstall dialog
}
interface TomlIssue { kind: 'syntax' | 'too-large' | 'io' | 'value'; line: number | null; column: number | null; key: string | null }
interface Preferences { lang: string; mode: 'system' | 'light' | 'dark'; palette: 'soft' | 'standard' | 'vivid'; mica: boolean; mascot: string; theme: string }
interface DesktopState { preferences: Preferences | null; enabledExtensions: string[] | null; bookmarks: Bookmark[]; reset: 'bookmarks'[]; desktopToml: TomlIssue[] }

type PackageIssue = { reason: 'manifest' | 'entry' | 'io' };
type FileInfo = { name: string; bytes: number; modifiedAt: number };
type ObjectData = Record<string, unknown>;
type None = Record<string, never>;

interface Commands {
  read_desktop_state: [None, DesktopState];
  set_preferences: [{ changes: Partial<Preferences> }, Preferences];
  package_list: [None, PackageEntry[]];
  package_check_file: [ArrayBuffer, PackageCheck]; // x-aproxy-file-name: encodeURIComponent(name)
  package_check_path: [{ path: string }, PackageCheck];
  package_verify: [{ token: string }, GithubFacts];
  package_install: [{ token: string; enable: boolean }, PackageEntry];
  package_discard: [{ token: string }, null];
  package_set_enabled: [{ id: string; enabled: boolean }, PackageEntry];
  package_uninstall: [{ id: string; removeData: boolean }, null];
  extension_settings_read: [{ id: string }, ObjectData];
  extension_settings_write: [{ id: string; changes: ObjectData }, ObjectData];
  extension_storage_read: [{ id: string }, ObjectData];
  extension_storage_write: [{ id: string; changes: ObjectData; reset: boolean }, ObjectData];
  extension_files_list: [{ id: string }, FileInfo[]];
  extension_file_read: [{ id: string; name: string }, ArrayBuffer];
  extension_file_write: [ArrayBuffer, FileInfo]; // x-aproxy-extension: id; x-aproxy-file-name: encodeURIComponent(name)
  extension_file_delete: [{ id: string; name: string }, null];
}
```

Raw commands take the file itself as the invoke body, **not** `{ bytes: [...] }`. Pass headers through the invoke options. `extension_file_read` returns a native IPC `Response` (JavaScript `ArrayBuffer`); it has no JSON envelope. Missing headers, incomplete percent escapes, invalid decoded UTF-8 or a JSON body in place of the raw body are bad input. A literal `+` does not mean a space.

### Window preferences and extension data

`<home>/desktop/desktop.toml` is the user-editable source of truth. Missing preferences give `lang="system"`, `mode="system"`, `palette="standard"`, `mica=false`, `mascot="tumbler"`, `theme="builtin"` and `extensions.enabled=[]`. A first write starts with a commented English template. Language is `system` or a manifest language tag (≤35 bytes), mode/palette are the enums above, mascot matches `[a-z0-9.-]{1,100}`, and theme is `builtin` or a valid package ID. A missing theme falls back to the built-in appearance on the page. `enabled` is an ordered array of distinct IDs: load in that order. Unknown keys are preserved. There is no preferences.json migration or pasted theme string. The separate private bookmarks file is unchanged.

On focus, call `read_desktop_state`. On a fatal `desktopToml` issue (`syntax`, `too-large`, `io`), both `preferences` and `enabledExtensions` are null: **keep the last good settings**, don't reset or save defaults. Syntax locations are 1-based line and Unicode-scalar column. A `value` issue identifies a bad known key: only that value defaults. This is independent of the `reset` array, which can contain only `bookmarks`. All preference writers edit the existing TOML and preserve unrelated keys, order, comments and formatting. Unreadable, syntactically invalid or >1 MiB documents are never overwritten. Writes share the per-home desktop state lock and replace files in their own directory.

All eight extension data commands require a currently installed extension, not merely an existing data directory. It need not be enabled. Reads do not provision data directories. Upgrade/reinstall keeps settings, storage and files; only an explicit uninstall with `removeData` removes them.

- **Settings:** `[extensions.settings."<id>"]`, up to 64 KiB as a serialized TOML table. Read an absent table as `{}`. Patch only top-level keys `[A-Za-z0-9_-]{1,64}`. Top-level null deletes; nested null (in either an object or an array) is not representable and is refused. Objects become inline tables, arrays become TOML arrays, and booleans, strings, finite floats and representable integers keep their types. A hand-written TOML datetime reads as its ISO/RFC text (no timezone is fabricated for a local date/time), an integer outside ±2^53 reads as decimal text, and a non-finite TOML float reads as text. Supply a string rather than an out-of-range JSON integer if exact digits matter. A write returns the entire resulting object.
- **Storage:** `extension-data/<id>/storage.json`, a JSON object, maximum 4 MiB serialized. Keys contain 1–256 Unicode scalars and no controls. Read missing as `{}`; never silently discard corrupt storage. `changes` sets top-level keys or deletes them with null. `reset:true` starts from `{}` without parsing the previous bytes, then applies the changes. Nested JSON values do not undergo TOML conversion.
- **Files:** `extension-data/<id>/files/<name>`. Use `[A-Za-z0-9._-]{1,128}`; no leading/trailing dot or Windows device stem. Only regular files with valid names are visible, sorted by name. `modifiedAt` is Unix milliseconds. Write replaces one file atomically and returns its info; read returns the exact bytes. Deleting a missing file succeeds. Refuse links/junctions/reparse points along the path. Limits: 64 MiB per file, 1,000 regular files and **512 MiB for files plus storage.json together**. A replacement charges the difference, not both versions. `dataBytes` in the list counts regular bytes under the ID's data directory without following links.

### Faults to localize

Faults have `{code,details}`. Do not parse OS messages or treat failed GitHub verification as a rejected invoke: its network/HTTP outcomes are the `GithubFacts`/`ReleaseFacts` above.

| Code | Details and meaning |
|---|---|
| `desktop_bad_input` | `{}`: malformed arguments/headers, invalid filename, ID or unrepresentable patch. |
| `desktop_package` | `{ reason, field, path, limit, max }`: nullable context; `path` is a cleaned package-relative name, at most 200 scalars, never the source's absolute path. See reasons below. |
| `desktop_toml` | `TomlIssue`: `syntax` (invalid TOML/UTF-8, line/column), `too-large` (>1 MiB), `io` (unreadable/unsafe file), or `value` (wrong field/table, key named). A read reports issues in `desktopToml`; a refusing edit throws this Fault. |
| `desktop_state_io` | `{file}`: could not access the private desktop state path or lock (`packages`, `preferences` or `bookmarks`). |
| `desktop_unavailable` | `{}`: the desktop background operation could not run to completion. |

`desktop_package.reason` values:

- **Container:** `archive` invalid ZIP/header; `encrypted` encrypted entry; `compression` unsupported method; `content` cannot decompress/read the member; `size-mismatch` actual size differs from declaration.
- **Layout:** `path` invalid name; `collision` ambiguous duplicate/case-folded path; `link` link/reparse point; `file-type` not the required node type; `unexpected-file` not permitted in this package kind; `missing` required file absent; `text` invalid text entry; `json` malformed JSON/object, including duplicate keys or excess depth.
- **Manifest:** `field` invalid/missing named field; `unsupported-schema` integer schema other than 1.
- **Lifecycle/data:** `kind-change` cannot reuse an ID across kinds; `not-installed` no installed package; `not-extension` operation requires an extension; `manifest`/`entry` cannot enable an installation with that list issue; `token` unknown, expired or already consumed; `changed` installed version/presence changed since check; `corrupt-storage` invalid JSON store; `io` filesystem operation failed; `rollback` could not restore prior state after a filesystem failure.
- **`limit`:** `max` is a count for `files` (2,000) and `extension-files` (1,000); otherwise bytes: `zip-bytes`/`file-bytes` 64 MiB, `total-bytes` 256 MiB, `json-bytes`/`settings-bytes` 64 KiB, `css-bytes` 1 MiB, `storage-bytes` 4 MiB, `extension-bytes` 512 MiB, private `records-bytes` 32 MiB. A corrupt, too-new or oversized provenance file is ignored on read and replaced on installation, not used as evidence.

`PackageEntry.issue` is deliberately smaller than a thrown Fault: `manifest` means a broken/missing manifest, `entry` a broken/missing required file, `io` a read error. Never activate a package with an issue; expose removal so the user can recover.

### Resource URLs, CSP and drops

Serve `http://aproxy-pkg.localhost/<first>/<path>` on Windows and `aproxy-pkg://localhost/<first>/<path>` on macOS/Linux. For reactivation and upgrades use `<id>~<generation>` as the first segment: `convertFileSrc(id + '~' + generation, 'aproxy-pkg')`, then append `'/' + path.split('/').map(encodeURIComponent).join('/')`. Tauri would encode the slashes if passed the entire path. Choose a distinct generation for the activation. Unlike a query on `main.mjs`, this prefix survives relative resolution: nested imports, images and fonts now have distinct browser cache keys too. Query and fragment still do not enter file lookup.

The handler decodes the first segment once (`%7E` works), splits at `~`, and looks up only the installed ID. The generation is 1–64 ASCII bytes from `[A-Za-z0-9._-]`; empty, invalid or multiple generations give an empty 404. Legacy `<id>/<path>` still works. The suffix grants no access to staging/retired directories and changes neither bytes nor headers.

GET returns the bytes; HEAD gives the same headers, no body. Unsupported method: 405. Missing, unrepresentable or unsafe paths, links and internal staging IDs: empty 404. Decode path segments exactly once; encoded separators cannot create hierarchy. Responses include `Content-Length`, `Access-Control-Allow-Origin: *` (module and font CORS), `X-Content-Type-Options: nosniff` and `Cache-Control: no-cache`. I/O runs off the window thread. Content types (case-insensitive): `mjs/js` → `text/javascript; charset=utf-8`, `css` → `text/css; charset=utf-8`, `json` → `application/json`, `svg` → `image/svg+xml`, `png/webp/gif/avif` → corresponding `image/*`, `jpg/jpeg` → `image/jpeg`, `woff2/woff/ttf/otf` → corresponding `font/*`, `wasm` → `application/wasm`, `html/htm` → `text/html; charset=utf-8`, `txt/md` → `text/plain; charset=utf-8`, otherwise `application/octet-stream`.

Here `PKG` means **both** `http://aproxy-pkg.localhost` and `aproxy-pkg:`. `self` keeps the embedded UI working; remote sources are intentional for trusted, full-power extensions, not a permission system.

| Directive | Sources | Purpose |
|---|---|---|
| `default-src` | `'self'` | No implicit remote source for other resource types. |
| `script-src` | `'self' 'wasm-unsafe-eval' 'unsafe-eval' https: blob: PKG` | App, package and remote modules, generated scripts, JS evaluation and WebAssembly. |
| `style-src` | `'self' 'unsafe-inline' https: blob: data: PKG` | App/package/remote CSS, generated and embedded styles, injected style elements. |
| `img-src`, `font-src`, `media-src` | `'self' https: blob: data: PKG` | Bundled, packaged, remote, in-memory and inline assets. |
| `connect-src` | `'self' ipc: http://ipc.localhost https: wss: blob: data: PKG` | Tauri IPC on both transports, HTTPS/WebSocket connections, fetching generated/inline/package data. |
| `worker-src`, `frame-src` | `'self' blob: https: PKG` | Embedded or packaged workers/frames, generated workers/documents, remote HTTPS content. |
| `object-src`, `base-uri`, `form-action` | `'none'` | No plugin objects, document base overrides or form submissions. |

Tauri 2.12.3's manager controls script/style CSP modification separately. Disable only `style-src` modification: a style nonce/hash would make the engine ignore `'unsafe-inline'`. Retain the script protections. The main webview still refuses external top-level navigation, new windows and downloads. There is no mascot-specific response policy. Native drag/drop is on; `getCurrentWebview().onDragDropEvent` registers event listeners and unregisters them on teardown. The capability grants `core:event:allow-listen` and `core:event:allow-unlisten`; dropped native paths go to `package_check_path`, not a general filesystem API.

### Real-window qualification (page owner)

1. Start the built app in a **disposable home**. Pick a ZIP with a non-ASCII filename and separately drop the ZIP and an unpacked directory. Confirm the inventory before any network result, cancel at each warning in turn, and check nothing was installed. Repeat to completion. Verify the page distinguishes a digest match, an unverified named repository and a hand copy with no source.
2. Activate a theme with CSS referencing a relative image and font; inspect the request origin, CORS, MIME and applied style in each platform engine. Refocus after a valid edit to `desktop.toml`, then after a syntax error: keep the previous appearance, show the issue and refuse a save until repaired.
3. Enable an extension importing `./assets/lib.mjs`. In the main page verify its relative assets, injected shadow-root style, and binary file write/read. Disable, upgrade the entry to visibly different behavior and re-enable. Inspect the generation segment on both module URLs and confirm the upgraded nested code actually ran. Check the stored settings/files survive upgrade and disappear only after choosing removal of data.
4. Try an external top-level navigation, popup and download: all remain denied. Inspect the shipped CSP header (no style nonce/hash), confirm native drops still arrive after listener teardown/re-registration, and repeat package URL, module and font checks on WebView2 and both WebKit hosts. The Rust tests do not substitute for these engine checks.

## Format control and restart (as built)

This is an instance interface, separate from package installation. Use `[[format]].name` to select an **entry**. Use the hello reply's `name` (`program` in a view) to discover an author's extension. Two entries can run the same program; keep both, and send mutations to the entry and run the user selected. A control call does not by itself change a running worker: the author defines the methods, storage, and when changes take effect. The official role programs read their role file at process start: restart the entry after editing that file. Changing `[[format]]` itself still requires restarting the instance.

### Format program: a separate control process

A protocol-1 worker's hello reply can include `"control":{"args":["control-mode"]}`. Omit it if the program offers no control. The list may be empty; at most 32 strings, each at most 4096 UTF-8 bytes. A malformed offer invalidates the hello. The first call starts the same executable in the workers' directory and environment, **substituting** these arguments for the configured ones. Its hello has `seq:0`, `process:"persistent"`, `stages:["control"]`, `worker_id:4294967295`; the reply must list `control` in `stages`. Ordinary spawn workers still see ID 0. Neither the control process nor a pinned stream consumes a request worker slot.

Exchange one UTF-8 JSON object per line. After hello, `seq` starts at 1 and advances once per valid reply. For example, the request and two alternative replies are:

```jsonl
{"v":1,"seq":1,"stage":"control","method":"settings.read","params":null}
{"v":1,"seq":1,"result":{"enabled":true}}
{"v":1,"seq":1,"fail":{"code":"unconfigured","reason":"No profile selected"}}
```

`method` matches `[A-Za-z0-9._-]{1,64}`. `params` is required, may be any JSON value (including null), and is limited to 65,536 bytes of compact serialized JSON. It is passed unchanged. Reply limits include the newline: 64 KiB for hello and `limits.control` (1 MiB) for the call. A valid `fail` does not end the session. Before returning either alternative, the instance replaces known secrets in string values, object keys, and both strings of a `fail`; a collision of redacted keys is a protocol fault, not permission to discard data.

One call runs and at most 16 wait **per entry**; the next is refused. The five-second deadline includes waiting, launch, hello and exchange. A caller that expires in the queue does not kill the active caller's process. Timeout during the exchange, wrong sequence, malformed/oversized reply or process death discards that process; the next call starts a fresh one. The idle process receives EOF after five minutes. A restart or shutdown retires it too. None of this opens the request breaker or occupies a request worker.

### Instance IPC (v1)

Read `ping.result.ops` first. Only send a mutating op to the verified instance that advertised it, not a new occupant of the same port. Use the ordinary success or error envelope; example request lines and replies (each is a separate line):

```jsonl
{"v":1,"op":"restart_formats","args":{"names":["keys"]}}
{"v":1,"ok":true,"result":{"configReloaded":false,"entries":[{"outcome":"restarted","name":"keys","program":"keys","version":"1"}]}}
{"v":1,"op":"restart_formats","args":{"names":null}}
{"v":1,"ok":true,"result":{"configReloaded":false,"entries":[{"outcome":"handshake-failed","name":"keys","reason":"invalid_output"}]}}
{"v":1,"op":"format_control","args":{"name":"keys","method":"settings.read","params":null}}
{"v":1,"ok":true,"result":{"result":null}}
{"v":1,"ok":true,"result":{"fail":{"code":"unconfigured","reason":"No profile selected"}}}
{"v":1,"ok":false,"error":{"code":"format_control_error","message":"format control failed: no-control","details":{"reason":"no-control"}}}
```

`names:null` means every configured entry, possibly none. A non-empty list selects names case-sensitively; `"all"` is not an entry name. Missing `names` is not authorization to restart all. Resolve the whole selection before changing anything. Return each selected entry once in configuration order. Each replacement probes from the instance's existing configuration; the publication of that replacement is the cutover. Old messages and streams finish on their original processes. A successful restart starts with a closed breaker and zero failures. On handshake failure the instance remains up: optional entries can be bypassed, required entries follow the existing unavailable/open-breaker behavior, notify-only entries use their own breaker, and a disabled breaker still permits later attempts. `reason` is the format fault kind (e.g. `spawn`, `io`, `died`, `timeout`, `protocol`, `invalid_output`), not localized prose.

`ping.result.formats` and the live `status --json` entry snapshot expose `program` and `version` once negotiated, `control` (false without an offer), and `restarted_at` (null until an explicit restart; the epoch-millisecond timestamp is set even on a failed attempt). The enumerated `status.formats` array is omitted when empty. Do not use program names as unique row keys or expose command arguments through the page view.

### CLI

`aproxy restart <PORT|ALIAS|all|idle> --format all` restarts all entries of each selected instance. Repeat `--format NAME` for a subset. Do not combine the format selector with `--force`, `--idle-threshold-secs`, or `all` plus individual format names. The instance is never restarted by a format selection.

| Notice | English | Chinese |
|---|---|---|
| Restarted | `Port {port}: restarted format {name} ({program} {version}).` | `端口 {port}：已重启 format {name}（{program} {version}）。` |
| Handshake failure | `Port {port}: format {name} handshake failed: {reason}.` | `端口 {port}：format {name} 握手失败：{reason}。` |
| Older instance | `Port {port}: this instance does not support restarting formats.` | `端口 {port}：此实例不支持单独重启 format。` |
| Other refusal | `Port {port}: could not restart formats: {message}` | `端口 {port}：无法重启 format：{message}` |
| Selection error | `Use --format all alone, or specify one or more format names.` | `请单独使用 --format all，或指定一个或多个 format 条目名。` |
| Configuration notice | `config.toml was not reloaded; restart the instance to change a [[format]] entry.` | `未重读 config.toml；修改 [[format]] 条目须重启实例。` |

With `--json`, stdout is one schema-1 document and notices go to stderr. Example mixed result:

```json
{"schema":1,"results":[{"target":"work","port":19901,"outcome":"restarted","name":"keys","program":"keys","version":"1","configReloaded":false},{"target":"work","port":19901,"outcome":"handshake-failed","name":"other","reason":"invalid_output","configReloaded":false}],"summary":{"ok":1,"failed":1}}
```

An instance-wide refusal instead produces one `outcome:"failed"` result with the ordinary nested `error`. Exit 0 for all successes, 1 for format failures only, 5 for mixed success/failure, 2 for invalid selection/flags. Ordinary target resolution retains its own exit codes. `configReloaded:false` on an entry is not a failed config reload; none was attempted.

### Window commands and faults

```ts
type Run = { port: number; instanceId: string };
type FormatView = {
  name: string; program: string | null; version: string | null;
  process: 'spawn' | 'persistent'; stages: string[]; required: boolean;
  breaker: 'closed' | 'open' | 'probing'; control: boolean; restartedAt: number | null;
};
type RestartOutcome =
  | { name: string; outcome: 'restarted'; program: string | null; version: string | null }
  | { name: string; outcome: 'handshake-failed'; reason: string };
type FormatCommands = {
  format_list:    [{ run: Run }, FormatView[]];
  format_restart: [{ run: Run; names: string[] | null }, RestartOutcome[]];
  format_call:    [{ run: Run; name: string; method: string; params: unknown }, unknown];
};
```

`format_list` is a separate read for the instance page rather than a change to every card. It projects snapshot `half_open` as `probing`; absent hello facts or restart time become null. `format_call` resolves the actual JSON result, **not** `{result:…}`; a valid program failure rejects with the author's redacted details. Keep error layers distinct:

| Code | Details and meaning |
|---|---|
| `format_name_reserved` | Config diagnostic `{name:"all"}`: choose another entry name. |
| `unknown_format` | IPC `{name}`: no matching entry; the whole selection was refused. |
| `format_control_error` | IPC `{reason}`: `no-control` (no offer/retired), `busy` (queue full), `timeout` (deadline), `protocol` (bad session/exit), `too-large` (params or reply), `spawn` (cannot start the process). |
| `bad_request`, `unsupported_arg`, `invalid_state`, `unknown_op` | Ordinary IPC validation, unexpected keys, non-serving state, or unavailable operation; check `ops` before sending. |
| `format_restart_failed` | CLI `{port,reason}`: one instance-wide refusal; the reason is the remote code, `run_changed`, or `ipc`. |
| `desktop_format` | Window `{reason}`: the six host-side control reasons above, `unknown-format`, or `unsupported` (older instance). |
| `desktop_format_fail` | Window `{code,reason}`: valid program-level refusal. Neither value is a host enum. |
| `desktop_bad_input`, `desktop_changed`, `desktop_busy`, `desktop_unavailable` | Existing window faults for invalid arguments, stale selection, state change in progress, or unavailable transport/response respectively. The usual run-selection faults also apply. |
