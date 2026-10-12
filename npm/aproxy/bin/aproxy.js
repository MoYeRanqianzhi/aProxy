#!/usr/bin/env node
'use strict';

// aProxy npm 入口转发器（esbuild/rollup 同款多平台包模式）。
//
// 原理：npm 主包（@meowo/aproxy / @meowo/aproxy-format）只含这个 JS 脚本；
// 真正的二进制在各平台子包里（optionalDependencies），npm install 时按
// os/cpu/libc 字段自动只装当前平台的那一个——全程 registry 内分发，无二次
// 网络下载。本脚本运行时解析出平台子包内的二进制路径，spawn 并透传参数/
// stdio/退出码。
//
// 包名自检测：wrapper 由主包 package.json 推导子包前缀（name 加 -）与二进制名（bin 的命令名）
// ——@meowo/aproxy → 子包 @meowo/aproxy-*、二进制 aproxy；@meowo/aproxy-headless →
// 子包 @meowo/aproxy-headless-*、二进制同样是 aproxy；@meowo/aproxy-format 同理。一份脚本
// 三组通用，改包名时无需同步此处。

const { spawnSync } = require('child_process');
const path = require('path');

const PKG = require('../package.json');
const SELF_NAME = Object.keys(PKG.bin)[0]; // aproxy | aproxy-format
const PKG_PREFIX = PKG.name + '-';
// 带桌面界面的那一组：Linux 上它的 gnu 二进制动态链接 GTK 与 WebKitGTK
const DESKTOP_GROUP = PKG.name === '@meowo/aproxy';

// gnu 产物能运行的 glibc 下限。必须与 .github/workflows/release.yml 的 GNU_GLIBC_FLOOR、
// scripts/install.sh 的 MIN_GLIBC 保持一致；低于它的系统改用 musl 产物。
const MIN_GLIBC = '2.28';
// 带界面的 gnu 产物能运行的 glibc 下限，必须与 release.yml 的 DESKTOP_GLIBC_FLOOR、
// scripts/install.sh 的 DESKTOP_MIN_GLIBC 保持一致。它还要 WebKitGTK 2.40+，那一项这里查不了，
// 缺库时由动态加载器报错（见文件末尾）。
const DESKTOP_MIN_GLIBC = '2.35';
const HEADLESS_HINT = [
  '改装无界面的 headless 版本（代理与命令行完全相同，只是没有 aproxy desktop 窗口）：',
  '  npm uninstall -g @meowo/aproxy && npm install -g @meowo/aproxy-headless',
  '想留着带界面的版本，就装上 WebKitGTK 2.40+（Debian / Ubuntu：sudo apt install libwebkit2gtk-4.1-0），',
  '它还需要 glibc ' + DESKTOP_MIN_GLIBC + ' 或更新。',
].join('\n');

// 平台 → 子包后缀。linux 需区分 glibc/musl（两套产物）；forceMusl 用于 glibc 过老的系统。
function platformPackage(forceMusl) {
  const { platform, arch } = process;
  if (platform === 'win32') {
    if (arch === 'x64') return PKG_PREFIX + 'windows-x64';
    if (arch === 'ia32') return PKG_PREFIX + 'windows-ia32';
    if (arch === 'arm64') return PKG_PREFIX + 'windows-arm64';
  }
  if (platform === 'darwin') {
    if (arch === 'arm64') return PKG_PREFIX + 'darwin-arm64';
    if (arch === 'x64') return PKG_PREFIX + 'darwin-x64';
  }
  if (platform === 'linux' && arch === 'x64') {
    return PKG_PREFIX + 'linux-x64' + (forceMusl ? '-musl' : '');
  }
  if (platform === 'linux' && arch === 'arm64') {
    return PKG_PREFIX + 'linux-arm64' + (forceMusl ? '-musl' : '');
  }
  return null;
}

// process.report 的 glibcVersionRuntime 只在 glibc 构建的 Node 上有值；musl 环境
// （Alpine 等）拿不到它。返回值：版本字符串（glibc）/ null（report 可用但无 glibc，
// 即 musl）/ undefined（report 不可用，无法判断，保守按 gnu 处理）。
function glibcRuntime() {
  try {
    if (!process.report || !process.report.getReport) return undefined;
    return process.report.getReport().header.glibcVersionRuntime || null;
  } catch {
    return undefined;
  }
}

// 'X.Y' 版本比较：a 是否严格低于 b
function versionLt(a, b) {
  const [a1, a2] = a.split('.').map((n) => parseInt(n, 10) || 0);
  const [b1, b2] = b.split('.').map((n) => parseInt(n, 10) || 0);
  return a1 < b1 || (a1 === b1 && a2 < b2);
}

const glibc = process.platform === 'linux' ? glibcRuntime() : undefined;
const glibcTooOld = typeof glibc === 'string' && versionLt(glibc, MIN_GLIBC);

// 带界面的 gnu 二进制在这里注定起不来（glibc 够 headless、不够带界面版）：不 spawn 一个必然在
// 动态加载阶段失败的进程，直接说清原因与改装办法。glibc 低于 MIN_GLIBC 的走下面的 musl 分支。
if (DESKTOP_GROUP && typeof glibc === 'string' && !glibcTooOld && versionLt(glibc, DESKTOP_MIN_GLIBC)) {
  console.error(
    `${SELF_NAME}: 本机 glibc ${glibc} 低于带桌面界面的版本要求的 ${DESKTOP_MIN_GLIBC}，它在这里起不来。\n` +
      HEADLESS_HINT
  );
  process.exit(1);
}

const pkg = platformPackage(glibc === null || glibcTooOld);
if (!pkg) {
  console.error(
    `${SELF_NAME}: 不支持的平台 ${process.platform}-${process.arch}。` +
      '请从 https://github.com/MoYeRanqianzhi/aProxy/releases 直接下载二进制。'
  );
  process.exit(1);
}

let bin;
try {
  const pkgDir = path.dirname(require.resolve(pkg + '/package.json'));
  bin = path.join(pkgDir, 'bin', process.platform === 'win32' ? `${SELF_NAME}.exe` : SELF_NAME);
} catch {
  if (glibcTooOld) {
    // 不 spawn 一个必然在动态加载阶段失败的 gnu 二进制（报错晦涩）：直接说清原因与补救办法。
    // npm 按 libc 字段过滤平台包，glibc 系统上默认不会装 musl 子包，所以这里通常走到。
    const forceMusl = `npm install -g ${pkg} --force（npm 默认按 libc 过滤，不会在 glibc 系统上装它）`;
    const remedies =
      SELF_NAME === 'aproxy'
        ? [
            '改用安装脚本（会自动选用静态链接的 musl 产物）：',
            '     curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.sh | sh',
            `或强制安装 musl 平台包：${forceMusl}`,
          ]
        : [`强制安装 musl 平台包：${forceMusl}`];
    console.error(
      [
        `${SELF_NAME}: 本机 glibc ${glibc} 低于 ${MIN_GLIBC}，预编译的 gnu 二进制无法运行，且未安装 musl 平台包 ${pkg}。`,
        '补救办法：',
        ...remedies.map((r) => (r.startsWith(' ') ? r : `  - ${r}`)),
        '或从 https://github.com/MoYeRanqianzhi/aProxy/releases 下载文件名含 unknown-linux-musl 的产物。',
      ].join('\n')
    );
    process.exit(1);
  }
  // optionalDependencies 被 --omit=optional 跳过、或 npm 平台过滤未装上时走到这里
  console.error(
    `${SELF_NAME}: 平台包 ${pkg} 未安装。请重新安装：npm install -g ${PKG_PREFIX.replace(/-$/, '')}`
  );
  process.exit(1);
}

const result = spawnSync(bin, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
  console.error(`${SELF_NAME}: 无法启动二进制：` + result.error.message);
  process.exit(1);
}
// 动态加载器在 main 之前失败（缺库、库太旧、glibc 太旧）时退出码是 127，它的报错已经打在终端上。
// aproxy 自己不用 127，但要确认是加载失败才提示：再跑一次 --version 把 stderr 收下来看（只在
// 这种失败时多花这一次）。带界面的版本在缺 WebKitGTK 的 Linux 上就是这样起不来的。
if (result.status === 127 && DESKTOP_GROUP && process.platform === 'linux') {
  const probe = spawnSync(bin, ['--version'], { encoding: 'utf8', timeout: 10000 });
  const loaderError =
    /error while loading shared libraries|symbol lookup error|version `GLIBC_[^']*' not found/;
  if (probe.status === 127 && loaderError.test(probe.stderr || '')) {
    console.error(
      `${SELF_NAME}: 带桌面界面的版本在本机缺系统库，起不来（见上面动态加载器的报错）。\n` + HEADLESS_HINT
    );
  }
}
process.exit(result.status === null ? 1 : result.status);
