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
| 文档适用版本 | **0.1.0-alpha.4**（含 alpha.4 引入的全部行为） |
| 代码版本坐标 | Cargo.toml `version` 字段；alpha.4 于 2026-09 发布线 |
| 大版本线 | 0.1.x（0.1 系列内小版本不另开目录，直接更新 latest/ 文档） |

## alpha.4 关键行为（相对 alpha.3 及更早）

对旧实例执行操作时注意这些差异：

1. **磁盘缓存（新）**：alpha.4 默认开启 `disk_cache`，spool 溢写
   `~/.aproxy/spool/<端口>/`。旧实例无此目录无此行为。
2. **请求体上限放宽（变更）**：旧版硬编码 10 MiB（超限 413）；alpha.4 默认
   128 MB 且可配（`max_body_mb`，0=不限）。
3. **新配置字段**：`max_body_mb`、`disk_cache`（settings.json 全局默认 + toml
   覆盖）。旧版 settings.json 无这些字段——旧二进制读到会忽略（行为不变），
   alpha.4 读旧文件取默认（升级安全）。
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
