//! 列举实例：`aproxy status` 与界面的实例列表共用的数据。

use crate::daemon::{self, InstanceRecord, InstanceStatus};
use crate::settings;

/// 按闲置与否筛选应答了控制通道的实例。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityFilter {
    /// 不筛选
    All,
    /// 只要闲置达到阈值的
    Idle,
    /// 只要没有闲置到阈值的
    Busy,
}

/// 一次列举的结果。
#[derive(Debug, Clone)]
pub struct Listing {
    /// 应答了控制通道、并通过筛选的实例（实时状态取自 IPC ping 的应答）
    pub instances: Vec<InstanceStatus>,
    /// 筛选之前应答了控制通道的实例数：`instances` 为空时，据此区分「一个都没有」与
    /// 「有，但都被筛掉了」
    pub responsive_total: usize,
    /// 进程仍在、却不应答控制通道的实例（多半挂死）。不受筛选影响：用户最需要知道的
    /// 正是它们
    pub unresponsive: Vec<InstanceRecord>,
    /// 判定闲置用的阈值（秒），取自 settings 的 `idle_timeout_secs`
    pub idle_threshold_secs: u64,
    /// 列举时刻（Unix 秒）。闲置时长、运行时长都按它算，同一次列举里口径一致
    pub now: u64,
}

impl Listing {
    /// 这个实例已经闲置了多少秒（按列举时刻算）
    pub fn idle_secs(&self, status: &InstanceStatus) -> u64 {
        self.now.saturating_sub(status.activity.last_request_at)
    }
}

/// 列举本 home 的实例。存活以 IPC 探测为准。
pub async fn enumerate(filter: ActivityFilter) -> Listing {
    let idle_threshold_secs = settings::load().idle_timeout_secs;
    let survey = daemon::survey_instances().await;
    let now = super::now_unix();
    let responsive_total = survey.responsive.len();
    let instances = survey
        .responsive
        .into_iter()
        .filter(|status| {
            let idle = now.saturating_sub(status.activity.last_request_at) >= idle_threshold_secs;
            match filter {
                ActivityFilter::All => true,
                ActivityFilter::Idle => idle,
                ActivityFilter::Busy => !idle,
            }
        })
        .collect();
    Listing {
        instances,
        responsive_total,
        unresponsive: survey.unresponsive,
        idle_threshold_secs,
        now,
    }
}
