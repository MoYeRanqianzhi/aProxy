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

// 平台 → 子包后缀。linux 需区分 glibc/musl（两套静态链接产物）。
function platformPackage() {
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
    return PKG_PREFIX + 'linux-x64' + (isMusl() ? '-musl' : '');
  }
  if (platform === 'linux' && arch === 'arm64') {
    return PKG_PREFIX + 'linux-arm64' + (isMusl() ? '-musl' : '');
  }
  return null;
}

// process.report 的 glibcVersionRuntime 只在 glibc 构建的 Node 上有值；
// musl 环境（Alpine 等）拿不到它。report 不可用时保守回退 gnu（更常见）。
function isMusl() {
  try {
    if (!process.report || !process.report.getReport) return false;
    return !process.report.getReport().header.glibcVersionRuntime;
  } catch {
    return false;
  }
}

const pkg = platformPackage();
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
