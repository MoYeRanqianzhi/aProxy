//! 安装文件操作的故障。路径和系统错误按原样传递；嵌套操作由同一读者一起渲染。
use super::{InstallFailure, reason::Cause};
use aproxy_ipc::{i18n::Lang, message};
use serde::{Deserialize, Serialize};

macro_rules! stored { (Text)=>{String}; (Reason)=>{Box<Cause>}; }
macro_rules! arg {
    ($v:ident, Text,$lang:ident) => {
        $v.as_str()
    };
    ($v:ident, Reason,$lang:ident) => {
        &$v.render($lang)
    };
}
macro_rules! reasons {
    ($($id:ident { $($v:ident:$t:ident),+ }, en:$en:literal, zh:$zh:literal;)+) => {
        /// 可以写进私有现场的文件操作故障；不是公开的错误码。
        #[derive(Clone,Debug,Serialize,Deserialize)]
        #[serde(tag="id",content="args",rename_all="snake_case")]
        pub enum FileIssue { $( #[doc=$zh] $id { $($v:stored!($t)),+ }, )+ }
        impl FileIssue {
            /// 根据消费端选择措辞，不读取进程级语言。
            pub fn render(&self,lang:Lang)->String { match self { $(Self::$id { $($v),+ } => { message!(sentence($($v:&str),+),en:$en,zh:$zh);sentence(lang,$(arg!($v,$t,lang)),+) },)+ } }
        }
    };
}
reasons! {
Floor { target: Text, floor: Text }, en: "Target version {target} is below the minimum {floor}. Versions 0.1.x and 0.2.0 or later have incompatible control protocols and disk layouts. --allow-downgrade does not bypass this minimum. To return to 0.1.x, use the current version to run `aproxy stop all`, delete the aproxy binary from bin, then reinstall with the 0.1.x installation script.", zh: "目标版本 {target} 低于 {floor}，不能安装：0.1.x 与 0.2.0 起的版本互不兼容（控制协议与磁盘布局都变了），\
         --allow-downgrade 也越不过这条下限。要回到 0.1.x：先用当前版本执行 `aproxy stop all` 停掉全部实例，\
         删掉 bin 目录里的 aproxy 二进制，再用 0.1.x 的安装脚本重新安装";
Execute { path: Text, e: Text }, en: "Cannot execute {path}: {e}", zh: "{path} 无法执行: {e}";
Timeout { path: Text, secs: Text }, en: "{path} --version did not finish within {secs} seconds", zh: "{path} --version 在 {secs} 秒内没有结束";
Exit { path: Text, status: Text, stderr: Text }, en: "{path} --version failed ({status}): {stderr}", zh: "{path} --version 失败（{status}）: {stderr}";
BelowFloor { path: Text, e: Reason }, en: "{path}: {e}", zh: "{path}：{e}";
BadLine { path: Text, stdout: Text }, en: "{path} --version did not report `aproxy v<version> (…)` (required since 0.2.0): {stdout:?}", zh: "{path} --version 的输出不是 `aproxy v<版本> (…)`（0.2.0 起每个版本都这样输出）: {stdout:?}";
Missing { path: Text }, en: "Source binary does not exist: {path}", zh: "源二进制不存在: {path}";
StageDir { e: Text }, en: "Cannot create the staging directory: {e}", zh: "staging 目录创建失败: {e}";
StageCopy { e: Text }, en: "Cannot copy the binary to staging: {e}", zh: "复制到 staging 失败: {e}";
Version { path: Text, probed: Text, target_version: Text }, en: "Version mismatch: {path} reports {probed}; expected {target_version}", zh: "版本不匹配：{path} 自报 {probed}，目标 {target_version}";
BinDir { e: Text }, en: "Cannot create the bin directory: {e}", zh: "bin 目录创建失败: {e}";
Fallback { e: Text }, en: "Cannot write the fallback launchers: {e}", zh: "入口兜底脚本写入失败: {e}";
Placement { e: Text }, en: "Could not place the new binary; bin is unchanged: {e}", zh: "新二进制落位失败，bin 未改动: {e}";
PlacementRecovery { e: Text, bin: Text, back: Text, old: Text }, en: "Could not place the new binary ({e}) or move the old binary back to {bin} ({back}); the old binary is at {old}", zh: "新二进制落位失败（{e}），把旧二进制改回 {bin} 也失败（{back}）：旧二进制现在在 {old}";
Reports { v: Text, expected_version: Text }, en: "Reports version {v}; expected {expected_version}", zh: "自报版本 {v}，应为 {expected_version}";
VerifiedRollback { e: Reason }, en: "Verification after placement failed; the original binary was restored: {e}", zh: "落位后核对失败，已换回原来的二进制: {e}";
FailedRollback { e: Reason, back: Reason }, en: "Verification after placement failed ({e}); restoring the original binary also failed: {back}", zh: "落位后核对失败（{e}），换回原来的二进制也失败: {back}";
Copy { source: Text, path: Text, e: Text }, en: "Cannot copy {source} to {path}: {e}", zh: "{source} 复制到 {path} 失败: {e}";
Collision { dir: Text }, en: "Cannot allocate a unique temporary name in {dir}", zh: "{dir} 里取不到不冲突的中间文件名";
Retire { e: Text }, en: "Cannot move the old binary aside; another program may have it open. bin/ is unchanged: {e}", zh: "旧二进制让位失败（可能被别的程序打开着），bin/ 未改动: {e}";
Retain { e: Text }, en: "Cannot retain the old binary for rollback if the new version fails to start; bin is unchanged: {e}", zh: "旧二进制保留失败（没有它，实例在新版本下起不来时无从回滚），bin 未改动: {e}";
MoveAside { e: Text }, en: "Cannot move the new binary aside: {e}", zh: "挪开新二进制失败: {e}";
RestoreBoth { e: Text, old: Text, aside: Text }, en: "Cannot restore the old binary: {e}; old binary: {old}; new binary: {aside}", zh: "旧二进制改回失败: {e}；旧二进制在 {old}，新二进制在 {aside}";
RestoreOld { e: Text, old: Text }, en: "Cannot restore the old binary: {e}; the old binary is at {old}", zh: "旧二进制改回失败: {e}；旧二进制在 {old}";
WriteState { e: Text }, en: "Cannot write install/state.json: {e}", zh: "install/state.json 写入失败: {e}";
Staging { e: Reason }, en: "Could not stage the binary: {e}", zh: "备料失败: {e}";
Swap { e: Reason }, en: "Binary replacement failed: {e}", zh: "二进制交换失败: {e}";
}
impl From<FileIssue> for Cause {
    fn from(e: FileIssue) -> Self {
        Self::Files(e)
    }
}
impl From<FileIssue> for InstallFailure {
    fn from(e: FileIssue) -> Self {
        Self::because("install_failed", e.into())
    }
}
