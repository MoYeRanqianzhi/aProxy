//! 离线工具只解析显式给出的文件，不查找正在运行的实例，不写入代理 home。
use crate::{Options, Role, config};
use aproxy_ipc::config_file::{self as f, FormatEntry, Stage};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// 诊断不回显可能含凭据的路径；三类离线输入使用同一限额和边界。
const OFFLINE_TYPE: &str = "离线输入必须是可读的本地普通文件";
/// 超限的文本不能进入 TOML 或 JSON 解析器。
const OFFLINE_SIZE: &str = "离线输入超过 8 MiB";
/// 先取得文件句柄，再按同一句柄的类型和长度读取；多读一个字节可以发现并发增长。
pub(super) fn offline_read(path: &Path) -> Result<Vec<u8>, &'static str> {
    use std::io::Read;
    const LIMIT: u64 = 8 * 1024 * 1024;
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(path)
    };
    #[cfg(windows)]
    let file = local_disk_file(path);
    let file = file.map_err(|_| OFFLINE_TYPE)?;
    let meta = file.metadata().map_err(|_| OFFLINE_TYPE)?;
    if !meta.is_file() {
        return Err(OFFLINE_TYPE);
    }
    if meta.len() > LIMIT {
        return Err(OFFLINE_SIZE);
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| OFFLINE_TYPE)?;
    if bytes.len() as u64 > LIMIT {
        return Err(OFFLINE_SIZE);
    }
    Ok(bytes)
}
/// 只从本机盘符打开目录与文件。逐级持有不可改写或换名的目录句柄，不跟随能跳到网络的联接点；
/// 普通和 verbatim 盘符同等对待，不能把 UNC、管道或 DOS 设备名交给文件打开操作。
#[cfg(windows)]
fn local_disk_file(path: &Path) -> std::io::Result<fs::File> {
    use std::{
        io,
        os::windows::{
            fs::{MetadataExt, OpenOptionsExt},
            io::AsRawHandle,
        },
        path::{Component, Prefix},
    };
    use windows_sys::Win32::{
        Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_TYPE_DISK, GetDriveTypeW, GetFileType,
        },
        System::WindowsProgramming::{DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOVABLE},
    };
    let invalid = || io::Error::from(io::ErrorKind::InvalidInput);
    let absolute = std::path::absolute(path)?;
    let mut parts = absolute.components();
    let Some(Component::Prefix(prefix)) = parts.next() else {
        return Err(invalid());
    };
    let (Prefix::Disk(letter) | Prefix::VerbatimDisk(letter)) = prefix.kind() else {
        return Err(invalid());
    };
    if parts.next() != Some(Component::RootDir) {
        return Err(invalid());
    }
    let drive = [u16::from(letter), b':' as u16, b'\\' as u16, 0];
    // SAFETY: 盘符根路径是以 NUL 结尾的 UTF-16 数组，调用不保留它。
    if !matches!(
        unsafe { GetDriveTypeW(drive.as_ptr()) },
        DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_CDROM | DRIVE_RAMDISK
    ) {
        return Err(invalid());
    }
    let mut parent = PathBuf::from(prefix.as_os_str());
    parent.push(r"\");
    let mut guards = Vec::new();
    let mut parts = parts.peekable();
    while let Some(Component::Normal(name)) = parts.next() {
        // DOS 保留名称也可以带扩展名；冒号会让普通文件名指向另一个数据流。
        let name_text = name.to_string_lossy();
        let stem = name_text
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end_matches(' ')
            .to_ascii_uppercase();
        if name_text.contains(':')
            || matches!(
                stem.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            )
            || stem
                .strip_prefix("COM")
                .or_else(|| stem.strip_prefix("LPT"))
                .is_some_and(|s| {
                    matches!(
                        s,
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                    )
                })
        {
            return Err(invalid());
        }
        parent.push(name);
        let mut open = fs::OpenOptions::new();
        open.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
        if parts.peek().is_none() {
            open.read(true);
        } else {
            open.access_mode(FILE_READ_ATTRIBUTES)
                .share_mode(FILE_SHARE_READ);
        }
        let handle = open.open(&parent)?;
        let meta = handle.metadata()?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid());
        }
        if parts.peek().is_none() {
            // SAFETY: 类型查询仅借用仍由 File 持有的句柄。
            if unsafe { GetFileType(handle.as_raw_handle() as isize) } != FILE_TYPE_DISK
                || !meta.is_file()
            {
                return Err(invalid());
            }
            return Ok(handle);
        }
        if !meta.is_dir() {
            return Err(invalid());
        }
        guards.push(handle);
    }
    Err(invalid())
}

/// 校验过的条目只持有本角色的状态。未识别的外部命令永远不因静态检查而执行。
pub(super) struct Layer {
    /// 与代理共享的严格配置，包括环境、进程方式和阶段。
    pub entry: FormatEntry,
    /// 官方角色在本进程运行；第三方没有可静态检查的角色对象。
    pub role: Option<Role>,
    /// 公开的固定角色名，避免把任意路径、配置名称或 argv 打到试跑输出。
    pub label: &'static str,
}
/// 离线模拟器只用已经检查的链和实例文件声明的默认设置，不读全局设置。
pub(super) struct Chain {
    /// 配置文件给出的代理设置。
    pub config: f::FileConfig,
    /// 顺序就是请求出站顺序。
    pub layers: Vec<Layer>,
    /// 外部命令的相对配置路径以选定的实例工作目录为基准。
    pub directory: PathBuf,
    /// 不妨碍运行、但容易误以为回程也会转换的情形。
    pub notes: Vec<&'static str>,
}
/// 固定角色名也是输出的白名单；用户配置的程序名称不能代替命令身份。
fn official(command: &str, args: &[String]) -> Result<Option<&'static str>, String> {
    let stem = Path::new(command)
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or("");
    if !(stem == "aproxy-format" || cfg!(windows) && stem.eq_ignore_ascii_case("aproxy-format.exe"))
    {
        return Ok(None);
    }
    [
        "adaptive",
        "keys",
        "convert",
        "heartbeat",
        "route",
        "rewrite",
        "policy",
    ]
    .into_iter()
    .find(|s| args.first().is_some_and(|a| a == s))
    .map(Some)
    .ok_or_else(|| "官方命令缺少有效角色".into())
}
/// 只重定位角色文件参数，不改进程全局工作目录。绝对路径与用户目录简写沿用角色自己的解析。
pub(super) fn load(
    role: &str,
    args: &[String],
    dir: &Path,
    vars: config::Variables,
) -> Result<Role, String> {
    let opt = Options::parse(args)?;
    let text = opt
        .config
        .map(|p| -> Result<String, &'static str> {
            let bytes = offline_read(&dir.join(config::role_path(p)?))?;
            String::from_utf8(bytes).map_err(|_| "角色配置不是 UTF-8")
        })
        .transpose()?;
    Role::configured(
        role,
        opt,
        text.as_deref().map(config::Source::Offline),
        vars,
    )
}
/// 代理的基础地址允许自己的凭据，但不能带查询或片段；试跑用同一解析结果追加客户端路径。
pub(super) fn base_url(text: &str) -> Result<url::Url, &'static str> {
    let url = url::Url::parse(text).map_err(|_| "base_url 无效")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("base_url 无效");
    }
    Ok(url)
}
/// 配置字段诊断来自公开词表，其余加载错误可能含用户提供的环境名称，不展示在工具输出中。
fn safe_issue(error: String) -> String {
    if error.starts_with("角色配置字段无效：")
        || matches!(
            error.as_str(),
            OFFLINE_TYPE | OFFLINE_SIZE | "角色配置不是 UTF-8"
        )
    {
        error
    } else {
        "角色配置无效".into()
    }
}
/// 使用与代理同源的严格 TOML 解析；只报告固定错误码、位置，不转发可能含环境值的解析器原话。
pub(super) fn chain(path: &Path, home: Option<&Path>, structural: bool) -> Result<Chain, String> {
    let directory = match home {
        Some(dir) => dir,
        None => {
            eprintln!("提示：未指定 --home，相对路径以代理配置所在目录为准。");
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
        }
    };
    let directory = std::path::absolute(directory).map_err(|_| "无法确定角色工作目录")?;
    let text = String::from_utf8(offline_read(path)?).map_err(|_| "代理配置不是 UTF-8")?;
    let (mut cfg, diags) = f::parse(&text, path);
    if let Some(d) = diags.iter().find(|d| d.is_error()) {
        return Err(format!(
            "代理配置无效：{}（行 {}，列 {}）",
            d.code,
            d.line.unwrap_or(1),
            d.column.unwrap_or(1)
        ));
    }
    if let Some(base) = &cfg.base_url {
        base_url(base)?;
    }
    let mut layers = Vec::new();
    let mut notes = Vec::new();
    let mut routed = false;
    for (i, entry) in cfg
        .format
        .take()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        let name = official(&entry.command, &entry.args)?;
        let role = if let Some(name) = name {
            let vars = config::Variables {
                overlay: entry.env.clone(),
                structural,
            };
            let role = match load(name, &entry.args[1..], &directory, vars) {
                Ok(role) => role,
                Err(_) if !structural && !entry.required => {
                    // 未能读配置或取得凭据的工作进程在 hello 之前退出，不能当作握手不兼容。
                    layers.push(Layer {
                        entry,
                        role: None,
                        label: name,
                    });
                    continue;
                }
                Err(e) => return Err(format!("第 {} 项的 {name}：{}", i + 1, safe_issue(e))),
            };
            if entry
                .stages
                .iter()
                .any(|s| !role.stages().contains(&s.as_str()))
            {
                return Err(format!("第 {} 项的 {name} 不支持所声明的阶段", i + 1));
            }
            if routed
                && matches!(name, "convert" | "rewrite")
                && entry.has_stage(Stage::Request)
                && !entry.has_stage(Stage::Retry)
            {
                return Err("路由后的请求转换或改写必须声明 retry".into());
            }
            if let Role::Convert(c) = &role {
                // 决定回程协议的 state 只来自本层自己的 request；首轮不能指望 retry 补上它。
                if (c.from == "auto" || c.to == "auto")
                    && !entry.has_stage(Stage::Request)
                    && (entry.has_stage(Stage::Response) || entry.has_stage(Stage::Stream))
                {
                    return Err("自动协议转换的回程需要声明 request".into());
                }
                if !entry.required && (c.from != c.to || c.from == "auto") {
                    return Err("跨协议转换必须设置 required=true".into());
                }
                if entry.has_stage(Stage::Request)
                    && !entry.has_stage(Stage::Response)
                    && !entry.has_stage(Stage::Stream)
                {
                    notes.push("转换仅作用于请求，响应仍是上游协议");
                }
            }
            routed |= name == "route" && entry.has_stage(Stage::Request);
            Some(role)
        } else {
            None
        };
        layers.push(Layer {
            entry,
            role,
            label: name.unwrap_or("external"),
        });
    }
    Ok(Chain {
        config: cfg,
        layers,
        directory,
        notes,
    })
}
/// 四种起步链均使用默认 retry；只有真正需要的角色才占一次管道往返。
fn init(scenario: &str, dir: &Path) -> Result<(), String> {
    if !matches!(
        scenario,
        "claude-relay" | "key-rotation" | "openrouter" | "adaptive-relay"
    ) {
        return Err("未知场景，可用 claude-relay、key-rotation、openrouter、adaptive-relay".into());
    }
    fs::create_dir_all(dir).map_err(|_| "无法创建输出目录")?;
    let base = dir.canonicalize().map_err(|_| "输出目录无效")?;
    // 短盘符路径仅在普通拼写仍然指向同一目录时去掉 verbatim 前缀；长路径和特殊名称保留原样。
    #[cfg(windows)]
    let base = base
        .to_str()
        .and_then(|p| p.strip_prefix(r"\\?\"))
        .filter(|p| {
            Path::new(p).is_absolute() && p.encode_utf16().count() + "\\adaptive.toml".len() < 260
        })
        .map(PathBuf::from)
        .filter(|p| p.canonicalize().is_ok_and(|actual| actual == base))
        .unwrap_or(base);
    let mut files = Vec::new();
    let mut proxy = if scenario == "openrouter" {
        String::from(
            "# OpenRouter 的上游地址；使用自建网关时同步修改 route.toml。\nbase_url = 'https://openrouter.ai'\n",
        )
    } else {
        String::from(
            "# 替换为实际中继地址；使用密钥池时也要修改 keys.toml 的 origin。\nbase_url = 'https://relay.example.com'\n",
        )
    };
    // TOML 字面串保留 Windows 反斜杠。遇到单引号或控制字符才转为带转义的基本串。
    let file_arg = |name: &str| -> Result<String, &'static str> {
        let joined = base.join(name);
        let path = joined.to_str().ok_or("角色配置路径无效")?;
        if !path.contains('\'') && !path.chars().any(char::is_control) {
            Ok(format!("'{path}'"))
        } else {
            serde_json::to_string(path).map_err(|_| "角色配置路径无效")
        }
    };
    if scenario == "adaptive-relay" {
        proxy += r#"
[timeouts]
# 慢模型可适当延长首字节时限，单位为秒。
first_byte_secs = 600

[[retry.rules]]
# 只限制计数接口的尝试次数，其他请求沿用实例策略。
path = '/v1/messages/count_tokens(\?.*)?'
max_attempts = 3
"#;
        files.push((
            "adaptive.toml",
            "# 连续失败达到次数后切换交付模式；按中继稳定性调整。\nfailure_threshold = 2\n"
                .to_owned(),
        ));
        proxy += &format!(
            r#"
# 根据近期失败情况选择交付模式；阈值在 adaptive.toml 中设置。
[[format]]
name = 'adaptive'
command = 'aproxy-format'
args = ['adaptive', '--config', {}]
process = 'persistent'
stages = ['request', 'decide', 'notify']
required = false
pool_max = 1
worker_idle_secs = 0
"#,
            file_arg("adaptive.toml")?
        );
    }
    if scenario == "openrouter" {
        files.push((
            "route.toml",
            r#"[[channel]]
# 渠道标识用来区分候选上游，可自行命名。
id = 'openrouter'
# 替换为接收 Chat Completions 的完整上游地址。
url = 'https://openrouter.ai/api/v1/chat/completions'
# 只写存放凭据的环境变量名，不要填入密钥本身。
credential_env = 'OPENROUTER_API_KEY'
"#
            .into(),
        ));
        proxy += &format!(
            r#"
# 选用 route.toml 中的渠道；重试时重新选择。
[[format]]
name = 'route'
command = 'aproxy-format'
args = ['route', '--config', {}]
process = 'persistent'
stages = ['request', 'retry']
"#,
            file_arg("route.toml")?
        );
    }
    if scenario != "key-rotation" {
        let (from, stages) = if scenario == "openrouter" {
            ("auto", "'request', 'retry', 'stream'")
        } else {
            ("anthropic_messages", "'request', 'stream'")
        };
        proxy += &format!(
            r#"
# 在客户端与中继的协议间转换；按两端协议修改 --from 和 --to。
[[format]]
name = 'convert'
command = 'aproxy-format'
args = ['convert', '--from', '{from}', '--to', 'openai_chat']
process = 'persistent'
stages = [{stages}]
required = true
"#
        );
    }
    if matches!(scenario, "key-rotation" | "adaptive-relay") {
        files.push((
            "keys.toml",
            r#"[[pool]]
# 替换为允许接收这些密钥的来源，须与实际中继的协议、主机和端口一致。
origin = 'https://relay.example.com'
# 按中继要求选择鉴权头和前缀。
header = 'authorization'
scheme = 'Bearer'
keys = [
    # 第一把密钥：可修改标识和变量名，变量值在环境中设置。
    { id = 'a', env = 'RELAY_KEY_A' },
    # 第二把密钥：按需增删条目，不在文件里写入密钥。
    { id = 'b', env = 'RELAY_KEY_B' },
]
"#
            .into(),
        ));
        proxy += &format!(
            r#"
# 在同一中继的密钥之间轮换；在 keys.toml 中设置来源和环境变量名。
[[format]]
name = 'keys'
command = 'aproxy-format'
args = ['keys', '--config', {}]
process = 'persistent'
stages = ['request', 'retry']
pool_max = 1
worker_idle_secs = 0
"#,
            file_arg("keys.toml")?
        );
    }
    files.insert(0, ("config.toml", proxy));
    if files
        .iter()
        .any(|(n, _)| base.join(n).symlink_metadata().is_ok())
    {
        return Err("输出文件已存在，不覆盖".into());
    }
    for (name, text) in &files {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(base.join(name))
            .map_err(|_| "无法新建输出文件")?;
        f.write_all(text.as_bytes()).map_err(|_| "写入配置失败")?;
    }
    println!("已生成（{}）：", base.display());
    for (name, _) in files {
        let purpose = match name {
            "config.toml" => "实例配置里的 [[format]] 链",
            "keys.toml" => "keys 的配置",
            "route.toml" => "route 的配置",
            _ => "adaptive 的配置",
        };
        println!("  {name}   {purpose}");
    }
    println!("下一步：");
    match scenario {
        "openrouter" => println!("  1. 设置环境变量 OPENROUTER_API_KEY（密钥不写进文件）"),
        "claude-relay" => println!("  1. 把 base_url 改为实际中继地址"),
        _ => println!("  1. 设置环境变量 RELAY_KEY_A、RELAY_KEY_B（密钥不写进文件）"),
    }
    println!(
        "  2. 检查：aproxy-format check \"{}\"",
        base.join("config.toml").display()
    );
    println!("  3. 把 config.toml 的内容并入实例的配置，然后启动或重启实例");
    Ok(())
}
/// 按名字去重列出未检查的凭据；控制字符转义，诊断不能被配置里的换行或转义序列伪造。
fn unchecked<'a>(roles: impl IntoIterator<Item = &'a Role>, proxy: bool, external: bool) {
    let mut names = std::collections::BTreeSet::new();
    for role in roles {
        match role {
            Role::Keys(k) => names.extend(k.credential_names()),
            Role::Route(r) => names.extend(r.credential_names()),
            _ => {}
        }
    }
    if names.is_empty() && !proxy {
        return;
    }
    println!("没有检查：");
    if !names.is_empty() {
        println!(
            "  - 环境变量 {} 是否已设置（启动密钥角色或选用渠道时需要）",
            names
                .into_iter()
                .map(|s| s.escape_debug().to_string())
                .collect::<Vec<_>>()
                .join("、")
        );
    }
    if external {
        println!("  - 第三方程序的能力与配置（未启动程序）");
    }
    if proxy {
        println!("  - 实例的其余配置（监听地址、超时等），由 aproxy doctor 检查");
    }
}
/// 工具的参数与角色协议分开解析，未知参数可能含密钥，不在用法错误中回显。
pub(super) fn run(name: &str, args: &[String]) -> Result<(), String> {
    let mut words = Vec::new();
    let mut home = None;
    let mut args = args.iter().map(String::as_str);
    while let Some(word) = args.next() {
        if name != "init" && word == "--home" {
            let dir = args
                .next()
                .filter(|v| !v.is_empty() && !v.starts_with("--"))
                .ok_or("--home 缺少目录")?;
            if home.replace(Path::new(dir)).is_some() {
                return Err("--home 重复".into());
            }
        } else {
            words.push(word);
        }
    }
    match (name, words.as_slice()) {
        ("init", [s, "--output", p]) => init(s, Path::new(p)),
        ("check", [p]) => {
            let c = chain(Path::new(p), home, true)?;
            for n in c.notes { println!("提示：{n}"); }
            println!("format 链与官方角色配置有效（{} 个程序）。", c.layers.len());
            unchecked(c.layers.iter().filter_map(|l| l.role.as_ref()), true, c.layers.iter().any(|l| l.role.is_none()));
            Ok(())
        }
        ("check", [r, "--config", p]) => {
            let role = load(r, &["--config".into(), (*p).into()], home.unwrap_or(Path::new(".")), config::Variables { structural: true, ..Default::default() }).map_err(safe_issue)?;
            println!("{r} 角色配置结构有效。"); unchecked([&role], false, false); Ok(())
        }
        ("try", [p, "--request", sample]) => crate::dryrun::run(Path::new(p), Path::new(sample), home, false),
        ("try", [p, "--request", sample, "--allow-external"]) => crate::dryrun::run(Path::new(p), Path::new(sample), home, true),
        _ => Err("用法：init 场景 --output 目录；check 代理配置 [--home 目录] 或 check 角色 --config 文件；try 代理配置 --request 样例 [--home 目录] [--allow-external]".into()),
    }
}

#[cfg(test)]
/// 用独立目录生成每个场景，生成物须由真正的链加载器接受，二次初始化不能悄悄覆盖文件。
mod tests {
    use super::*;
    /// 起步链按职责最小化；所有转换必需、所有有状态密钥池都只占一个常驻进程。
    #[test]
    fn check_accepts_every_generated_scenario_without_credentials() {
        for (scenario, names) in [
            ("claude-relay", vec!["convert"]),
            ("key-rotation", vec!["keys"]),
            ("openrouter", vec!["route", "convert"]),
            ("adaptive-relay", vec!["adaptive", "convert", "keys"]),
        ] {
            let dir = tempfile::tempdir().unwrap();
            init(scenario, dir.path()).unwrap();
            let file = dir.path().join("config.toml");
            let c = chain(&file, None, true).unwrap();
            assert_eq!(c.layers.iter().map(|l| l.label).collect::<Vec<_>>(), names);
            assert_eq!(
                c.config.delivery.unwrap_or(f::Delivery::Retry),
                f::Delivery::Retry
            );
            for l in c.layers {
                match l.label {
                    "keys" | "adaptive" => {
                        assert_eq!(l.entry.pool_max, 1);
                        assert_eq!(l.entry.worker_idle_secs, 0);
                    }
                    "convert" => {
                        assert!(l.entry.required);
                        assert_eq!(l.entry.has_stage(Stage::Retry), scenario == "openrouter");
                    }
                    _ => {}
                }
            }
            fs::write(&file, "user content").unwrap();
            assert!(init(scenario, dir.path()).is_err());
            assert_eq!(fs::read_to_string(file).unwrap(), "user content");
        }
    }
    /// 故意让 route 只订阅首轮，以免公用的通用后缀检查掩盖角色特有的顺序检查。
    fn after_route(role: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("route.toml"),
            "[[channel]]\nid='one'\nurl='https://relay.test'",
        )
        .unwrap();
        let args = if role == "rewrite" {
            "'rewrite'"
        } else {
            "'convert','--from','anthropic_messages','--to','openai_chat'"
        };
        let file = dir.path().join("config.toml");
        fs::write(&file,format!("[[format]]\nname='r'\ncommand='aproxy-format'\nargs=['route','--config','route.toml']\nstages=['request']\n[[format]]\nname='after'\ncommand='aproxy-format'\nargs=[{args}]\nstages=['request']")).unwrap();
        chain(&file, None, true).err().unwrap()
    }
    /// 换渠道后客户端体须按新的目标协议再次转换。
    #[test]
    fn check_rejects_convert_after_route_without_retry() {
        assert!(after_route("convert").contains("必须声明 retry"));
    }
    /// 换渠道也会更换模型别名和头，因此后置改写同样需要重跑。
    #[test]
    fn check_rejects_rewrite_after_route_without_retry() {
        assert!(after_route("rewrite").contains("必须声明 retry"));
    }
}

/// 命名空间必须在打开前拒绝；verbatim 本地盘符仍能读取真实的配置。
#[cfg(all(test, windows))]
#[test]
fn offline_accepts_verbatim_disk_but_not_device_or_share_paths() {
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("sample");
    fs::write(&local, b"local").unwrap();
    assert_eq!(
        offline_read(&local.canonicalize().unwrap()).unwrap(),
        b"local"
    );
    for name in [
        r"\\localhost\unused-share\s",
        r"\\?\UNC\localhost\unused-share\s",
        r"\\.\pipe\unused",
        r"\\?\GLOBALROOT\Device\Null",
        "NUL.txt",
    ] {
        assert_eq!(offline_read(Path::new(name)), Err(OFFLINE_TYPE));
    }
    assert_eq!(offline_read(dir.path()), Err(OFFLINE_TYPE));
}
/// FIFO 的写端无需存在；打开后在同一句柄上拒绝它，而不是等待另一进程写入。
#[cfg(all(test, unix))]
#[test]
fn offline_refuses_fifo_without_a_writer() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fifo");
    let c = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: 名称以 NUL 结尾且只在测试私有目录内创建节点。
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert_eq!(offline_read(&path), Err(OFFLINE_TYPE));
}
