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
// 包名自检测：wrapper 由主包 package.json 的 name 推导子包前缀与二进制名
// （@meowo/aproxy → 子包 @meowo/aproxy-*、二进制 aproxy；@meowo/aproxy-format
// → 同理）——一份脚本两组通用，改包名时无需同步此处。

const { spawnSync } = require('child_process');
const path = require('path');

const SELF_NAME = path.basename(require('../package.json').name); // aproxy | aproxy-format
const PKG_PREFIX = require('../package.json').name + '-';

// gnu 产物能运行的 glibc 下限。必须与 .github/workflows/release.yml 里 gnu 构建的
// glibc 下限、scripts/install.sh 的 MIN_GLIBC 保持一致；低于它的系统改用 musl 产物。
const MIN_GLIBC = '2.28';

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
process.exit(result.status === null ? 1 : result.status);
