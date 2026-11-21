//! 窗口的应用命令。自行从请求体解码，拒绝结构固定，不让框架把含密钥的非法值写进错误文字。
use aproxy_ipc::{
    desktop::{Fault, R},
    view::HomeView,
};
use serde::{Deserialize, de::DeserializeOwned};
use std::sync::Arc;
use tauri::{
    State,
    ipc::{InvokeBody, Request},
};

fn args<T: DeserializeOwned>(req: &Request<'_>) -> R<T> {
    let InvokeBody::Json(json) = req.body() else {
        return Err(Fault::DesktopBadInput {});
    };
    T::deserialize(json).map_err(|_| Fault::DesktopBadInput {})
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

/// 唤醒现有发现任务；只排队，不在窗口线程上读磁盘或连接实例。
#[tauri::command]
pub(crate) fn rescan(request: Request<'_>, view: State<'_, Arc<HomeView>>) -> R<()> {
    let _: Empty = args(&request)?;
    view.rescan();
    Ok(())
}
