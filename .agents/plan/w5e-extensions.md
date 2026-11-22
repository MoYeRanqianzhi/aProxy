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

**分工。** Codex 做 Rust 侧：包的读取与安装、列出、启停、卸载，包文件的 sha256，GitHub 查询，format 的 control 进程（hello 回复里的 control 参数、独立进程的启动与退役、实例 IPC、桌面命令），相应的桌面命令（安装分两步：
先「检查」返回清单摘要与可信度事实，再带检查时的句柄「安装」），并删掉 master 上 t10 的沙箱、租约、授权与审计代码；按上面的标准校验 manifest 与文件；`desktop.toml` 换掉
`preferences.json`；扩展的设置、键值与文件三组命令；（用户同意后）format 的数据目录。
主代理做前端：弹窗、设置页的列表、主题包的样式与图片加载、扩展加载与 `api`。

**Rust contract for 0.2.0. Baseline `9f372fc0`; the native boundary is implemented, not qualified by a real window.** This adds to [Desktop API](w5b-desktop-api.md) (W5b), not to daemon IPC v1, install C1–C8, CLI commands or their JSON. Three different acts: copying a directory discovers it; enabling trusts those exact bytes to read; authorizing permits one specified kind of write. None implies the next. There is no install-from-URL or package code in the shipped page.

Trusted: the Rust process and the page shipped in the binary. Untrusted: every manifest field, directory entry, byte and message from a package, including what it calls itself. Rust cannot distinguish the shipped page from code the user injects into that same page. Private state protects against a package running through this interface, not a local user rewriting their own files. A redacted request can still contain a person's conversation or an *unknown* third-party secret: enabling a script is a meaningful disclosure, even with zero write permissions. Do not describe this as anonymous data.

## 1. Package v1

```
<home>/desktop/extensions/<id>/manifest.json
                               main.mjs          # page or mascot, never both entry types
                               assets/<key>.bin  # optional local bytes for those two kinds
                               theme.txt         # theme instead of module and assets
```

Example `manifest.json`: `{"manifestVersion":1,"id":"violet-desk","name":"Violet desk","version":"1.0.0","kind":"page","entry":"main.mjs","permissions":["instance-stop"]}`. UTF-8 without BOM. Root JSON object, all seven fields required, no null defaults. Unknown fields ignored, **not** forwarded or interpreted (still count toward byte, depth and digest limits). Reject duplicate keys at any level before decoding to a map. JSON nesting ≤8 containers including root. Report one deterministic issue per invalid directory, not an incomplete usable package.

|Field|Validation before exposing or loading anything|
|---|---|
|`manifestVersion`|JSON integer literal `1`, not the package version or the port protocol. Unsupported integer version is distinguishable from a malformed field.|
|`id`|ASCII `[a-z][a-z0-9-]{0,47}`, no trailing hyphen, no reserved device stem. Must byte-match the actual direct child directory **as enumerated**, never a normalized alias. Labels and module exports are not IDs.|
|`name`|String ≤256 UTF-8 bytes before cleaning; strip C0, DEL, C1 and bidi formatting/isolates (U+061C, 200E–200F, 202A–202E, 2066–2069), trim whitespace; 1–64 Unicode scalars afterwards. Display only as text, always alongside the ASCII id and version in settings/consent.|
|`version`|ASCII SemVer 2.0.0, 1–64 bytes, no leading `v`, including build/prerelease if present. Do not order trust by version.|
|`kind`, `entry`|`page` and `mascot` require exactly `main.mjs`; `theme` requires exactly `theme.txt`. The literal is checked, never used as an arbitrary file argument. No HTML entry, extra scripts/import tree, executable on PATH or symlinks.|
|`permissions`|Array of distinct strings from `instance-start`, `instance-stop`, `instance-restart`, `install-update` (≤4); may be empty. Both script kinds use the same read API and write checks. Theme requires `[]`. No wildcard, implication or `config` in v1 (§5).|

Pages export `default function mount(api: ExtensionApiV1): void | Promise<void>`; the module constructs its DOM **inside the child**. Mascots default-export the `MascotDefinition` of `ui/src/lib/mascot/types.ts:90` (`render(state)` / `icon(options)` return SVG strings; optional `css`); consume only inside the child, using the existing in-frame stylesheet, replay and freeze semantics of `ui/public/mascot-sandbox.js`. Never import the module to inspect it in Rust, Node, the settings page or a shadow root of the host. Installation validates nonempty bounded UTF-8 JS text without NUL, not JS syntax, export type or termination: missing exports/throw = frame runtime failure. The bootstrap sets the same read/write API on `globalThis.aproxyExtension` before importing either kind. No authority from the module's `id`, `name` or exported metadata.

`theme.txt` is **data only**: `aproxy-theme-v1:` then an object, outer whitespace trimmed. Mirror `ui/src/lib/theme.ts:277–363`, not just the prefix test in `src/desktop/state.rs:52`: `name` required, trimmed 1–40 UTF-16 units; optional `author` trimmed ≤40 (omit empty); `contrast=standard|more`, `font=rounded|system|mono`, finite `radius,motion` in [0,1.5] rounded to hundredths, `titlebarMin` in [36,64] rounded to nearest integer (ties up). Optional `light,dark` objects: only `primary,text,surface,ok,retry,error,warn` seeds; exact hex #rgb/#rrggbb or the parser's anchored, case-insensitive oklch decimal grammar (L 0..1 or 0..100%, C 0..0.5, H 0..360 optionally `deg`; no alpha, calc, exponent or CSS URLs). Ignore unknown fields, strip display controls as for names before accepting name/author. Apply JSON/depth limits. Rust emits a freshly serialized **known-fields-only** theme string, never raw CSS, unknown keys or raw theme text; UI parses again and applies its existing token stylesheet. A theme cannot contain even an unused script/asset. Disable/removal un-applies a package theme; don't persist a copy as a pasted theme in preferences. Package selection is distinct from built-in/pasted fallback (see §2).

**Budgets (bytes uncompressed).** At most 24 direct entries, counting invalid entries/files, not just valid manifests. Root missing = empty. On the 25th, abort enumeration (no unbounded sorting), fail closed for the entire inventory with a root issue; no arbitrary “first 24 win.” At most 10 regular files per package (manifest, entry, ≤8 assets), root plus at most one directory level `assets/`. Each asset key matches `[a-z][a-z0-9-]{0,31}`; no other entries, generated executables, thumbnails or dotfiles. Manifest and theme ≤16 KiB each; JS ≤256 KiB; each opaque asset ≤64 KiB; sum including manifest ≤512 KiB. No implicit decompression, MIME sniffing, or image decoding by Rust/host. Host gives bytes to child, not URLs into the home. These ceilings bound a worst-case full scan to 12 MiB of payload, and a single mount to 512 KiB before encoding; keep digest/metadata, not 24 unpacked packages. A caller cannot request a slice of an arbitrary file.

**Confine by handles, not by a canonicalize/startsWith check.** Start at the resolved home, open/check `desktop`, `extensions`, id and each component without following redirects. Components are literal enumerated names; no empty/`.`/`..`, separators inside a component, drive/UNC/NT prefix, ADS colon, percent-decoding, NUL, trailing space/dot, 8.3 tilde aliases, non-ASCII names or case-folded collisions. On *all* platforms reject device stems before the first dot (CON, PRN, AUX, NUL, CLOCK$, CONIN$, CONOUT$, COM/LPT 1–9 including superscript aliases; the ASCII alphabet already excludes several). No ZIP extraction. Reject non-regular entries, links including multi-linked files, sockets, FIFOs, devices, junctions and **all** Windows reparse points. On Unix enumerate/open relative to held directory FDs, `openat` with `O_NOFOLLOW|O_CLOEXEC`, `O_DIRECTORY` for directories, nonblocking file open; inspect type, link count and device on opened handles (refuse mount/device crossings). On Windows pin each verified ancestor with a directory handle denying write/delete sharing before descending; `OPEN_EXISTING`, `FILE_FLAG_OPEN_REPARSE_POINT`, directory backup semantics, check handle type, attributes, link count and enumerated long name, refuse sharing conflicts. Never read via a second pathname open. Count observed bytes with cap+1; verify identities/lengths and tree inventory across snapshot, fail on instability. Test ancestor/leaf replacement at the open boundary, including the extensions root. No security decision uses path-string prefix, locale, or an mtime. Do not claim defense against a privileged filesystem administrator.

Validation vocabulary below is also the fixture index. Within a package use structure/safety, manifest in table order, kind/layout, entry, projection order; sort bounded enumerations by raw name. Precedence for overlapping failures: safety/IO or budget exhaustion wins over interpreting more bytes. Field is a **fixed** selector (`ExtField` in §7), never the bad filename, secret value or parser error. Report a safe directory id only when available, otherwise null (a count of indistinguishable invalid directories is fine). Invalid ≠ disabled valid package.

|Issue reason|Trigger / representative invalid fixture|
|---|---|
|`package-count`|25th direct entry (whole root).|
|`io`|Entry disappears/unreadable, short read/changed inventory; deterministically inject read failure.|
|`path`|Unsafe component, traversal/absolute entry, spelling/case/alias collision, reserved name.|
|`link`|Symlink, junction/reparse, cross-device descent, or hard-linked regular file.|
|`file-type`|Not an expected directory/regular file (e.g. FIFO).|
|`depth` / `file-count`|Nested assets directory / 11th regular file.|
|`file-bytes` / `package-bytes`|Per-role cap exceeded / legal individual sizes whose sum exceeds 512 KiB.|
|`layout`|Missing manifest/entry, stray file, wrong kind's files, or asset in a theme.|
|`text`|Bad UTF-8, BOM or NUL/empty module.|
|`json`|Malformed/duplicate-key/over-deep JSON, non-object manifest.|
|`manifest-version`|Missing, wrong type or unsupported format version.|
|`field`|Missing/invalid known field, including name/semver/kind/entry/permissions or manifest id unequal to directory.|
|`theme`|Invalid theme prefix/field/shape/value; selector `theme`, no embedded parse message.|
|`private-data`|Unsafe identity/display/payload detected by the desktop known-secret check, or no trusted projection available.|

## 2. Discovery, state and lifecycle

Copy one complete directory locally (stage outside `extensions/`, rename in); no CLI, download, remote catalog or automatic enable. Rust scans off the UI thread at first attach, when the page calls `extension_list` (including Settings Refresh), and at the authority boundaries below. While attached, every 5 s make only a bounded metadata listing (names, kinds, sizes, modification times, no payload reads); a change hints that one full scan is needed. Coalesce scans, skip ticks during work, and never use metadata equality to authorize an action. Compare sorted results, emit `extensions-changed` only on change; no notification loop for a no-op refresh. `resync` means reread all visible extension panels too. Child RPC checks the current live lease; Rust **freshly snapshots/hash-checks** at enable, mount, write admission and final authorization, not just on a timer. No repeated disk scan per history chunk. Frames execute immutable captured bytes; new on-disk bytes never run under the old authorization. A change between two observations that leaves identical bytes is indistinguishable; reads from the old immutable frame may continue until detection, already delivered data cannot be recalled.

Identity: SHA-256 over `aproxy-extension-v1` then NUL, followed in ASCII full relative-path byte order by `u32be(path length), path, u64be(byte length), exact bytes` for **every** file including raw manifest, unknown fields and assets. Lowercase 64-hex. Revalidate the same snapshot you hand out. Enabling records `(id,digest)`, not semver. Any observed mismatch, invalidation, removal, disable or root overflow invalidates enablement, grants, leases, selections and pending requests; persist this before accepting more operations. An identical reinstall after **observed** removal starts disabled. Fresh state/corruption = no grants/no enablement. For a permission-only edit, even reordering the manifest changes the digest. Disabling does not delete the user's files; removing means removing the directory outside the UI. Enable at most one selected theme and one mascot; explicitly enabling another of that kind disables the previous one atomically. Enabled pages become navigation choices; mount scripts only if actually shown. Prefer a built-in mascot when a package page occupies the single script slot (§4); previews never secretly start code. Built-in preferences remain intact.

`<home>/desktop/extension-state.json` (private, version 1) is separate from preferences/bookmarks and the package tree. One atomic document for enablements, remembered grants, pending **descriptors** and audit ring so a crash cannot commit a grant without its decision. See §5–6. Live frame leases, original typed arguments, answer-token secrets, monotonic deadlines, immutable mount bytes and waiters exist only in Rust memory bound to the window/attachment. Startup under single-window ownership retires abandoned pending as denied and executing as unknown; never resume/replay them. Use `src/desktop/state.rs:99–125` discipline: per-home async serialization, `desktop/state.lock`, private same-directory atomic replacement, validate type/no reparse, hard bounds on read/write (256 KiB), blocking I/O off the event loop. No lock held across a prompt/child/daemon await. Validate persisted invariants before use; do not accept arbitrary fields as identities/targets. Corrupt/too-new: no grants usable, expose `desktop_extension_state` and require explicit recovery (see §7); do not silently overwrite a newer schema. If reset fails, stay closed. Never treat an I/O error as empty authority or successful revocation.

## 3. Loading an opaque frame

**Choose command bytes into a fixed, shipped frame document**, not an addressable package origin. Only the parent invokes `extension_open`. It transfers the frozen bundle to a trusted bootstrap over its private port; the child creates a JS Blob, imports its object URL, then revokes it. Assets are name→bytes, accessed through `api.asset(name)`; they have **no relative URL base**. Bundle imports at authoring time. Do not interpolate bytes into HTML, import in the parent, grant a Tauri asset scope over `desktop/`, or provide fetch-by-path. A theme needs no frame/JS. Close drops buffers and object URLs. Base64 costs ~4/3 on the command boundary plus transient JS copies; decode once, transfer ArrayBuffers.

|Alternative|IPC and document CSP|Assets, caching, memory|
|---|---|---|
|Rust custom `extension:` URL per document/asset|Can set response headers. A Tauri-registered scheme is considered local by `tauri-2.12.3/src/webview/mod.rs:1961`; a scheme is **not** an IPC permission boundary. Must prevent loading package content at top level. Windows maps custom schemes to HTTP(S) hosts.|Relative assets/streaming natural, but opaque-origin module loads need CORS. Requires confined serving on every request, correct MIME, revisioned cache keys and revocation independent of cache.|
|Raw parent `iframe.srcdoc` / parent-created `blob:` document|Opaque with the sandbox, but local-scheme documents inherit the creating context's CSP; a meta policy can only restrict, not relax it, and isn't a header sandbox. Parent `script-src 'self'`, `frame-src 'self'` and `connect-src` allow IPC for the app. Do not weaken the parent to make this work. No proof of IPC exclusion from a null origin alone.|No path server; relative URLs refer to the wrong base or fail. Rewriting arbitrary HTML/CSS isn't a secure resolver. Whole-document copies, lifetime/revocation burden.|
|Fixed shipped bootstrap + command bytes (chosen)|Separate response-header policy. Use the embedded app protocol for one compiled document, only trusted bootstrap before the port. Same IPC caveats below; parent policy needs no broader source. Set iframe sandbox **before** `src` or insertion. No package URL is ever a main-window URL.|Embed constant shell once; one live script frame, half-MiB mount, no per-package protocol, reusable URL or disk cache.|

Only Windows/WebView2 is eligible: `scriptSupport` is available exactly when the compiled qualification constant (initially false, opened by the lead after the real-window check), full shell fingerprint and all native startup hook registrations succeed. No setting or environment override. Otherwise return `unverified`. Theme packages work everywhere. Use `sandbox="allow-scripts"` only, `referrerpolicy="no-referrer"`, no feature delegation. Rust's `on_web_resource_request` sets headers on the exact embedded `/extension-frame.html`; never serve fallback `index.html` as the bootstrap. Ship one fixed classic inline bootstrap, hash **its exact built bytes** from trusted assets (no manifest hash). Replace the response policy for that one document. Missing/mismatched bootstrap or header → no package mount. Main CSP unchanged. Baseline `aproxy-desktop/src/lib.rs:311` adds `blob:` only on `/mascot-sandbox.html` while retaining the app's `connect-src`; that exception **is not** a package sandbox. Do not pass installed mascot bytes through the prototype loader. The lead owns `ui/public/extension-frame.html`. Build hash rule: from `aproxy-desktop/dist/extension-frame.html` (UTF-8, no BOM), require exactly one lowercase literal `<script>` and one `</script>`, in order, no script attributes or other script elements. Hash the UTF-8 script text between those delimiters, including its surrounding whitespace; HTML newlines normalize CRLF and lone CR to LF. Use standard padded base64 of SHA-256 for CSP. Also fingerprint the full original document bytes; serve that exact embedded document, not an index fallback or reconstructed HTML. Invalid or absent asset is a supported build and disables script opens. On the new shell (substitute the trusted base64 SHA-256 script hash):

```
Content-Security-Policy: sandbox allow-scripts; default-src 'none'; script-src 'sha256-<built-bootstrap>' blob:; script-src-attr 'none'; style-src 'none'; img-src data: blob:; connect-src 'none'; font-src 'none'; media-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; webrtc 'block'
Permissions-Policy: clipboard-read=(), clipboard-write=(), camera=(), microphone=(), geolocation=(), display-capture=(), fullscreen=(), payment=(), usb=(), serial=(), hid=()
Referrer-Policy: no-referrer
X-DNS-Prefetch-Control: off
X-Content-Type-Options: nosniff
Cache-Control: no-store
```

Use CSSOM inside the child for mascot styles; no external CSS, eval, reporting endpoint or package nonce. The iframe never has `allow-same-origin`. A package's DOM, SVG and CSS never enter parent DOM/CSS. Separately deny top-level navigation to the bootstrap; only the trusted app entry may be an executable main document. Present `is_app_url` checks scheme/host/port, not path.

**IPC evidence, not a window test.** `Cargo.lock:3802,3939,4701,4754,5151`: Tauri 2.12.3, tauri-runtime-wry 2.12.1, Wry 0.57.0, webkit2gtk binding 2.0.2, webview2-com 0.39.1. Actual engine is installed by the OS; registry here reports WebView2 Runtime **157.0.4341.60** (not the Edge browser version). No engine run for this document; Linux WebKitGTK and macOS WKWebView runtime versions unmeasured, not their binding versions.

* Inspected `tauri-2.12.3/src/manager/webview.rs:161–205`: invoke initialization is main-frame-only. `tauri-runtime-wry-2.12.1/src/lib.rs:5111` passes that flag on. `wry-0.57.0/src/webkitgtk/mod.rs:813` uses `TopFrame`, `src/wkwebview/mod.rs:780` uses the native main-frame flag; **`src/webview2/mod.rs:507` discards it**. `withGlobalTauri:false` only removes a convenience namespace. A Windows child can have internals. Tauri also documents that on Linux it cannot distinguish an iframe request from the window by capability. ACL `main` is not an iframe ACL.
* Tauri `src/ipc/protocol.rs:478` checks the HTTP invoke header and parses browser Origin; `src/webview/mod.rs:2007–2084` verifies the process invoke key before ACL. The alternate native message transport also leads to `on_message`; CSP `connect-src` is not a filter on native messages. Wry Windows registers `ICoreWebView2::WebMessageReceived` (`src/webview2/mod.rs:956`), not the frame message handler. Microsoft documents that the former receives top-level messages. Neither a missing global nor a rejected **reply** demonstrates that no handler executed.
* Windows boundary: install the **keyed sender only in the trusted top document**. On Windows use Tauri's `Builder::invoke_system` (`src/app.rs:1750`): preserve the pinned transport's serialization, channels, IPC and fallback for the app, but guard creation of the key-bearing closure by `window.top === window` and the fixed shipped app URL at document start. No key-bearing function/string on a child/global, no key in DOM, URL, port, diagnostics or storage. Do not append a late JS deletion to the default sender. Review every initialization script on upgrade; check that no second sender embeds the key. Prevent untrusted top-level documents/popups. No native hooks on macOS/Linux; scripts cannot be enabled there. Native stubs may exist; the invariant is **no successful native dispatch**, including core commands and channel fetch, not “no property called invoke.” No change to existing invoke arguments or wire. If the boundary cannot be established, withhold scripting on that engine; packages can still be inspected and themes used.

**Egress and containment ledger.** Browser semantics below are **documentation only**, not verified on these system WebViews. Rust/host controls are proposed; §8 specifies observations required before release.

|Surface|Enforcement and limits|
|---|---|
|fetch, XHR, EventSource, beacon, WebTransport, WebSocket incl loopback/IPC|Response `connect-src 'none'`. Independently enforce the keyed native boundary. No generic URL/open/proxy RPC.|
|Images, fonts, audio/video, SVG/CSS `url()`, imports, remote CSS/JS, objects|Destination CSP; only local data/blob images, fixed bootstrap and child blob scripts. CSSOM does not exempt URL fetches from destination policy. No URL passed to parent.|
|Forms, link ping|Sandbox lacks `allow-forms`; `form-action 'none'`, connect policy. Deny external protocols/new windows natively.|
|Self-navigation, links, meta refresh, top navigation, popups / `window.open`|Sandbox forbids top navigation (also by user activation), popups and escape; **does not forbid navigating oneself**. Neither `default-src`, referrer policy nor subsequent `load` prevents that request. Parent frame policy restricts embeddings; require engine enforcement cancelling child navigation *before request/redirect*, except initial exact trusted bootstrap. WebView2 has frame navigation events, not just Wry's `add_NavigationStarting` at `src/webview2/mod.rs:742`; no scripted frames on WK/GTK. Deny native new-window/download/external-handler actions including clicks. Destroy on unexpected navigation. Where pre-request enforcement is uncertain, scripting is not ready.|
|Prefetch, prerender, preconnect, DNS-prefetch, speculative parser loads|Default-deny destinations, no external source in trusted HTML, DNS-prefetch header. A resource hint/DNS lookup can send information **before** a cancellable HTTP response. Unsupported hints are residual egress, not covered by `connect-src` and not a Windows qualification veto. Disclose read trust at enable.|
|WebRTC/ICE, STUN/TURN, local network|Media device denial doesn't disable data channels or ICE. `webrtc 'block'` is specified in CSP3, **not** implied by connect or known supported on every engine. Record actual engine behavior. ICE or speculative egress can remain possible; these residuals do not decide Windows qualification.|
|Service/shared/dedicated workers, nested frames, storage|Opaque origin segregates parent cookies, Web Storage, IDB and service-worker registration; `worker-src`/`frame-src 'none'`, no same-origin grant. Blank, `srcdoc`, data/blob descendants need particular attention: inherited sandbox/CSP and no sender; an empty Window object isn't proof of an external load. No port/key in storage.|
|`postMessage` to parent/siblings, broadcast channels|Browser permits window messages; origin `null` doesn't identify a principal. Only §4's startup message from the expected window is accepted globally; no relays. Everything else on the bound port. No other untrusted sibling while a package runs. A port is transferable, not proof of authorship of each line of code.|
|Downloads, clipboard, devices|No `allow-downloads`; native deny downloads/permissions/new windows. Feature policies aren't uniformly supported; distinguish programmatic clipboard access (deny, incl legacy commands and native integration) from a user deliberately transcribing/copying displayed data. No file/clipboard RPC. Do not delegate file pickers, drag/drop of native files, screen sharing, geolocation, payment, pointer lock or custom protocol launch; verify native permission/chooser handlers in the child context. A user deliberately providing a file/text is not source redaction.|
|Resource exhaustion, user-mediated disclosure|Sandbox confines DOM and exceptions, not CPU, renderer crashes, structured-clone allocation before dispatch, pictures on screen or what a user elects to copy/type elsewhere. Bound work/traffic, don't await a child on the UI thread; detach and show a trusted placeholder. No process-level availability or DRM guarantee. Stronger availability requires separate resource limits.|

Primary references: `https://www.w3.org/TR/CSP3/`, `https://html.spec.whatwg.org/multipage/origin.html`, `https://v2.tauri.app/security/capabilities/`, `https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/frames`, `https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2`. Documentation sources (not engine measurements): WHATWG HTML, *Sandboxing*, *Policy containers*, *Cross-document messaging*; W3C *Content Security Policy Level 3* §§4.3, 6 and *Permissions Policy*; Tauri v2 *Capabilities*, remote API access caution; Microsoft *Using frames in WebView2* and Win32 `ICoreWebView2::{AddScriptToExecuteOnDocumentCreated,add_WebMessageReceived}`. Pinned source paths above qualify the general documentation. Test the production shell, not just Chrome; record actual OS/engine build and dispatch/network observations.

## 4. API v1: frame ↔ shipped host

**Binding, before any package script.** Mount at most one untrusted document in the window (built-in UI stays usable). Host obtains a Rust lease for this attachment/id/digest, registers a fresh element and a cryptographically random 128-bit bootstrap nonce before setting its fixed shell URL `.../extension-frame.html#<nonce>`. Nothing sensitive in a query. The shipped child speaks first: after the initial `load` has fired, create a MessageChannel, keep port1 and transfer port2 to `parent.postMessage({type:'aproxy-hello',versions:[1],nonce}, targetOrigin, [port2])`; use the build's exact parent origin where representable, otherwise `'*'` **only** for this empty-data hello to `parent`. Delay hello by one task after load so a subsequent load is always teardown. Host checks `event.source === registeredIframe.contentWindow`, opaque `event.origin === 'null'`, waiting generation, exact one-use nonce, one port, shape and supported version. Never authenticate by id in a message or by `origin` alone. Consume registration synchronously before replying. A stale/mismatched/extra port is closed; unsupported version or timeout (5 s) closes the mount without bytes. No later global message is RPC; never send capabilities/data to `contentWindow` with target `'*'` (WindowProxy survives navigation).

On the bound port host sends `init` below, then child imports only in its own realm, makes `api` available, calls page `mount(api)` or verifies mascot exports, then sends `ready`. Permit calls during async `mount` after `init` to avoid deadlock; initial render/context notifications wait for ready (retain latest). Init once; second navigation/reload creates **a different element, port, lease and nonce**, never rebind the old port. Timeout for readiness 10 s (not a guarantee against a wedged renderer). No lease, approval capability, Tauri channel, page backend, auth event, filesystem path selector or executable URL crosses this port. W5b choice/cursor/check tokens in results are scoped observations, not authority.

Types in this section reuse names from `ui/src/lib/backend/contract.ts` and mascot `types.ts`, not similarly named prototype types. `Context` is host-computed, not arbitrary CSS; colors are concrete resolved colors from the shipped palette code. No known-secret theme names, file contents or diagnostic prose in notifications.

```ts
type Context = {lang:'en'|'zh-CN'; theme:'light'|'dark'; palette:MascotPalette;
                intensity:Preferences['palette']; reducedMotion:boolean};
type Topic = 'instances'|'requests'|'settings'|'desktop-state'|'update';
type Notice = {type:'resync'} | {type:'stale';topic:Exclude<Topic,'requests'>}
            | {type:'stale';topic:'requests';run:Run};
type ExtMethods = { // [params,result]; parameter/result DTOs are JSON
  snapshot:[None,Snapshot];
  queryLogs:[{run:Run|null;cursor:Token|null;limit:number},LogPage];
  requestDetail:[{ref:Ref},Detail];
  bodyChunk:[{ref:Ref;part:BodyPart;offset:number;limit:number},Chunk];
  cancelDetail:[None,null]; listConfigs:[None,Choice[]]; readConfig:[{target:ConfigTarget},ConfigView];
  readSettings:[None,SettingsView]; readDesktopState:[None,DesktopState];
  readAutostart:[None,Registration]; about:[None,About]; desktopInfo:[None,ShellInfo];
  updateOffer:[None,UpdateInfo|null];
  startInstance:[{target:StartTarget},Run]; stopInstance:[{run:Run},null];
  restartInstance:[{run:Run},Run]; installUpdate:[{check:Token},InstallEnd];
  subscribe:[{topics:Topic[]},null]; // atomic replacement, [] unsubscribes
};
type ExtensionApiV1 = {
  request<K extends keyof ExtMethods>(method:K,params:ExtMethods[K][0]):Promise<ExtMethods[K][1]>;
  context:Readonly<Context>; asset(name:string):Uint8Array|null; // key WITHOUT assets/ or .bin
  onNotice(fn:(notice:Notice)=>void):()=>void;
  onContext(fn:(context:Context)=>void):()=>void;
};
type FrameRequest = {[K in keyof ExtMethods]:{v:1;type:'request';id:number;method:K;params:ExtMethods[K][0]}}[keyof ExtMethods];
type FrameError = {code:Fault['code'];details:None}; // allowlisted code only, see below/§7
type Reply<K extends keyof ExtMethods> = {v:1;type:'reply';id:number} &
  ({ok:true;result:ExtMethods[K][1]}|{ok:false;error:FrameError});
type ToHost = FrameRequest | {v:1;type:'ready'|'failed'} | {v:1;type:'seen';seq:number};
type ToChild = {v:1;type:'init';kind:'mascot'|'page';context:Context;module:ArrayBuffer;
                assets:{key:string;bytes:ArrayBuffer}[]} | Reply<keyof ExtMethods>
  | {v:1;type:'notice';seq:number;notice:Notice} | {v:1;type:'context';context:Context}
  | ({v:1;type:'render';replay:number;freezeAt:number|null} &
       ({state:MascotState}|{icon:MascotIconOptions}));
```

`api.context` is a getter for the latest value; deliver context changes independently of subscriptions and only on actual change. No user locale outside the host's resolved language. `asset` returns only this mount's bytes. Host sends `render` **only** to a mascot, coalesces by animation frame/latest state; clamp icon px to 1..512, freeze time to 0..60000 ms or null, replay a nonnegative safe integer. Inside child delegate to existing render/icon contract, never return SVG to host. Boot failure = fixed `failed`, no exception payload. A theme gets no port, no resources, no notifications.

**Read routing.** `snapshot`, `readSettings`, `readDesktopState`, `readAutostart`, `about`, `desktopInfo` → correspondingly `snapshot`, `read_settings`, `read_desktop_state`, `read_autostart`, `about`, `desktop_info`. Only the shipped typed host invokes those fixed commands; unchanged W5b DTO. `queryLogs/requestDetail/bodyChunk/cancelDetail` → lease-scoped siblings of `query_logs/request_detail/body_chunk/cancel_detail`, sharing Rust implementation/projection, **not** their page cursor/selection. Baseline `bridge.rs:396,512,549` has only two cursor slots and one selected request per attachment: simply forwarding these calls would cancel somebody else's work. Each lease has two cursor slots, one current detail, one active chunk, host assigns increasing generation on every new detail; cancel/supersede actually drops pending read; late responses discarded. `listConfigs/readConfig` share the `list_configs/read_config` projection but keep the extension's choice nonce separate (`src/desktop/instances.rs:258` rotates the page's nonce). Use the corresponding private choice table also for attributed start. `updateOffer` reads the most recent **page-initiated** check result unchanged or null; `check_update` itself launches a child, can contact the release service and replaces the offer, so not invocable by a frame. A check token is an observation, **not** an approval. `installUpdate` accepts only the still-current offered check. `titlebar_insets` is chrome geometry, not extension data; `validate_config` is a dry run for a writer not supported here, not a read of effective config. `rescan/attach/detach/ack` belong to the host, never used to drive discovery/subscriptions on a frame's behalf. No preview-only read of raw history/config, shell backend or installer output.

W5b §5 remains the source boundary: only verified run/ref, source-redaction-capable peers, current secret set, structured config, vetted fields/diagnostics; body bytes through the same margin-aware, base64 decode/recheck/re-encode path. No prefetch, body stream, automatic retry or retained body cache. `limit` 1..100 rows or 1..65536 bytes per explicit chunk; offset and returned byte counts use W5b safe integers. Host does not post a result if lease is retired meanwhile. Existing W5b config integers remain decimal strings; no new overflowing 64-bit counters are exposed. For package-derived data, check public identities and display fields against the current private matcher, mask unambiguous display labels and withhold unsafe identities, check entire captured payload for known secret bytes before opening; never redact by editing executable code. If a safe projection can't be made, no bytes. Never post persisted package themes/labels/events from an obsolete projection as a fallback. This protects configured secrets at source, not secrets a user chooses to put in arbitrary code or novel encodings.

**Notices.** One real W5b attachment per window; never expose session, raw `Frame`, grant events or confirmation metadata. On `instances-changed` → instances; `requests-changed` → requests for that run; `settings-changed` / `desktop-state-changed` → corresponding topic; `update-progress` or changed check offer → update (reread snapshot/offer). `rescued` is ephemeral animation, not replayed. `resync` → resync all subscribed topics; `extensions-changed` is host-only: reconcile live identity and unmount first. Successful subscribe sends a resync if nonempty. Maximum one unacknowledged notice/port; coalesce ≤16 stale keys, overflow → one resync; seq from 1, bootstrap acknowledges on dequeue (`seen`), 10 s without ack retires the port. Never delay ack of the real Rust channel on a frame. Context/render updates collapse to latest, not a historical feed.

**Bounds and faults.** After hello only exact declared plain records/arrays and scalar types, no transferred ports, exotic objects, cycles or extra keys; binary transfer only in host init. Child→host RPC/control JSON UTF-8 ≤16 KiB, nesting ≤8, nodes ≤256; check with bounded traversal (not blind `JSON.stringify` of an unbounded structured clone). Use an explicit switch/Map, never index `Backend` or `invoke` by an incoming method string. IDs positive safe integers strictly increasing per port; reply once by id, completions may reorder. Up to 4 outstanding calls and 1 active chunk, 20 new calls per second (token bucket capacity 20, refill 20/s), 2 outstanding writes/port. No queued overflow: `desktop_busy`. Retire on malformed/unknown envelope or ID replay; well-formed unknown method/invalid params → `desktop_bad_input`. Three violations/throttle hits in 10 s → close this mount. Host→child ordinary JSON reply ≤2 MiB UTF-8 (else fixed `desktop_extension_limit`); init ≤512 KiB binary plus fixed envelope. Validate lease/limits again in Rust for attributed commands. Never post stack traces, raw rejection, terminal logs, complete approval/audit records or parser messages. For known W5b Fault on an allowed route, keep only its recognized code with `{}`; unknown/transport → `desktop_unavailable`. Frame-side thrown errors carry that envelope. `desktop_extension_denied` covers refusal, undeclared permission, expired/closed prompt without distinguishing user behavior. Package cannot tell whether an allow was remembered from the reply shape. Close retires the port and selected futures, rejects local outstanding promises as `desktop_cancelled`; never replay a write. A committed execution is allowed to finish under its original identity even if nobody is listening.

## 5. Authorization in Rust

Only `instance-start`, `instance-stop` (graceful verified run), `instance-restart`, `install-update` can be requested. No `control` umbrella. Four **distinct attributed commands**, all require a Rust-issued lease, no optional `extensionId`, no generic dispatch. Use an extension-only host adapter with no reference to page mutation methods. Its start/stop/restart/install cases can only call `extension_start/stop/restart/install`; TypeScript exhaustive route tests use spies to assert zero page writes for every case. Ordinary page commands retain their own signatures; neither forwarding `method` to `invoke` nor dropping an optional attribute is possible through this adapter. Rust resolves lease → attachment, immutable principal and manifest, **not** a frame-supplied name or a client-supplied `approved` boolean. Unknown methods, undeclared operations, disabled/changed packages and released handles are denied before invoking `DesktopOps`.

**Never offered**, including via aliases: force stop, restore/forget, rescan, watchdog changes, login registration changes, set preferences/pins, validate/write config, change desktop chrome, direct W5b start/stop/restart/check/install, extension list/enable/open/close/grant/revoke/pending/answer/audit, core/plugin/window APIs, arbitrary URL/command/file/network or low-level IPC/channel. Read-only preferences/bookmarks/watchdog/registration and projected run state remain readable. Absence from the fixed table means deny; adding a page command does not add an extension method.

**Config ruling: omit config writes in 0.2.0.** `base_url` or `proxy` can route later traffic/credentials elsewhere without the editor ever reading an API key. Confirmation for *every* exact edit, never remembered, supports the most automation but requires a truthful destination/credential-impact diff, external ACL handling, stale-file check and no hidden second edit; even a technically accurate prompt can be misunderstood. Key-limited grants (e.g. numeric timeouts only) reduce rerouting risk but need semantic review of combinations, future keys and persistent disruption. Omitting writes sacrifices extension config editors; native read-only ConfigView and the user's own config editor still work. Do not implement a blanket grant or expose a dormant `config` capability. **Install: always ask**, even on the same version or after a previous allow. It replaces the binary and may restart every instance of the home; the convenience of remembering doesn't justify silent future installations.

Remember **only** on an explicit separate “allow future <operation> on any target in this home for this exact package” choice for one instance verb. Default button deny; also offer “allow once”. A grant `{grantId,id,digest,permission,createdAt}` is scoped to this home (file location), exact digest, exactly one verb and any *verified local target of that verb*. Never key by display strings, version alone, port alone, or pathname from JS. Show this scope in settings/prompt. Max 3 remembered grants per enabled script, ≤72 total. Removing even one permission changes digest and cancels *all* its grants. `extension_revoke` removes the exact opaque grant; stale token → `desktop_changed`, no new grant substituted. Disable/revoke/removal cancels pending approvals of affected scope; ordering with execution is serialized below. Explicit revoke while already executing prevents **future** actions, does not claim to undo this one.

**One transaction, not a “ticket to execute later” sent to JS:**

1. Frame asks `stopInstance({run})`, for example. Host maps port to live lease, invokes only `extension_stop({lease,run})`. The call remains pending. Rust validates input, verifies current enablement, declared verb, immutable content and actual target. Start: resolve choice under the lease's private nonce, canonical file/default identity, effective listen port and private config revision (including relevant inherited defaults); stop/restart: exact verified `(port,instanceId)`, for restart also reproducible desired spec; install: checked offer token, exact installed binary fingerprint, channel, variant and candidate. Store prepared typed args/fingerprints in Rust, not in page or audit. No shell strings or forced action. Reject if the target cannot be unambiguously and safely described.
2. Rust creates an audit operation ID. If a matching remembered grant exists, go to execution check. Otherwise atomically persist pending descriptor, link audit and register waiter/token under the window fence; only **after** successful commit notify `extension-consents-changed`. Max 2 pending per lease, 2 globally (one script slot). Oldest first; no hidden overflow queue. At most one dialog in the host. Timeout 60 s from creation via Rust monotonic clock, including queue time. Rust task expires without polling/visible window; wall `expiresAt` for display only. `extension_pending` is authoritative after any missed event or resync. Answer token = fresh 256 random bits (separate namespace from operation/grant/lease), only in memory and returned by that page-only command. Failure to obtain randomness or save ⇒ no write.
3. **User sees a dialog outside the iframe**, focuses deny, makes embedded content inert while modal, Esc/dismiss = deny. Render only Rust `Consent` from §7: verified safe name + *id*, version, digest, fixed install location relative to this home; verb and exact resolved target: start choice/path + port, stop/restart port + run identity, install version/channel/variant + warning that scope is *all instances in this home* (no guessed restored ports). Show home from the page's `about`. No package-supplied reason, HTML, icons, target label or “remember” default. Name is untrusted display, never evidence of publisher. If identity/path/port cannot be safely distinguished, Rust refuses rather than prompting with an ambiguous masked target. No answer token, prompt contents or queue notifications in frame results.
4. Trusted button invokes `extension_answer({session,token,decision})` with no fresh action/target fields. Rust atomically matches current window/attachment, active lease, pending token, deadline and allowed decision; consume single use before releasing waiters. Stale/wrong/second use → `desktop_changed`; invalid `remember` for install → `desktop_bad_input`, no execution. Close/detach, replaced session, frame teardown, refusal, expiry, disable, version/removal and shutdown settle a still-pending call without effects. Window close must retire even when `Bridge::close_requested` hides it for a running installer. If storage fails, fence all affected authority in memory, no retry/replay, surface the state fault. Startup never reissues an old token.
5. **Execution boundary**: under the Rust lifecycle/authority serialization, compare exact saved plan, fresh package digest, live lease, deadline for a pending answer, grant/decision, current target and any check/offer; reserve the same operation once. Stage remembered grant only for a valid approved plan. Persist authorized/executing and decision **before** allowing effects. In the operation implementation consume the checked target itself, not a re-resolved looser ID; use normal preflight/run generation/installer offer/fingerprint checks at the last side-effect point. For start/restart do not reload changed config/spec after it was approved: compare revision and launch the checked snapshot or refuse `desktop_changed`. Nothing stale triggers an automatic re-prompt. A revoke/close that wins the fence prevents admission; one after admission cannot cancel an already issued daemon action. Don't hold desktop state/file locks across OS/IPC work. Same install worker/progress, close behavior and single-job slot as the page; no change to installer C1–C8. `extension_answer` confirms handling of the decision only; **original attributed call** resolves with operation result or rejects with Fault. Rust records completion even if the port disappeared. No “execute with this grant” endpoint.

## 6. Audit and bounded persistence

In the private state document: metadata `schemaVersion:1`, enabled digest by id, grant records above, pending descriptors (no redeemable token), audit rows. Store no package bytes, user-entered text, request/response bodies, credentials, raw args/config, confirmation/lease/check tokens, PID, command line or install stderr. Operation/grant IDs are opaque random correlation IDs, not authorization. For each attributed, parsed write record principal id/digest/version, verb, redacted resolved target if verified, requested/finished time (Unix ms), allowed by which grant id or one-shot decision id, outcome and stable Fault code only. Invalid lease/undecodable input has no verified principal and is a transport/input refusal, not falsely attributed; admission failures for a known lease, including undeclared/busy/changed, get bounded refusal rows with null target if unverified. A frame-local throttle rejection hasn't reached Rust and isn't reported as a Rust attempt. Re-project disk rows with current secret matcher before displaying; never persist unsafe strings just because a package supplied them. `extension_audit` page-only; notice `extension-audit-changed`.

FIFO up to 128 rows and 160 KiB serialized audit, evict oldest **terminal** records before adding one; never evict pending/executing. ≤2 executing and ≤2 pending; at most 512 UTF-8 bytes per projected target path/label, otherwise refuse exact-target authorization. Validate envelope 256 KiB (including grants, untruncated ids, pending and audit); if an entry still won't fit, refuse before side effects. Corruption, store-full and unsupported schema fail closed. If a completion save fails after a real effect, don't say it failed to run: leave durable `executing`/eventual `unknown`, return state fault to the page, no automatic retry. On restart mark unfinished pending denied, executing unknown, never synthesize success. Audit is a bounded local activity record, not a tamper-proof ledger. Settings distinguishes refused from failure after authorization.

## 7. W5b addendum for `contract.ts`

Add these definitions beside the existing named DTOs; merge `ExtCommands` into `Commands`, `ExtEvent` into `DesktopEvent` and `ExtFault` into `Fault`. Uses existing `None`, `F`, `Selection`, `Token`, `Session`, `Time`, `FaultCode` and DTOs; all IDs comparison-only strings, all numbers nonnegative safe integers (timestamps epoch ms), no additional 64-bit decimal counters. Strict fixed command inputs, stable `{code,details}` only; reject serde/Tauri/process error prose. No new CLI/IPC code path.

```ts
type Kind = 'page'|'mascot'|'theme';
type Control = 'instance-start'|'instance-stop'|'instance-restart';
type Permission = Control|'install-update';
type ExtKey = {id:string;digest:string};
type Principal = ExtKey & {name:string;version:string;kind:Kind};
type ExtField = 'root'|'directory'|'manifest'|'manifestVersion'|'id'|'name'|'version'|'kind'|'entry'|'permissions'|'assets'|'theme';
type InvalidReason = 'package-count'|'io'|'path'|'link'|'file-type'|'depth'|'file-count'|'file-bytes'|'package-bytes'|'layout'|'text'|'json'|'manifest-version'|'field'|'theme'|'private-data';
type InvalidPackage = {id:string|null;code:'desktop_extension_package';details:{reason:InvalidReason;field:ExtField}};
type Grant = ExtKey & {grantId:Token;permission:Control;createdAt:Time};
type PackageInfo = Principal & {permissions:Permission[];enabled:boolean;grants:Grant[]};
type Inventory = {packages:PackageInfo[];invalid:InvalidPackage[];scriptSupport:'available'|'unverified';stateFault:Extract<ExtFault,{code:'desktop_extension_state'}>|null};
type Lease = string; // host only, fresh random, scoped to session
type Bundle = {kind:'theme';theme:string} | {kind:'page'|'mascot';lease:Lease;moduleB64:string;assets:{key:string;dataB64:string}[]};
type TargetView = {op:'instance-start';kind:StartTarget['kind'];label:string;path:string;port:number}
  | {op:'instance-stop'|'instance-restart';run:Run}
  | {op:'install-update';version:string;channel:UpdateInfo['channel'];variant:UpdateInfo['variant'];scope:'all-home-instances'};
type Consent = {token:Token;decisionId:Token;principal:Principal;target:TargetView;requestedAt:Time;expiresAt:Time;canRemember:boolean};
type ExtDecision = 'deny'|'once'|'remember';
type Authority = {kind:'none'}|{kind:'grant';grantId:Token}|{kind:'confirmation';decisionId:Token;createdGrantId:Token|null};
type Audit = {operationId:Token;id:string;digest:string;version:string;op:Permission;target:TargetView|null;
  requestedAt:Time;finishedAt:Time|null;authority:Authority;
  outcome:'pending'|'executing'|'denied'|'succeeded'|'failed'|'unknown';code:FaultCode|null};
type L = {lease:Lease};
type ExtCommands = {
  extension_list:[None,Inventory];
  extension_enable:[{key:ExtKey;enabled:boolean},null];
  extension_open:[{session:Session;key:ExtKey},Bundle]; extension_close:[L,null];
  extension_query_logs:[L & Commands['query_logs'][0],LogPage];
  extension_request_detail:[L & {generation:number;ref:Ref},Detail];
  extension_body_chunk:[L & Omit<Commands['body_chunk'][0],'session'>,Chunk];
  extension_cancel_detail:[L & {generation:number},null];
  extension_list_configs:[L,Choice[]]; extension_read_config:[L & {target:ConfigTarget},ConfigView];
  extension_update_offer:[L,UpdateInfo|null];
  extension_start:[L & {target:StartTarget},Run]; extension_stop:[L & {run:Run},null];
  extension_restart:[L & {run:Run},Run]; extension_install:[L & {check:Token},InstallEnd];
  extension_pending:[{session:Session},Consent[]];
  extension_answer:[{session:Session;token:Token;decision:ExtDecision},null];
  extension_revoke:[{key:ExtKey;grantId:Token},null]; extension_audit:[None,Audit[]];
};
type ExtEvent = {type:'extensions-changed'|'extension-consents-changed'|'extension-audit-changed'|'extension-offer-changed'};
type ExtFault = F<'desktop_extension_package',{reason:InvalidReason;field:ExtField}>
  | F<'desktop_extension_state',{reason:'io'|'corrupt'|'too-new'}>
  | F<'desktop_extension_denied'|'desktop_extension_runtime'|'desktop_extension_limit',None>;
```

`extension_list` returns discovered packages even with inaccessible, corrupt or too-new authority state: all disabled, no grants, typed `stateFault` alongside; otherwise `stateFault:null`. The enable dialog uses only the cleaned name, id, version, kind, digest and declared permissions; tell users that enabling trusts this code with page-visible data, including requested body chunks. The trusted page confirms, then calls `extension_enable`; no second Rust consent for enable. `extension_list` validates but never mounts (safe id/digest sorting; grants by time then id). `extension_enable` compare-and-sets current observed key (same state no-op); no grant implied. Refuse enabling scripts on an unqualified runtime; on downgrade show previously enabled scripts as inactive and never open them. `extension_open` returns bytes only on current attachment, enabled digest, and verified containment for scripts, rejects occupied script slot; theme is sanitized data with no lease. `extension_close` idempotently retires that lease, never closes another. `extension_pending` returns outstanding only, oldest first; `extension_audit` newest first (ties by insertion order); re-read after `resync`. Neither provides any action retrieval by public id. `extension_revoke` takes the exact current key; no bulk or cross-package selector. The four stale events go **only** to existing acknowledged channel; initial resync suffices. Consents/audit/offer read commands do not generate notices unless state changed. `extension-offer-changed` also fires on invalidation/consumption, not just check success. Corrupt or newer state recovery is explicitly **out-of-app**: quit, preserve the private file for diagnosis and intentionally move it aside before reopening; this resets all enablements/grants, not the packages. No new reset or delete RPC.

All commands have `desktop_bad_input` for invalid shape and `desktop_unavailable` for unavailable service; compare mismatches/stale tokens `desktop_changed`; exhausted active slots `desktop_busy`; old selections `desktop_cancelled`; cursor/history/ops propagate only W5b's appropriate Fault. `desktop_extension_package` validation, `desktop_extension_state` private storage unavailable, `desktop_extension_denied` no authorization, `desktop_extension_runtime` script containment unavailable, `desktop_extension_limit` byte budget exceeded. `extensions-changed` includes grant changes; UI must refresh a row even if its digest did not change. Gate generated codes with the code table/`message!`, regenerate only `docs/error-codes.md`. No Rust-localized page sentences.

Each literal name in `ExtCommands` goes in the bridge group of `aproxy-desktop/src/command_list.rs:4`; it feeds (1) app manifest via `build.rs`, (4) `generate_handler!` via `src/lib.rs`; (2) commit exactly its allow/deny pair in `permissions/autogenerated/<name>.toml`; (3) allow exactly `allow-<name with hyphens>` once in `capabilities/main.json`. Typed functions in `src/bridge.rs`/its Rust modules, undecoded `Request` boundary like existing functions. Do not add core grants, `remote`, dynamic ACL, raw IPC or frontend storage permissions. `build.rs:17,103` checks the sets **before regeneration**. Pure DTO/selection/state transitions under `aproxy-ipc`'s desktop feature; filesystem, secrets, exact preparation and injected `DesktopOps` in main crate; Tauri owns only window/attachment lifecycle, channel, transport and policy. No Tauri dependency in headless tests.

## 8. Verification specification

**Implementation and evidence.** `src/desktop/extensions/{tree,format,authority}.rs` own the handle walk, parsed projections and serialized authorization transactions. Pending descriptors are the non-redeemable pending rows in the audit array, not a duplicate array on disk. `aproxy-desktop/src/bridge.rs` owns the page and lease read areas. On Windows, `extension_frame.rs` installs a source-kinds-aware document resource filter in addition to global/per-frame navigation cancellation. Native capability callbacks fail closed; the top-document sender never resubmits an attributed side effect after an ambiguous transport failure. The build embeds an independent copy of the exact verified shell, or an empty asset and no hashes. No native version or engine probe alone enables scripts. For the currently exercised headless vectors see `tests/desktop_extensions.rs`; the installer is a counted slot plus a compiled local fake child, while instance actions exercise a real disposable worker. `tokio` virtual time exercises expiry. These are not observations of WebView2, engine scheduling or an installed frontend. The matrix below includes the separate integration/engine qualification checks, not claims that all have already run.

**Headless Rust.** Small, handwritten fixtures under `tests/fixtures/desktop-extensions/`: `violet-desk/` (one page that reads snapshot on request plus asset), `little-orb/` (mascot module with render/icon/css), `paper-lilac/` (theme string). One invalid package per §1 issue reason, plus table cases for *each known manifest field and kind restriction*. Ship literal manifest/theme/module failures; materialize filesystem-specific invalid packages from those fixtures in `common::TestHome` (e.g. many files, over-cap by one, invalid name bytes, FIFO, symlink, NTFS junction/other reparse, hardlink). No symlinks or device entries committed to Git, no downloaded assets. For root overflow, multiple individually valid packages comprise the invalid root fixture. Prove each negative by expected **code and field** and absence of bytes/enablement; a known-good sibling remains valid except on root-wide failure. Check exact limits at N and N+1; redacted UI labels and error payloads do not reflect a planted canary. Compare theme canonical semantics against literal `parseTheme` good/bad vectors, including UTF-16 name lengths, unknown fields, rounding, hostile CSS/color strings. A runtime-invalid export is a valid package that fails at mount, not a manifest parse failure.

* POSIX: raw path components, case-only collisions and device names even on case-sensitive FS, symlink in each position, nonregular file cannot block, multi-link, pre-open/post-enumerate replacement, file growing/truncating. Windows: drive/UNC/device/ADS/trailing-dot/case/short-name aliases, junction at root/parent/leaf and regular-file reparse, long vs short spelling bound to the *same* file. Test no outside canary byte returned, not path text. Test injected handle metadata/error cases **in addition to** real platform fixtures. Report if privileges/FS prevent a case; cross-clippy is not a runtime test.
* Deterministic digest/reference vector (including byte order/lengths), manifest-only/asset-only/same-version changes, half-copy during scan, missing/overflowed root, removal/reinstall, clean vs corrupt/too-new private state, private mode/ACL, atomic replacement and concurrent update without lost grants. Enable only after explicit action; known-secret collision/unknown projection refuses. Fresh content rechecked between listing→enable→open→ask→answer. Repeated scans stable, no unbounded allocation from hostile depth/size/count.
* State machine with injected monotonic/wall clocks, randomness and a counting `DesktopOps` executor: no pending action reaches executor; distinguish missing declaration, refusal, once vs remembered, revocation, removal, timeout, duplicate/cross-window/cross-lease/stale token, two simultaneous requests, capacity boundary, races of answer vs close/revoke/timeout, attach replacement, close-hidden-for-install. Advance fake time instead of sleeping. Change config/desired spec, reuse port with a different run, replace update offer/binary while dialog is open: assert **zero** invocations on wrong target. The old allowed event cannot be re-used; config, force and remembered install never execute. Executing action records exactly one terminal/unknown outcome, never retries. Simulated crash before/after admission, state save failures and audit-ring eviction. No approval/lease token or raw secrets in persisted JSON, events, extension replies or Fault details.
* Projection tests: independent page/lease choice nonce, cursors and detail generations, one chunk in flight, canceled late result not delivered, secret split across chunk boundaries, unsupported source redaction, config/URL/headers redacted at source. Separate audit/consent events collapse to resync safely. Snapshot fence and close don't strand check/install workers. No socket per extension. Fake installed child in a temporary home for install, no downloads. Unique local listeners, ports from the test harness; never assume a fixed port is free.
* Four-place registration: retain the exact-set `build.rs` check; headless test compares expected `ExtCommands` names to manifest source list and parsed committed allow/deny + capability (no extra entry/remote scope); assert handler and manifest actually expand the same list. Build default desktop variant to exercise it (no Tauri load in headless tests). Negative test with missing, duplicate and overbroad grants on in-memory fixture input. For host implementation the lead separately tests exhaustive typed mapping, denied methods never call an unattributed handler, stale/rate-limited ports, correct known/unknown Fault mapping and dialog focus. Step 1 modifies none of these tests.

**Release checks needing a real window — lead, after integrating the host.** Use a disposable user profile/home and production embedded assets on Windows; record actual WebView2 and OS build. On macOS/Linux check themes operate and scripts are listed but cannot be enabled. Do not use an installed production home, real API, Internet target, user's instances or system clipboard secrets. Use only harmless fixture requests and an isolated local observer. Never infer dispatch from a JS promise or lack of reply.

1. Start a test harness with a new `common::TestHome` and synthetic redacted history. Verify **parent** `about`, channel attach + frame/ack, native shell and known harmless page action succeed. Enable native **dispatch counters** in a test build/observer for the same innocuous read and a no-effect write probe. Record parent control count, reset. Observe local network requests/destinations **before dispatch**, incl DNS/UDP in isolated test networking; a trusted positive control must be observed. No external DNS/STUN.
2. Copy the three fixture packages; verify discovery without code execution or prompts; enable and apply theme, then switch it off and verify fallback. Enable mascot, observe light/dark, locale, reduced motion, render/icon/replay inside the sandbox. Enable/open page, read snapshot and one explicitly requested chunk; check canaries. Verify package exception/bad export only removes its frame and main navigation remains functional. User can always disable the package from trusted UI.
3. Inspect response/attributes for main and child independently. In the diagnostic fixture exercise same-engine normal read-only invoke, supported native transport and channel path directed at test observers, parent DOM/storage access, global messages and stale/cross-frame port messages. Check **no child native dispatch** (including no effect on counters/state); parent still invokes; the only data reply came via the lease's port. No need for a malicious payload or production mutation. Inspect that answer tokens never appear on the frame side. Navigate/reload/remove while awaiting a read and a consent; no port reassignment, old answer rejected. Ensure direct top-level bootstrap navigation, new windows and external protocols are denied.
4. Run the local egress matrix in §3 with a unique benign marker per mechanism, including real user-clicked links/forms/download/clipboard, self/document redirects, CSS/font/media, hint lookups and RTC data channel with local test ICE endpoints. Observe actual outbound attempts at the network observer, not just CORS/console errors; distinguish the mandatory blocked transports/navigation from the residual DNS/speculation/ICE surfaces; check no top navigation, separate window, file download, clipboard read/change, or shared app storage. Check permission prompts do not escape to a native allow dialog. Record unsupported/unobservable paths as **unverified**, not passed. Recheck after renderer or key-injection changes. Record residual exposure separately from the required native hooks.
5. Open two different safe test requests; focus defaults to deny, exact Rust targets and remembered scope visible, no package HTML. Deny first (no effect), allow second once (one effect), repeat and choose remember for an instance verb, revoke in settings and ensure next action asks again. Change only asset bytes without changing version while asking: frame retires, pending settles, grants disappear; reinstall requires enable. Close window with prompt pending, reopen and verify denied audit; separate controlled installer fixture must always ask, share page progress and finish without being relaunched after window teardown. Inspect audit for allowed-by/outcome, no secrets or reusable tokens. Repeat with a slow/stalled port; page continues to receive W5b events.

## 9. Implementation parts

Each part includes its negative and positive tests and ends at Gate L in **one independently buildable commit**. Any split commit must pass the same gate. Never expose a partial authorization path. Run added/changed test targets and the relevant desktop build/check when handlers or ACL change. No work in `ui/` in these parts.

1. **Package reader/validator**: handle-safe snapshot, digest, complete kind/theme validation and invalid inventory codes, static fixtures and platform-specific builders. No command serves bytes. Add DTO/Fault/table/doc changes together. Gate L + affected IPC tests and new integration target `desktop_extensions`.
2. **Private authority state**: persisted enablement/grants/pending descriptors/audit, lease/selection/clock fences and atomic decision state machine; typed exact-target preparation/executor (incl installer slot) behind `DesktopOps`, fake executor and crash/expiry tests. No native extension invokes yet. Gate L + affected targets.
3. **Trusted desktop bridge**: add entire narrow command set and four registrations together, scan/invalidation and lifetime wiring, separate choice/history/read slots, check/close/install semantics, headless contract tests. No script bundle on an unqualified runtime. Gate L + bridge/ACL/default-feature checks. Theme can operate without script support.
4. **Windows/WebView2 renderer boundary**: fixed shell response headers, top-document-only keyed sender, native navigation/egress/permission gates and explicit support reporting. Reference only trusted embedded assets; if lead's bootstrap isn't built, main window still builds/works, scripted open refuses. Run Gate L + desktop build. Qualification in the real window is a separate lead deliverable and recorded as such (not silently enabled on untested systems). Reconcile design with built result; run Gate F sequentially: Gate L → headless full suite → desktop full suite → desktop all-target clippy → Linux cross-clippy. Then update progress/memory in their prescribed commit with actual counts including ignored and any unverified engine; no push or tag.

## 10. Decisions and alternatives

The following are rulings for this release. No changes to a frozen layout, CLI exit status/document, W5b command signature, install args or daemon wire are necessary.

|Decision|Ruling and cost of the alternative|
|---|---|
|Delivery and versioning|Copy directories, seven-field v1 JSON, exact two-level allowlist, bundled module plus inert assets, strict caps in §1. A CLI adds another public contract; HTTP distribution adds provenance/update risk. Arbitrary HTML/relative module graphs require a secure path and MIME server. A small package may need prebundling.|
|Identity/lifecycle|Hash all bytes with SHA-256, scan on attach/refresh and authority transitions; coalesced ticks inspect metadata only, pin bytes at open, disable on observed change; target interval 5 s between normal scans (not a deadline under load). Id+version is cheaper but lets different code reuse grants. Manual-only scanning keeps removed code live longer. Retire on global overflow rather than admit an arbitrary subset.|
|Renderer and engine support|Fixed shipped shell/command bytes, script slot for the one visible untrusted frame; built-in fallback while package page is visible. Multiple untrusted windows allow inter-frame collaboration/resource growth; custom scheme/`srcdoc` tradeoffs in §3. Shipping on engines without evidence would turn CSP/ACL assumptions into product guarantees. Require the compiled Windows qualification switch, verified shell and installed native hooks; other engines support theme-only execution. Speculation, DNS and RTC are residual risks. Stronger cross-engine guarantees may require separately isolated renderer/networking, a separate scope decision.|
|Platform dependencies and sender|Prefer existing SHA-256, OS randomness, SemVer, serde, libc and Windows APIs. For native frame/permission hooks only the already locked Windows `webview2-com` and `windows` bindings are approved. No macOS/Linux hooks or new binding versions. Guard Windows keyed transport at installation; solely hiding JS globals is cheaper but not a boundary. Any other new direct dependency needs separate ruling.|
|Reads and capacity|Shared W5b projections, private lease cursors/choice/selection, explicitly requested body chunks, allow reads for both scripted kinds, not a new permission tier. Cached page-initiated update offer vs letting a read rotate offers and contact the network. Strict port limits and no renderer CPU guarantee; separate OS process if that stronger promise is required.|
|Writes / grants|Separate required-lease commands rather than optional attribution on old commands (keep all released shapes). Remember one home-wide **instance verb**, not all control operations; exact-run grants limit impact but expire on restart and are rarely reusable. One-shot for install; no config writes, alternatives and costs in §5.|
|Consent, state, audit|Two bounded requests, monotonic 60 s, single-use session-bound random answers, exact typed Rust targets, no answer secrets on disk, pending diagnostic descriptors only. Durable replayable pending actions survive crashes at the cost of executing without the original window/user. One <=256 KiB file/atomic state transition plus bounded 128-row audit rather than unbounded forensic history; intentional manual reset for unknown/corrupt schema avoids inventing recovery authority.|
|User-visible text and side channels|Strip controls, identify publisher claims as untrusted, recheck secrets, display fixed code/field. No arbitrary error message, persisted raw target or OS-localized diagnostic. A richer marketplace/info view requires additional hostile text/assets. Content the user voluntarily shares and renderer resource exhaustion remain outside the confidentiality/availability claim.|
