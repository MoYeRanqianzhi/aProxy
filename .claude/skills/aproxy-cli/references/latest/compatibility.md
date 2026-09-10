# 版本兼容性

判定本文档是否适用于用户手上的 aProxy 版本，以及跨版本操作时的行为差异。

## 版本判定方法

```
aproxy --version            # CLI 侧版本
aproxy status               # 每行 v<semver> = 各实例实际运行的守护进程版本
```

两者可能不同：status 显示的是**已在运行**的守护进程版本（升级二进制后旧实例
仍跑旧版本，直到重启）。操作某实例前以 status 显示的版本为准。

## 当前文档版本

| 项 | 值 |
|---|---|
| 文档适用版本 | **0.1.0-alpha.7**（含 alpha.4→alpha.7 引入的全部行为） |
| 代码版本坐标 | Cargo.toml `version` 字段；alpha 线于 2026-09 发布 |
| 大版本线 | 0.1.x（0.1 系列内小版本不另开目录，直接更新 latest/ 文档） |

## alpha.6 关键行为（相对 alpha.5 及更早）

对旧实例执行操作时注意这些差异：

1. **restart 与 `--force`（新命令）**：CLI 支持 `aproxy restart`（只重启不
   启动）与 stop/restart 的 `--force`（零等待强杀）。对旧版本实例 restart
   仍可工作（其 IPC shutdown 协议自 alpha 线稳定）；旧实例被 alpha.6 看护者
   重拉时也走同一路径。`--force` 对任何实例有效（纯进程操作）。
2. **看门狗（新，默认开启）**：全局看护进程自动重拉崩溃/挂死的实例。
   旧版本守护无心跳节——看护者对它退化为纯进程死亡检测（挂死不检测）。
3. **IPC v2**：响应携带 `proto` 字段与观测计数（请求/重试/最近错误）。
   status 对 v1 旧实例（alpha.5 及更早）不显示计数（显示为无观测数据）；
   CLI 的 Stats 查询对旧实例自动降级为 Ping。
4. **4xx 重试口径**（文档修正）：4xx/5xx 一律重试（`is_retryable_status`
   自原型起即为 400..=599）——此前部分文档误写「4xx 不重试」。任何版本的
   实际行为一致。

## alpha.4/alpha.5 关键行为（相对 alpha.3 及更早）

对旧实例执行操作时注意这些差异：

1. **磁盘缓存（alpha.4 新）**：alpha.4 默认开启 `disk_cache`，spool 溢写
   `~/.aproxy/spool/<端口>/`。旧实例无此目录无此行为。
2. **请求体上限放宽（alpha.4 变更）**：旧版硬编码 10 MiB（超限 413）；
   alpha.4 默认 128 MB 且可配（`max_body_mb`，0=不限）。
3. **新配置字段（alpha.4/5）**：`max_body_mb`、`disk_cache`（settings.json
   全局默认 + toml 覆盖）、`idle_timeout_secs`/`log_rotate_mb`（alpha.5）。
   旧二进制读到新字段会忽略（行为不变），新二进制读旧文件取默认（升级安全）。
4. **base_url 旧字段名**：`upstream_url` 为兼容别名，任何版本可读、保存写新名。

## 兼容性总原则

- **配置向前兼容**：新版二进制读旧配置文件/旧 settings.json → 缺字段取默认，
  不报错；旧二进制读新配置文件 → 未知字段忽略（serde 默认行为）。
- **混跑安全**：多实例可各自跑不同版本（实例独立进程独立配置）；但同一份
  settings.json 的别名表被两个版本共用，删除字段前确认没有旧实例还在读。
- **操作兼容**：status/stop/logs/restore 对旧版本实例全部有效（IPC 协议自
  alpha 线稳定）；新字段相关的 config --show 展示对旧实例无意义。

## 版本留存策略（维护者用）

- 小版本（0.1.x 内）：直接修改 `references/latest/` 下文档，不留存。
- 大版本更替（如 0.1 → 0.2、alpha → stable）：把整个 `latest/` 复制为
  `references/<旧版本号>/` 留存，再重建 latest；旧版本目录内含自己的
  compatibility.md 描述其适用范围。
- `SKILL.md` 的导航相对路径 `references/latest/` 不随版本变化。
