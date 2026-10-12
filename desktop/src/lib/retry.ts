/*
 * aProxy 的退避规则，界面与 mock 后端共用：配置页用它画「退避阶梯」预览，mock 后端用它模拟真实的等待。
 * 规则本身以 aProxy 的实现为准，这里只是同一条规则的界面侧副本。
 */

/** 前三次重试立即发出，第四次起从 5 秒翻倍，直到 max_retry_backoff_secs 封顶。返回毫秒 */
export function backoffAfter(attempt: number, capSecs: number): number {
  if (attempt <= 3) return 0;
  return Math.min(5 * 2 ** (attempt - 4), capSecs) * 1000;
}
