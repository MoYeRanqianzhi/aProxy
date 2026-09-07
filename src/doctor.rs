//! `aproxy doctor`：配置体检。三个独立检查函数分级：
//! - error 级：settings.json 静态合法性（在 settings::check_settings_errors_in，
//!   每次软件运行都执行；此处汇总展示）
//! - warning 级 1：别名指向的配置文件深入审查（toml 语法/校验 + 端口冲突）
//! - warning 级 2：配置目录下未被别名覆盖的其余 toml 轻量检查
//!
//! warning 不影响运行（端口冲突是合法的多开前状态，提示即可），仅 doctor 检查。

use crate::config::{self, Config};
use crate::settings::{self, Settings};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 一条体检发现
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub level: Level,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
        }
    }
}

/// 一次体检的完整结果
#[derive(Debug, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn error_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.level == Level::Error)
            .count()
    }
    pub fn warn_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.level == Level::Warn)
            .count()
    }
    pub fn print(&self) {
        for f in &self.findings {
            println!("[{}] {}", f.level.tag(), f.message);
        }
        println!(
            "检查结果: {} error, {} warning",
            self.error_count(),
            self.warn_count()
        );
    }
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}

/// 执行完整体检（三个检查函数依次调用并汇总）
pub fn run(settings_path: &Path) -> Report {
    let settings = settings::load_from(settings_path);
    run_with(settings::check_settings_errors_in(settings_path), settings)
}

/// 同上，Settings 与 error 清单由调用方注入（测试用——注入的 settings 与
/// 其 config_dirs 必须一致，否则扫描目录会跑偏）
fn run_with(errors: Vec<String>, settings: Settings) -> Report {
    let mut report = Report::default();

    // 1) error 级：settings.json 静态合法性（每次软件运行都查；此处正式汇总）
    for e in errors {
        report.findings.push(Finding {
            level: Level::Error,
            message: e,
        });
    }

    // 2) warning 级：别名配置深入审查（语法/校验/端口冲突）
    report.findings.extend(check_aliased_configs(&settings));

    // 3) warning 级：配置目录下未被别名覆盖的其余 toml
    report.findings.extend(check_unaliased_configs(&settings));

    // 4) 看门狗字段越界检查（error=会造成看护故障；warning=合法但需确认意图）
    report.findings.extend(check_watchdog_settings(&settings));

    report
}

/// 看门狗字段的越界检查：
/// - heartbeat_secs = 0：扫描周期退化为紧死循环（空转烧 CPU），error
/// - stale_after_cycles = 0：除数为 0 / 永远无法确认挂死，error
/// - max_restarts = 0：合法（仅观测不重拉）但等于放弃自愈，warning 确认意图
/// - heartbeat_secs 过大：挂死检测延迟随之放大到分钟级，warning 提示
pub fn check_watchdog_settings(settings: &Settings) -> Vec<Finding> {
    let mut findings = Vec::new();
    if settings.watchdog_heartbeat_secs == 0 {
        findings.push(Finding {
            level: Level::Error,
            message: "settings.json 的 watchdog_heartbeat_secs = 0：心跳扫描周期为 0 会让看护者空转烧 CPU，请设为 ≥ 5".to_string(),
        });
    } else if settings.watchdog_heartbeat_secs > 600 {
        findings.push(Finding {
            level: Level::Warn,
            message: format!(
                "settings.json 的 watchdog_heartbeat_secs = {}：心跳周期过大，挂死检测延迟将达 {} 秒级",
                settings.watchdog_heartbeat_secs, settings.watchdog_heartbeat_secs
            ),
        });
    }
    if settings.watchdog_stale_after_cycles == 0 {
        findings.push(Finding {
            level: Level::Error,
            message: "settings.json 的 watchdog_stale_after_cycles = 0：挂死容忍周期数必须 ≥ 1，否则无法判定挂死".to_string(),
        });
    }
    if settings.watchdog_max_restarts == 0 {
        findings.push(Finding {
            level: Level::Warn,
            message: "settings.json 的 watchdog_max_restarts = 0：看护者只观测不重拉，实例崩溃后不会自动恢复".to_string(),
        });
    }
    findings
}

/// 别名配置深入审查（warning 级）：
/// - 每个 toml 语法可解析 + base_url 校验
/// - 端口冲突：别名配置之间、与运行实例之间（合法但提示，多开前状态常见）
pub fn check_aliased_configs(settings: &Settings) -> Vec<Finding> {
    let mut findings = Vec::new();
    // 别名路径 -> 解析出的配置；同一路径挂多个别名时只查一次
    let mut configs: HashMap<String, (PathBuf, Option<Config>)> = HashMap::new();
    for (name, raw) in &settings.aliases {
        let path = settings::expand_path(raw);
        let key = settings::path_match_key(&path.display().to_string());
        let entry = configs
            .entry(key)
            .or_insert_with(|| (path.clone(), parse_config_file(&path)));
        let (_, cfg) = entry;
        match cfg {
            None => findings.push(Finding {
                level: Level::Warn,
                message: format!(
                    "别名 \"{name}\" 指向的配置无法解析为有效 aProxy 配置: {}",
                    path.display()
                ),
            }),
            Some(c) => {
                if let Err(e) = c.validate() {
                    findings.push(Finding {
                        level: Level::Warn,
                        message: format!("别名 \"{name}\" 的配置校验失败: {e}"),
                    });
                }
            }
        }
    }
    findings.extend(port_conflict_findings(
        "别名配置",
        configs
            .iter()
            .filter_map(|(_, (p, c))| c.as_ref().map(|c| (p.display().to_string(), c.clone()))),
    ));
    findings
}

/// 目录 toml 扫描（warning 级）：settings.config_dirs 的全部目录（含两个默认）
/// 下的 *.toml（只查该层，不递归子目录），排除已被别名覆盖的文件——那些
/// 由 check_aliased_configs 精查过。
pub fn check_unaliased_configs(settings: &Settings) -> Vec<Finding> {
    let mut findings = Vec::new();
    let aliased: std::collections::HashSet<String> = settings
        .aliases
        .values()
        .map(|p| settings::path_match_key(&settings::expand_path(p).display().to_string()))
        .collect();
    // 默认配置文件也被别名外的方式使用，视为已覆盖
    let default_key = settings
        .default_config
        .as_deref()
        .map(|p| settings::path_match_key(&settings::expand_path(p).display().to_string()));
    // 默认 config.toml 始终是「在用」配置
    let mut covered = aliased;
    if let Some(k) = default_key {
        covered.insert(k);
    }
    covered.insert(settings::path_match_key(
        &config::config_path().display().to_string(),
    ));

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut orphans: Vec<(PathBuf, Option<Config>)> = Vec::new();
    for dir in settings::effective_config_dirs(settings) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            // 默认目录不存在（全新安装）不算问题；用户自加目录不存在提示
            if !settings.config_dirs.iter().any(|raw| {
                settings::path_match_key(&settings::expand_path(raw).display().to_string())
                    == settings::path_match_key(&dir.display().to_string())
            }) {
                continue;
            }
            findings.push(Finding {
                level: Level::Warn,
                message: format!("配置目录不存在: {}", dir.display()),
            });
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let key = settings::path_match_key(&path.display().to_string());
            if covered.contains(&key) || !seen.insert(key) {
                continue;
            }
            orphans.push((path.clone(), parse_config_file(&path)));
        }
    }
    for (path, cfg) in &orphans {
        match cfg {
            None => findings.push(Finding {
                level: Level::Warn,
                message: format!(
                    "目录中的 toml 无法解析为有效 aProxy 配置（未被任何别名引用）: {}",
                    path.display()
                ),
            }),
            Some(c) => {
                if let Err(e) = c.validate() {
                    findings.push(Finding {
                        level: Level::Warn,
                        message: format!("配置校验失败（未被任何别名引用）{}: {e}", path.display()),
                    });
                }
            }
        }
    }
    findings.extend(port_conflict_findings(
        "目录配置",
        orphans
            .iter()
            .filter_map(|(p, c)| c.as_ref().map(|c| (p.display().to_string(), c.clone()))),
    ));
    findings
}

/// 端口冲突检测：同组配置内 listen_addr 端口相同的两两提示（warning——
/// 多开的配置文件并存时端口冲突只在实际同时启动时才报错，日常可共存）
pub fn port_conflict_findings(
    group: &str,
    configs: impl Iterator<Item = (String, Config)>,
) -> Vec<Finding> {
    let mut by_port: HashMap<u16, Vec<String>> = HashMap::new();
    for (path, cfg) in configs {
        if let Ok(port) = crate::daemon::port_of(&cfg.listen_addr).parse::<u16>() {
            by_port.entry(port).or_default().push(path);
        }
    }
    let mut findings = Vec::new();
    for (port, mut paths) in by_port {
        if paths.len() > 1 {
            paths.sort();
            findings.push(Finding {
                level: Level::Warn,
                message: format!(
                    "{group} 存在端口冲突（{port}）：{}——这些配置同时启动会失败，分开启动合法",
                    paths.join("、")
                ),
            });
        }
    }
    findings
}

/// 读 toml → Config（normalized）。无法解析（语法错/字段错）返回 None——
/// 由调用方生成对应提示。
pub fn parse_config_file(path: &Path) -> Option<Config> {
    let content = std::fs::read_to_string(path).ok()?;
    let cfg: Config = toml::from_str(&content).ok()?;
    Some(cfg.normalized())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_cfg(dir: &Path, name: &str, listen: &str, base: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(
            &p,
            format!("base_url = \"{base}\"\nlisten_addr = \"{listen}\"\n"),
        )
        .unwrap();
        p
    }

    fn settings_path_in_tmp(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    #[test]
    fn doctor_reports_alias_conflicts_and_bad_syntax() {
        let dir = tempfile::tempdir().unwrap();
        // 两个别名配置端口相同 → 冲突 warning；一个语法坏 → 解析 warning
        let a = write_cfg(
            dir.path(),
            "a.toml",
            "127.0.0.1:59841",
            "https://a.example.com",
        );
        let b = write_cfg(
            dir.path(),
            "b.toml",
            "127.0.0.1:59841",
            "https://b.example.com",
        );
        let bad = dir.path().join("bad.toml");
        std::fs::write(&bad, "not = [valid toml").unwrap();
        let esc = |p: &Path| {
            p.display()
                .to_string()
                .replace(std::path::MAIN_SEPARATOR, "/")
        };
        std::fs::write(
            settings_path_in_tmp(dir.path()),
            format!(
                r#"{{"aliases": {{"a": "{}", "b": "{}", "bad": "{}"}}}}"#,
                esc(&a),
                esc(&b),
                esc(&bad)
            ),
        )
        .unwrap();
        let report = run(&settings_path_in_tmp(dir.path()));
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.level == Level::Warn && f.message.contains("端口冲突")),
            "应报端口冲突: {:?}",
            report.findings
        );
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.message.contains("无法解析") && f.message.contains("bad")),
            "应报语法坏配置: {:?}",
            report.findings
        );
        assert_eq!(report.error_count(), 0, "别名配置问题都是 warning 级");
    }

    #[test]
    fn doctor_unaliased_scan_excludes_aliased_and_defaults() {
        let root = tempfile::tempdir().unwrap();
        // 目录 A：被别名覆盖的 x.toml；目录 B（用户自加）：孤儿 y.toml
        let dir_a = root.path().join("a");
        let dir_b = root.path().join("b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let x = write_cfg(&dir_a, "x.toml", "127.0.0.1:59842", "https://x.example.com");
        let y = write_cfg(&dir_b, "y.toml", "127.0.0.1:59843", "https://y.example.com");
        // 缺 base_url → validate 必然失败（doctor 的校验覆盖项）
        let _bad = write_cfg(&dir_b, "badval.toml", "127.0.0.1:59844", "");
        std::fs::write(
            settings_path_in_tmp(root.path()),
            format!(
                r#"{{"aliases": {{"x": "{}"}}, "config_dirs": ["{}"]}}"#,
                x.display()
                    .to_string()
                    .replace(std::path::MAIN_SEPARATOR, "/"),
                dir_b
                    .display()
                    .to_string()
                    .replace(std::path::MAIN_SEPARATOR, "/")
            ),
        )
        .unwrap();
        let settings = settings::load_from(&settings_path_in_tmp(root.path()));
        let findings = check_unaliased_configs(&settings);
        // x.toml 被别名覆盖 → 不出现；y.toml 是孤儿 → 正常解析无提示；
        // badval.toml（缺端口）→ 校验失败 warning；默认 config.toml 不报
        assert!(
            !findings.iter().any(|f| f.message.contains("x.toml")),
            "被别名覆盖的文件不应被目录扫描重复审查: {:?}",
            findings
        );
        assert!(
            findings
                .iter()
                .any(|f| f.message.contains("badval.toml") && f.message.contains("校验失败")),
            "孤儿配置的校验失败应报 warning: {:?}",
            findings
        );
        assert!(y.exists());
    }

    #[test]
    fn doctor_clean_when_all_good() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_cfg(
            dir.path(),
            "a.toml",
            "127.0.0.1:59844",
            "https://a.example.com",
        );
        std::fs::write(
            settings_path_in_tmp(dir.path()),
            format!(
                r#"{{"aliases": {{"a": "{}"}}}}"#,
                a.display()
                    .to_string()
                    .replace(std::path::MAIN_SEPARATOR, "/")
            ),
        )
        .unwrap();
        let report = run(&settings_path_in_tmp(dir.path()));
        assert!(report.is_clean(), "不应有发现: {:?}", report.findings);
    }

    #[test]
    fn doctor_watchdog_field_bounds() {
        // 0 周期 / 0 容忍 → error；max_restarts=0 与过大周期 → warning
        let mut s = Settings::default();
        assert!(check_watchdog_settings(&s).is_empty());
        s.watchdog_heartbeat_secs = 0;
        assert!(
            check_watchdog_settings(&s)
                .iter()
                .any(|f| f.level == Level::Error && f.message.contains("heartbeat_secs"))
        );
        s.watchdog_heartbeat_secs = 3600;
        let f = check_watchdog_settings(&s);
        assert!(f.iter().any(|f| f.level == Level::Warn));
        s.watchdog_heartbeat_secs = 30;
        s.watchdog_stale_after_cycles = 0;
        assert!(
            check_watchdog_settings(&s)
                .iter()
                .any(|f| f.level == Level::Error && f.message.contains("stale_after_cycles"))
        );
        s.watchdog_stale_after_cycles = 1;
        s.watchdog_max_restarts = 0;
        assert!(
            check_watchdog_settings(&s)
                .iter()
                .any(|f| f.level == Level::Warn && f.message.contains("max_restarts"))
        );
    }

    #[test]
    fn find_discovers_filters_and_dedupes() {
        let root = tempfile::tempdir().unwrap();
        let cfgs = root.path().join("cfgs");
        std::fs::create_dir_all(&cfgs).unwrap();
        let a = write_cfg(
            &cfgs,
            "alpha.toml",
            "127.0.0.1:59845",
            "https://alpha.example.com",
        );
        let b = write_cfg(
            &cfgs,
            "beta.toml",
            "127.0.0.1:59846",
            "https://beta.example.com",
        );
        std::fs::write(
            settings_path_in_tmp(root.path()),
            format!(
                r#"{{"aliases": {{"alpha": "{}"}}, "config_dirs": ["{}"]}}"#,
                a.display()
                    .to_string()
                    .replace(std::path::MAIN_SEPARATOR, "/"),
                cfgs.display()
                    .to_string()
                    .replace(std::path::MAIN_SEPARATOR, "/")
            ),
        )
        .unwrap();
        let settings = settings::load_from(&settings_path_in_tmp(root.path()));

        // 只统计用户自加目录内的发现（effective_config_dirs 的默认根是真实
        // ~/.aproxy/，开发机上可能存在其他文件，不在本测试断言范围）
        let all: Vec<_> = crate::find::discover(&settings)
            .into_iter()
            .filter(|d| d.path.starts_with(&cfgs))
            .collect();
        assert_eq!(all.len(), 2, "用户目录内应恰好发现两个: {all:?}");
        // --aliased：只有 alpha
        let aliased: Vec<_> = all.iter().filter(|d| !d.aliases.is_empty()).collect();
        assert_eq!(aliased.len(), 1);
        assert_eq!(aliased[0].aliases, vec!["alpha"]);
        // --unaliased：只有 beta
        let unaliased: Vec<_> = all.iter().filter(|d| d.aliases.is_empty()).collect();
        assert_eq!(unaliased.len(), 1);
        // 关键字过滤：alpha 命中文件名与别名
        assert!(all[0].matches_query("alpha") || all[1].matches_query("alpha"));
        // 端口过滤
        assert!(all.iter().any(|d| d.matches_port(59846)));
        assert!(!all.iter().any(|d| d.matches_port(59999)));
        // b 文件不再被第二次发现（同目录去重）
        assert_eq!(all.iter().filter(|d| d.path == b).count(), 1);
    }
}
