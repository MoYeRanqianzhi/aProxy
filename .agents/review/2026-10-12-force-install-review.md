# 强制安装与进程台账审查（alpha.1 第 2 步，2026-10-12）

> 只读审查：除本文件外未改动仓库任何文件，未提交；没有结束任何 aproxy 进程，没有读写用户真实的 home。
> 范围：提交 2f1df42、3c2d4b1、c6d3b43、a93f8dc、72fba9d、d2e2662（夹在中间的 a10f116、c5c85ac、bdf502f 是文档，不在范围），逐个 `git show` 读过。对照规格：`.agents/plan/release-0.2.0.md` 的「W2b 强制安装：永久兼容契约」一节（C1–C8、G4）与实现者自报的 8 条偏差。
> 行号：一律以 d2e2662 为准。src/ledger.rs、src/install/swap.rs、src/install/mod.rs、src/settings.rs、tests/force_contract.rs 至今未变；src/install/force.rs 在审查期间落地的 bc37209 里有改动，当前 HEAD 的行号与这里不同。审查时工作区另有未提交的改动（src/install/force.rs、src/install/flow.rs、src/install/mod.rs、tests/install_flow.rs、tests/install_force.rs），不在范围。
> 运行验证（仅一次）：用现成的 `target/debug/aproxy.exe`（2026-10-12 08:46 构建，`--version` 为 `aproxy v0.2.0-alpha.0 (desktop; x86_64-pc-windows-msvc)`；构建晚于审查范围，但所走的输出代码与 d2e2662 相同），APROXY_HOME 设为临时目录，执行 `install --force --from <不存在的文件> --yes`。标准输出与标准错误接到一个立即退出的管道（`2>&1 | true`）时，3 次全部以 101 退出，各留下一个 `install/force/<pid>/`（台账条目已在 panic 展开时删除）；不接管道时以 10 退出，本次的工作目录被删掉（install/ 与 force/ 因前三次留下的目录而保留）。临时目录事后已删除。没有编译，没有跑 cargo test；unix 代码是逐行读的，没有运行。
> 记号：已确认 = 读代码、读测试或实测坐实；可能 = 推理得出、未经运行验证。
> 级别：P0 = alpha.1 发布前必须修；P1 = 应该修；P2 = 小问题或措辞。

## 总评

安全核心是对的。只按台账结束进程；Windows 在同一个句柄上核验后再结束；Linux 先用 pidfd 钉住进程再核验、再发信号；跳过自己的 pid；从不按名称结束；不写 home 之外的东西。没有找到必须挡住 alpha.1 的 P0。问题集中在三处：冻结项的定义和测试钉得不够死；冻结的读者过严，以后无法真正「只增」；输出管道断开时退出码越出契约。

## P0

无。

## P1

**1. 输出管道断开时 panic，退出码 101 越出冻结集合**（已确认，实测）
- 位置：src/install/force.rs 中所有 `println!`/`eprintln!`/`print!`，例如 907、918、598、833。restore 子进程继承的是同一个 stdout。
- 场景：
  - `aproxy install --force --yes | head -5`；
  - agent 的工具超时后只杀掉了 shell；
  - 将来桌面界面用管道读输出，然后按规格先退出。
- 后果：在第 1 步失败时，实测得到 101 而不是 10，并留下 `install/force/<pid>/`，「10 = 什么都没动」不成立。同样的 panic 如果发生在第 3 步「已结束 …」之后，结果是进程已停、二进制没换、实例没拉回，调用方却拿到 101；接了 `| head` 的话，shell 报的甚至是 0。
- 修法：强制安装的输出统一改用忽略写错误的写法（`let _ = writeln!(io::stdout(), …)`）。W5 计划里写明：界面拉起强制安装时 stdio 一律为 null 或 detached。

**2. aproxy-cli skill 的 description 超过 1024 字符**（已确认，计数）
- 位置：.claude/skills/aproxy-cli/SKILL.md:3。72fba9d 之前 962 字符，之后 1029 字符，全是 ASCII，审查时的 HEAD（bc37209）仍是 1029。
- 后果：Agent Skills 规范和 skill-creator 的校验上限都是 1024。按上限校验的客户端会拒收或截断，随发布出去的 skill 可能整份加载失败。
- 修法：删掉一个例子，或者压短新加的那句 install --force。

**3. C4 的锁原语没有冻结，依赖 std 没有承诺的实现**（已确认：读代码与 std 文档）
- 位置：src/install/mod.rs:63-96。
- 缺陷：std 文档对 `try_lock` 的原话是「currently corresponds to the flock function on Unix … LockFileEx on Windows … Note that, this may change in the future」，Windows 上锁住的字节范围也没有写明。
- 场景：某天工具链把 unix 实现换成 fcntl/OFD 锁（在 Linux 上它和 flock 互不排斥），或者 Windows 换了锁的范围。两个版本在 C4 上互相看不见，会同时修改 bin/。
- 另一个问题：`open` 在 75-77 行，位于重试循环之外。共享模式不兼容（别的版本或杀毒软件占着文件）时会立即失败，而不是等待。
- 修法：
  - 契约写死具体原语：unix 用 `flock(LOCK_EX|LOCK_NB)`；Windows 用 `LockFileEx(EXCLUSIVE|FAIL_IMMEDIATELY)`，offset 0、长度 u64::MAX，打开时共享模式为 READ|WRITE、不带 DELETE。
  - 用 libc 和 windows-sys 直接调用。
  - tests/force_contract.rs 加一个交叉测试：用原生 flock 或 LockFileEx 持锁，断言 `lock_install_in` 拿不到锁；反过来也测一遍。
  - 把 `open` 挪进重试循环。

**4. 冻结的读者过严，以后「只增」会让所有旧版本的强制安装失败**（已确认：读代码）
- 版本行（src/install/force.rs:144-156）：`VersionLine::parse` 要求版本号后面正好是 `(...)`。以后某个版本在 `)` 后追加任何内容（例如 ` [lts]`），旧版本读到的 variant 就是 None。在线路径（535）和 `--variant` 路径（306-307）都会判成「变体不符」，退出 10。
- 发布清单（src/install/force.rs:180-202）：assets 的每个元素都要求五个字段齐全，任意一个元素缺字段，serde 就让整份清单解析失败。以后清单里加一种非二进制资产，所有旧版本都读不了清单。另外不容忍 BOM。
- 修法：
  - 现在就放宽读者：版本行只取版本号后的第一个括号组，忽略其后内容；
  - assets 先按 `Vec<Value>` 读，逐个元素解析，坏元素跳过；
  - 去掉 BOM；
  - 同时在 C5 和 C6 里写明对写者的约束。

**5. C1 的 home 解析既没冻结，也没被契约测试钉住**（已确认：读代码；跨版本后果是推理）
- 位置：src/settings.rs:16-30。tests/force_contract.rs 里没有任何 C1 断言。
- 缺陷：
  - `std::env::var("APROXY_HOME").ok()` 遇到非 UTF-8 的值，会静默退回 `~/.aproxy`；
  - 值会被 trim；
  - 相对路径原样使用，不同工作目录会解析出不同的 home；
  - 「用户目录」取的是 `dirs::home_dir()`。
- 场景：将来某个版本把它改成 var_os，或者改用 `std::env::home_dir`。同一环境下新旧版本就会解析出不同的 home：旧版本的强制安装会去结束另一个 home（默认的 ~/.aproxy）里的进程，而真正要修的 home 一个进程都没停。
- 修法：在 C1 写死完整算法（建议用 var_os，空值视为未设，相对路径拒绝），并在契约测试里钉住。

**6. 契约测试对推导方式的约束力不足**（已确认：读测试）

下面每一项都是一个具体改动，现有测试能放它通过：

| 冻结项 | 能逃过测试的改动 | 修法 |
|---|---|---|
| C3 Windows（tests/force_contract.rs:216 只验 `w:` 加数字） | 把 FILETIME 换成 Unix 毫秒，或高低位颠倒 | 测试里独立调用 `GetProcessTimes(GetCurrentProcess)` 拼出期望值比对 |
| C3 Linux（tests/force_contract.rs:220-226） | 把第 22 字段换成第 21 字段，或除以 CLK_TCK | 测试里独立解析 `/proc/self/stat` 的第 22 字段比对 |
| C5（tests/force_contract.rs:289-302 跑 `--version` 时不设 APROXY_HOME，也不查退出码） | 退出码改成 1，或在输出前读写 home 都能通过；而一旦 `--version` 有副作用，这个测试会碰到开发者真实的 `~/.aproxy` | 设临时 home，断言目录仍为空、退出码为 0 |
| C2（tests/force_contract.rs:97-98 只验时间戳是 u64） | 改成微秒能通过，但 aproxy.bat 靠 19 位纳秒按字典序跨版本挑最新的 `aproxy.old-*` | 断言 19 位十进制 |
| C7 | `restore` 子命令被改名时，契约测试照样通过 | 在空的临时 home 里跑 `restore`，断言退出 0 |
| C8「先结束安装者，再拿锁」 | 改成先拿锁再结束，全部测试仍然通过（测试里的安装者是 sleeper，不持锁） | 加一个持锁的台账安装者用例 |

**7. skill 和文档没有提醒 agent 给足超时、不要截断输出**（可能，推理）
- 位置：.claude/skills/aproxy-cli/references/latest/commands.md 的 Force install 一节；docs/INSTALL_AGENT.md。
- 场景：agent 用默认 2 分钟的工具超时执行 `--force --yes`。第 1 步慢速下载（客户端总超时 120 s），加上第 6 步（30 s + 每个实例 10 s + 15 s），总时长可以超过 2 分钟。如果工具超时在第 3–6 步之间杀掉进程树，实例已停、没有拉回，agent 自己经 aproxy 的会话也随之断开，无法再向用户报告。
- 修法：skill 里写明三点：给这条命令至少 10 分钟超时；不要接 head 之类的截断管道；跑完看退出码。

## P2

8. AlreadyGone 删条目时可能删掉同 pid 的新条目（可能）。位置 src/install/force.rs:599、603-605。一轮开始时 `read_all_in` 读到 pid P 的陈旧条目；前面每个进程的结束最多等 10 s，轮到它时 Windows 可能已把 P 分给一个新 aproxy 进程并写了新条目。核验发现标记不符，删掉的却是新条目，这个进程从此在台账里不可见。修法：删除前重读文件，`start` 一致才删。
9. 读者会删掉别的平台、别的 pid 命名空间写的条目（已确认）。位置 src/install/force.rs:603-605（AlreadyGone 分支删条目）；src/ledger.rs:25-27（文件头写明的读者义务）。WSL 与 Windows 共用 home，或容器与宿主共用 home 时，一方会把另一方的条目当成「pid 已被复用」删掉，`<pid>.json` 也会互相覆盖。修法：start 前缀不属于本平台的条目，既不结束也不删除。
10. 第 6 步只按端口判断「回来了」，不看版本（可能）。位置 src/install/force.rs:714-721。同一端口上若有一个用旧二进制起来的实例，会被当成已恢复，返回 0。修法：同时要求 `version` 等于目标版本；端口对不上时再按配置路径匹配。
11. 退出 11 时可能一个进程都没停（已确认）。位置 src/install/force.rs:1043-1049（失败输出在 1026-1038）。第一个安装者结束不了，或锁等满 30 s，都会这样。文档表格应改成「已开始结束进程」。
12. Linux 上结束普通安装者会留下 /dev/shm 里的安装宣告（可能）。依据 src/install/announce.rs:106-117 只在 Drop 时 unlink、src/watchdog.rs:872-889 每轮都检查（调用点 774）、src/install/flow.rs:712-713 没有 state 时直接返回。之后看护者每 30 s 拉起一个短命的 `install --continue`，直到重启。修法：强制安装结束安装者后，清掉本 home 已过期的宣告。
13. 第 4 步应先删 state.json（可能）。位置 src/install/force.rs:629-667。被中断时如果 state.json 还在，下一条任意 aproxy 命令会拉起续作，接着推进那个旧的普通安装。
14. 交换落位后的核对没有时限，而且是在持锁时做的（已确认）。位置 src/install/swap.rs:179 用的是不带时限的 `probe_version`；强制安装自己的 `probe` 有 60 s 时限，这里却没用上。
15. 新文件改名到位前不做 fsync（可能；旧代码就有，不是回归）。位置 src/install/swap.rs:196-223。交换刚完成几秒内断电，可能留下内容全零的 aproxy(.exe)，此时 exe 在场，aproxy.bat 也不会兜底。
16. 下载相关（超时已确认，其余可能）。位置 src/install/download/mod.rs:281、src/install/force.rs:434-438。总超时 120 s，慢链路上下载带界面的版本会失败；没有限定只走 HTTPS；清单和资产都没有大小上限。
17. latest 只认带清单的 release，会静默降级（读代码确认，场景是推理）。位置 src/install/force.rs:384-399（判据在 src/install/download/github.rs:131-137）。新版本漏传清单时，`--force --yes` 会装回更旧的版本而不提示。修法：latest 解析出的版本低于正在运行的版本时退出 10。
18. home 不存在时照样「成功」（已确认）。位置 src/install/force.rs:879-895。APROXY_HOME 写错时，二进制被装进一个新目录，退出 0。
19. 提权运行的强制安装信任用户可写的台账（可能）。位置 src/ledger.rs:429-474（提示语在 440-446）。退出 11 的提示还会建议用户提权重跑；同一用户的低权限进程可以写一条带正确启动标记的条目，借提权的强制安装结束别的进程。修法：结束前核对进程属主与台账文件属主。
20. Windows 上台账写入和删除不重试（可能）。位置 src/ledger.rs:226-246。目标文件被不带 FILE_SHARE_DELETE 的句柄占着时，这个进程就不进台账。
21. 第 7 步的报告措辞与覆盖面（已确认）。位置 src/install/force.rs:814-821：凡是从 bin 运行、不在台账里的进程都被说成「多半是 0.1.x 留下的」，放在 bin 里的 aproxy-format 也会被这样描述。macOS 上这一步结果恒为空，文档没写。另外 733-750 没有解析镜像所在目录，8.3 短名下会漏报——审查时工作区里未提交的 `file_is_within` 正在修这一点。
22. unix 续作重入会把 old_path 改成新二进制（已确认；旧代码同样如此，不是回归）。位置 src/install/swap.rs:148-158 与 src/install/flow.rs:933-936。在 rename 之后、推进到 Swapped 之前崩溃，续作重跑时会把新版本记成「旧二进制」，之后的回滚拉起的仍是新版本。
23. 持锁等待在 async 里用 `thread::sleep`（可能）。位置 src/install/mod.rs:94，由 src/install/flow.rs:599、928 调用，最多阻塞 30 s。
24. 被另一个强制安装结束时，退出码是 1（src/ledger.rs:452）或 137，不在 0/10/11/12 之内，文档没说明。
25. 杂项：
    - `--variant` 与 `--from` 同用时是核对，不是选择，文档写成了选择（.claude/skills/aproxy-cli/references/latest/commands.md 的选项表）；
    - 确认提示打到 stdout（src/install/force.rs:833），stdout 被重定向而 stdin 是终端时，用户看不到提示就卡住；
    - 在第 1–2 步按 Ctrl+C，会留下已下载的二进制（`install/force/<pid>/`，src/install/force.rs:879-903 建的工作目录）。

## 对 8 条自报偏差的判断

1. **MoveFileExW(flags=0) 配 create_new**：正确，而且必要。
2. **`--variant` 改名 `--cpu`**：合理。但契约测试已经钉住了 `--cpu`、`--pre`、`--download-proxy`（tests/force_contract.rs:396-439），规格 C7 的正文要同步列出。
3. **退出码 12 只按端口判断**：可以接受，但应加上版本校验（见第 10 条）。
4. **两个强制安装互相结束**：冻结语义决定了这一点，接受；文档补上退出码 1/137。还有一个推测的情况：被结束的那个如果正处在第 6 步，它拉起的 restore 子进程不在台账里，会继续运行。
5. **工作目录 `install/force/<pid>/`**：合理。
6. **`--from` 执行两次 `--version`**：合理。
7. **macOS 第 7 步未实现**：alpha 可以接受，文档要写明。
8. **`--no-skills`、`--allow-downgrade` 被接受但不起作用**：要么现在就拒绝，要么在契约测试里钉住；现状是「接受但没钉」，最危险。

## 九个重点逐项结论（除上面列出的问题外）

1. **安全**：Windows 的竞态由同一句柄堵住。Linux 的 pidfd 流程经推敲是对的：核验时读到的若是复用这个 pid 的新进程，标记必然对不上，不会发信号。ESRCH、EPERM、ENOSYS 都处理了。pid 0 和负数被挡住。不按名称结束进程。目前没有任何 aproxy 进程会拉起 `--force`，所以「结束自己的父进程链」的风险只在将来的桌面界面（见第 1 条）。
2. **可重入**：Windows 上两次改名之间被中断后，再执行一次（经 aproxy.bat，或用下载的二进制）能收敛。
3. **Windows 语义**：先结束安装者再拿锁，加上锁文件不带 FILE_SHARE_DELETE，没有死锁；Windows 的安装宣告随进程消失，不会形成活锁；改名失败的回滚是正确的。
4. **台账生命周期**：实例在绑定端口前、看护者在拉子进程前、安装者在做任何事前登记；正常退出、panic 展开、`ledger::exit` 三条路都会删条目。0.2.0-alpha.0 的进程不写台账，但不挡安装，只在第 7 步被报告。
5. **unix 代码**：`/proc/<pid>/stat` 从最后一个 `)` 之后切分、取下标 19，正确；zombie（Z/X、SZOMB）算已退出；` (deleted)` 后缀已去掉；libc 0.2.189 在 gnu、musl、apple 各目标上都提供了所需的常量和结构体。
6. **契约测试的约束力**：见第 6 条。
7. **清单与下载**：`file` 只作同一 release 内资产名的查找键，URL 来自 GitHub API，落盘路径固定，没有路径穿越，也拼不出任意 URL；sha256 在执行之前比对；清单的 `version` 和二进制自报的版本都与 tag 核对。HTTPS 与大小上限见第 16 条。
8. **普通安装没有回归**：`aproxy.exe.new` 原来只在测试里被引用，状态机的恢复检测只看 install.state。兜底脚本（bat 与 sh）能找到最新的 `aproxy.old-*`，有测试。清残留时，Windows 上删不掉仍被占用的文件，unix 上删的只是硬链接，不影响运行中的进程。
9. **文档与 skill**：退出码、`--yes`、非交互时退出 10、会中断 agent 自己的会话、`--cpu` 改名，各处一致；其余问题见第 2、7、11、21、25 条。

## 规格（C1–C8）本身的缺陷

- **C1**：没有定义非 UTF-8、首尾空白、相对路径以及「用户目录」的取法，实现与「绝对路径」的表述也不一致。
- **C3**：
  - 「标记对不上就删条目」与「共用 home 只会拒绝、不会误杀」的说法冲突；`<pid>.json` 跨系统、跨 pid 命名空间会撞名。冻结前要决定：声明不支持共用，或在文件名里带上平台前缀。
  - 缺少「删除前复核」的读者义务。
  - 缺少属主约束。
  - Windows 进程创建时间的实际精度是时钟滴答，同一 pid 在同一滴答内被复用理论上可能，需要确认可以接受。
- **C4**：没写锁原语、字节范围和共享模式。Linux 的安装宣告放在 /dev/shm，不在 `<home>/install/` 下，与「一切安装者状态只放在 install/」不符，强制安装也清不到它。
- **C5**：没写「首行在 `)` 之后不得追加任何内容」，也没要求读者容忍追加。
- **C6**：没写以下几点：
  - `file` 永远是裸可执行文件，每个 release 都要为每个目标保留旧版本能运行的裸二进制条目；
  - assets 的每个元素都带齐五个字段，或者读者跳过坏元素；
  - 清单是不带 BOM 的 UTF-8；
  - latest 遇到缺清单的 release 时的降级行为。

  另外，在线路径依赖 GitHub API 的形状和仓库名。规格应写明 `--from` 才是永久保证的路径，在线路径是尽力而为。
- **C7**：正文与契约测试不一致（见自报偏差第 2、8 条）；没有规定「写输出失败不得改变退出码」；被另一个强制安装结束时的退出码没有约定。
- **C8**：
  - 第 4 步应先删 state.json；
  - 第 3 步「连续两轮、间隔 500 ms」只是经验值，应补上「第 6 步按台账核对回来的实例版本」作为兜底；
  - 第 6 步依赖不冻结的 `.restore` 跨版本可读，所以退出 12 会比预期常见。建议现在就给实例条目追加一个机器可读的可选字段（例如 `config`，配置文件的绝对路径；`label` 是自由文本，不能依赖），让任何版本都能准确告诉用户怎样拉回实例。
