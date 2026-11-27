//! 安装命令对终端或事件首行给出的说明。
use aproxy_ipc::message;
message!(failed(cause: &str), en: "Installation failed: {cause}", zh: "安装失败: {cause}");
message!(correct(), en: "Correct the problems above and run aproxy install again.", zh: "修正上面的问题后重新执行 aproxy install。");

message!(skill_version(version: &str), en: "Skills describe the current binary. Only documentation for {version} can be installed. Upgrade aProxy before installing newer documentation.", zh: "skill 描述的是当前二进制，只能安装 {version} 的文档。要使用新版文档，请先升级 aProxy。");

message!(checking(channel: &str), en: "Checking the latest version in the {channel} channel...", zh: "查询最新版本（{channel} 通道）...");

message!(empty_channel(), en: "no release in this channel", zh: "通道内无版本");

message!(npm_fallback(gh_err: &str, version: &str), en: "Could not query GitHub ({gh_err}); latest version on npm: {version}", zh: "查询 GitHub 失败（{gh_err}），改查 npm 的最新版本：{version}");

message!(proxy_hint(), en: "Use --download-proxy <URL> to try another download proxy.", zh: "可尝试：--download-proxy <URL> 更换出口。");

message!(available(latest: &str, current: &semver::Version, label: &str), en: "Version {latest} is available (current: {current}, channel: {label}). Run aproxy install to upgrade.", zh: "有新版本 {latest}（当前 {current}，{label} 通道）。执行 aproxy install 升级。");

message!(latest(current: &semver::Version, label: &str, latest: &str), en: "Up to date: current version {current}, latest in {label}: {latest}.", zh: "已是最新：当前 {current}，{label} 通道最新为 {latest}。");

message!(empty(label: &str, current: &semver::Version), en: "No release is available in the {label} channel. Current version: {current}.", zh: "{label} 通道暂无可用版本，当前 {current}。");

message!(use_pre(), en: "Use --pre to include pre-releases.", zh: "（如需预发布版本，可加 --pre）");

message!(explicit_hint(), en: "Run aproxy install <version> to skip the latest-version lookup, or retry with aproxy install --download-proxy <URL>.", zh: "可尝试：aproxy install <具体版本号>（跳过 latest 查询），或 aproxy install --download-proxy <URL> 更换出口。");

message!(no_install(current: &semver::Version, label: &str, latest: &str), en: "Up to date: current version {current}, latest in {label}: {latest}. No installation needed.", zh: "已是最新：当前 {current}，{label} 通道最新为 {latest}，无需安装。");

message!(keep(label: &str, variant: &str, current: &semver::Version), en: "No {variant} release is available in the {label} channel. Keeping {current}.", zh: "{label} 通道暂无可用的 {variant} 版本，保持当前 {current} 不变。");

message!(download(target: &str, variant: &str, chain: &str), en: "Downloading aProxy {target} ({variant}; sources: {chain})...", zh: "开始下载 aProxy {target}（{variant}；渠道链：{chain}）...");

message!(next(previous: &str, asset: &str), en: "{previous}. Trying {asset}...", zh: "{previous}，改用 {asset}...");

message!(unavailable(), en: "The default variant includes the desktop interface. If this release has no desktop build for this system, use --variant headless. Alternatively, aproxy install --force checks the release manifest and chooses headless if no desktop build exists. It stops the current user's aProxy processes in this home and interrupts requests in progress through them.", zh: "没写 --variant，要装的是带桌面界面的版本。若这一版没有为本机发布它，可加 --variant headless 改装无界面版本；或者用 aproxy install --force（会先结束这个主目录下当前用户的全部 aproxy 进程、中断经过它们的在途请求），它按发布清单判断，确实没有带界面的版本时改装 headless。");

message!(loader(), en: "The desktop build cannot run on this system. See the dynamic loader error above. Check system libraries such as WebKitGTK and glibc. To install without the desktop interface, run aproxy install --variant headless.", zh: "带桌面界面的版本在本机起不来：动态加载器的报错见上，多半是缺 WebKitGTK 等系统库、这些库太旧，或者 glibc 太旧。不需要窗口，就改装无界面版本：aproxy install --variant headless");

message!(chain_hint(), en: "Try --download-proxy <URL>, or configure download_chain in settings.json. URL templates can use a CDN; see the README.", zh: "可尝试：--download-proxy <URL> 配置下载代理，或在 settings.json 配置 download_chain（含 url 模板 CDN 通道，文档见 README）。");

message!(headless(), en: "Installing the headless variant. The proxy and CLI are unchanged; the aproxy desktop window is unavailable. See the error above for the missing or outdated libraries (often WebKitGTK or glibc on Linux). After correcting it, run aproxy install --variant desktop.", zh: "改装的是无界面的 headless 版本：代理与命令行完全相同，只是没有 aproxy desktop 窗口。带界面的版本起不来的原因见上（Linux 上多半是缺 WebKitGTK 等系统库，或者 glibc 太旧），解决之后可用 aproxy install --variant desktop 换回。");

message!(install_variant(target: &str, installed_variant: &str), en: "Installing aProxy {target} ({installed_variant})...", zh: "开始安装 aProxy {target}（{installed_variant}）...");

message!(handoff(), en: "The target version has taken over the installation. Waiting while it prepares the instances, replaces the binary, restarts the instances one at a time and verifies them...", zh: "新版本进程已接手安装（让实例做好准备、替换二进制、逐个重启实例、最后核验），等待其完成...");

message!(done(path: std::path::Display<'_>), en: "Installation complete: {path}. The binary is in place and the instances are running the target version.", zh: "安装完成：{path}（二进制已落位，实例已滚动到新版本）");

message!(halted(), en: "Rolling restarts stopped and will not retry automatically. aProxy brought the failing instance back with the old binary, or kept its desired-state record if that failed. Instances restarted before it stay on the target version; the rest were not changed. Check whether the target version accepts the configuration. Correct the problem and rerun aproxy install, or reinstall the old version with aproxy install <old-version> --allow-downgrade.", zh: "滚动已中止，不会自动重试：出问题的实例已用旧二进制拉回（拉回失败时其期望状态记录已保留）；\
             在它之前已重启的实例留在目标版本，尚未轮到的实例未动。常见原因是新版本不接受现有配置——按上方原因修正后重新执行 aproxy install 完成升级；\
             若要留在旧版本，用 `aproxy install <旧版本> --allow-downgrade` 把规范位置的二进制换回。");

message!(retained(), en: "Installation state was retained. Rerun the same command to resume. The watchdog, or any command that changes instances or settings (such as start, stop, restart or restore), can also resume the installation in the background. Before the binary is replaced, use aproxy install --abort to roll back.", zh: "现场已保留：重新执行同一条命令即可接着装完；看护者，以及会改动实例或设置的 aproxy 命令\
             （start、stop、restart、restore 等），也会在后台接着装。交换二进制之前可以用 \
             `aproxy install --abort` 回滚。");

message!(install(target: &str), en: "Installing aProxy {target}...", zh: "开始安装 aProxy {target}...");

message!(no_installation(), en: "No installation is in progress.", zh: "没有进行中的安装。");

message!(aborted(), en: "Installation aborted. Temporary files were removed. The binary and instances were not changed.", zh: "安装已中止，现场已清理（二进制与实例未受影响）。");
message!(path_changed(files: &str), en: "Updated PATH for new terminals: {files}. Already open terminals keep their old PATH.", zh: "已为新终端更新 PATH：{files}。已打开的终端仍使用原 PATH。");
message!(path_present(bin: impl std::fmt::Display), en: "PATH is already configured for {bin}; nothing changed.", zh: "已为 {bin} 配置 PATH，无需更改。");
message!(path_failed(reason: &str, bin: impl std::fmt::Display), en: "Warning: PATH setup failed: {reason}. Add {bin} to your user PATH manually, then open a new terminal.", zh: "警告：PATH 配置失败：{reason}。请手动将 {bin} 加入用户 PATH，然后打开新终端。");
message!(path_skipped(), en: "Skipped PATH setup (--no-path).", zh: "已跳过 PATH 配置（--no-path）。");
