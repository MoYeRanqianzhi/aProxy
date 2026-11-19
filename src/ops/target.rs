//! 目标解析：把命令行上的 target（省略 / 端口 / `all` / 别名或路径 / `idle [秒]`）变成
//! 一份要处理的实例清单。stop 与 restart 共用，语义完全相同。

use std::path::PathBuf;

use crate::daemon::{self, InstanceRecord};
use crate::settings;

/// 配置路径的运行实例匹配键：别名匹配、配置目录去重、doctor 扫描共用同一比较规则。
pub use crate::settings::path_match_key as config_path_key;

/// 把「别名 | `default` 保留字 | 配置文件路径」解析成配置文件路径；三样都不是时为 None。
/// 别名表优先于路径：同名时别名说了算。
pub fn resolve_config_target(target: &str) -> Option<PathBuf> {
    if let Some(p) = settings::resolve_alias(target) {
        return Some(PathBuf::from(p));
    }
    if let Some(p) = settings::resolve_default_target(target) {
        return Some(p);
    }
    let p = settings::expand_path(target);
    if p.is_file() {
        return Some(p);
    }
    None
}

/// 选出来的实例清单，连同调用方该让用户知道的旁注。
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// 要处理的实例。空 = 这个 target 下没有可处理的实例，原因见 `empty`
    pub instances: Vec<InstanceRecord>,
    /// 清单为空的原因；清单不空时为 None
    pub empty: Option<Empty>,
    /// 进程仍在、却不应答控制通道，因而**没有**纳入清单的实例（只在不带 `force`、
    /// target 省略或为 `all` 时收集）。它们只能强制结束；调用方把它们报成没处理成的那几个
    pub unresponsive_skipped: Vec<InstanceRecord>,
}

/// 清单为空、但不算错误的两种情形。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Empty {
    /// 没有运行中的实例
    NoneRunning,
    /// `idle`：没有闲置超过阈值的实例
    NoneIdle { idle_secs: u64 },
}

/// 选不出目标的原因。每一种都意味着这条命令到此结束、什么都没动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectError {
    /// target 既不是端口号，也不是已知的别名或存在的配置文件
    UnknownTarget { target: String },
    /// 别名（或路径）解析出了配置文件，但没有实例在用它运行
    NotRunning {
        target: String,
        config_path: PathBuf,
    },
    /// 指定端口上没有本 home 的实例
    NothingOnPort { port: String },
    /// 找到了实例，但它不应答控制通道，没带 `force` 就停不了。`target` 是用户写的别名
    /// （按端口指定时为 None）
    Unresponsive {
        target: Option<String>,
        port: String,
        pid: u32,
    },
    /// target 省略，而运行中的实例不止一个：必须指明。`ports` 是它们各自的端口
    Ambiguous { ports: Vec<String> },
}

/// 按 target 选出要处理的实例。
///
/// `force` 时把「进程仍在、却不应答控制通道」的实例也纳入（按端口指定时回退注册表记录；
/// `all` 时并入普查结果）：它们正是只能强制终止的那一类，而强制终止之前按 pid + 创建时间
/// 核验身份，不依赖 IPC。
pub async fn select(
    target: Option<&str>,
    threshold: Option<u64>,
    force: bool,
) -> Result<Selection, SelectError> {
    // idle 保留字：全部空闲超阈值的实例（阈值可临时覆盖 settings 配置）
    if target.is_some_and(|t| t.eq_ignore_ascii_case("idle")) {
        let idle_secs = threshold.unwrap_or(settings::load().idle_timeout_secs);
        let now = daemon::now_millis();
        let instances: Vec<_> = daemon::list_instances()
            .await
            .into_iter()
            .filter(|s| now.saturating_sub(s.activity.last_request_at) / 1000 >= idle_secs)
            .map(|s| s.instance)
            .collect();
        let empty = instances
            .is_empty()
            .then_some(Empty::NoneIdle { idle_secs });
        return Ok(Selection {
            instances,
            empty,
            unresponsive_skipped: Vec::new(),
        });
    }

    // 别名定位：非数字且非 all 的 target 先查别名表/default——按 config_path 匹配
    // 运行实例（别名不依赖端口号，端口变了别名依然有效）
    if let Some(target) = target
        && target != "all"
        && target.parse::<u16>().is_err()
    {
        let Some(cfg_path) = resolve_config_target(target) else {
            return Err(SelectError::UnknownTarget {
                target: target.to_string(),
            });
        };
        let key = config_path_key(&cfg_path.display().to_string());
        let survey = daemon::survey_instances().await;
        // 没有配置文件的实例（config_path 为 null）不会被任何配置路径选中
        let matches = |i: &&InstanceRecord| {
            i.config_path
                .as_deref()
                .is_some_and(|path| config_path_key(path) == key)
        };
        if let Some(info) = survey.responsive.iter().map(|s| &s.instance).find(matches) {
            return Ok(Selection::of(vec![info.clone()]));
        }
        return match survey.unresponsive.iter().find(matches) {
            Some(info) if force => Ok(Selection::of(vec![info.clone()])),
            Some(info) => Err(SelectError::Unresponsive {
                target: Some(target.to_string()),
                port: daemon::port_of(&info.listen_addr).to_string(),
                pid: info.pid,
            }),
            None => Err(SelectError::NotRunning {
                target: target.to_string(),
                config_path: cfg_path,
            }),
        };
    }

    // 指定端口时不依赖注册表：直接按端口 IPC 定位（注册表丢失也能停）
    if let Some(target) = target
        && target != "all"
    {
        let port = daemon::port_of(target).to_string();
        return match daemon::ipc_ping(&port).await {
            Ok(status) => Ok(Selection::of(vec![status.instance])),
            Err(_) => match unresponsive_record(&port) {
                Some(info) if force => Ok(Selection::of(vec![info])),
                Some(info) => Err(SelectError::Unresponsive {
                    target: None,
                    port,
                    pid: info.pid,
                }),
                None => Err(SelectError::NothingOnPort { port }),
            },
        };
    }

    let survey = daemon::survey_instances().await;
    let mut instances: Vec<InstanceRecord> =
        survey.responsive.into_iter().map(|s| s.instance).collect();
    let mut unresponsive_skipped = Vec::new();
    if force {
        instances.extend(survey.unresponsive);
    } else {
        unresponsive_skipped = survey.unresponsive;
    }
    // target 省略时只接受唯一的那个实例
    if target.is_none() && instances.len() > 1 {
        return Err(SelectError::Ambiguous {
            ports: instances
                .iter()
                .map(|i| daemon::port_of(&i.listen_addr).to_string())
                .collect(),
        });
    }
    let empty = instances.is_empty().then_some(Empty::NoneRunning);
    Ok(Selection {
        instances,
        empty,
        unresponsive_skipped,
    })
}

impl Selection {
    fn of(instances: Vec<InstanceRecord>) -> Self {
        Selection {
            instances,
            empty: None,
            unresponsive_skipped: Vec::new(),
        }
    }
}

/// 按端口读注册表记录，且记录的进程经「pid + 创建时间」核验仍在——不应答
/// 控制通道的实例（多半挂死）只能这样定位。
fn unresponsive_record(port: &str) -> Option<InstanceRecord> {
    let info = daemon::read_instance_file_in(&daemon::run_dir(), port)?;
    (crate::process::record_identity(info.pid, &info.start)
        == crate::process::RecordIdentity::Alive)
        .then_some(info)
}
