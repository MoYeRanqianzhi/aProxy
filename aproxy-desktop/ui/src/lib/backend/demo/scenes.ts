/*
 * 截图场景：桌面窗口以 APROXY_DESKTOP_DEMO=<语言>,<深浅色>,scene=<名字> 启动时（只有 debug 构建认这个开关，
 * 见 aproxy-desktop/src/lib.rs 的 demo_script），页面载入演示数据以后按名字摆好一个画面：打开哪一页、展开哪个
 * 抽屉或浮层、滚到哪里。截图工作流（probe/screenshots 分支的 .github/screenshots/）对每个场景各开一次窗口来截，
 * 场景名的清单写在那边；这里认不出的名字什么也不做，窗口停在总览，截出来一眼就看得出来。
 *
 * 步骤照用户的操作来：改地址栏 hash 换页（与 App.svelte 的 hashchange 监听同一条路），点一行、点一个标签页，
 * 打开命令面板再输入几个字。要点的元素等它出现再点（最多等几秒），因为数据与动画都是异步的。
 * 名字只由 Rust 放进来，而且只含小写字母、数字与连字符，这里只拿它查表。
 */
import { overlays } from '../../overlays.svelte';
import { demoControls } from './index';

type Step = () => Promise<void>;

const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** 等选择器的第 index 个元素出现；超时返回 null，场景就停在当时的样子 */
async function find(selector: string, index = 0, timeout = 6000): Promise<HTMLElement | null> {
  for (const end = Date.now() + timeout; Date.now() < end; await wait(100)) {
    const el = document.querySelectorAll<HTMLElement>(selector)[index];
    if (el) return el;
  }
  console.warn(`[scene] not found: ${selector} #${index}`);
  return null;
}

/** 换页：等页面里出现这一页的内容再往下走 */
const go =
  (hash: string): Step =>
  async () => {
    location.hash = hash;
    await wait(900);
  };

const click =
  (selector: string, index = 0): Step =>
  async () => {
    (await find(selector, index))?.click();
    await wait(700);
  };

/** 把元素滚到内容区的顶上（设置页、配置页的某一块） */
const reveal =
  (selector: string, index = 0): Step =>
  async () => {
    (await find(selector, index))?.scrollIntoView({ block: 'start' });
    await wait(500);
  };

/** 内容区滚到底 */
const toBottom: Step = async () => {
  const page = await find('.page');
  if (page) page.scrollTop = page.scrollHeight;
  await wait(500);
};

/** 打开日志里第一条「重试以后救回」的请求（没有就第一条），tab 是抽屉里第几个标签页（0 概览） */
const openLog =
  (tab = 0): Step =>
  async () => {
    await find('.row .hit');
    const rows = [...document.querySelectorAll<HTMLElement>('.row')];
    const row = rows.find((r) => r.querySelector('.res.rescued')) ?? rows[0];
    row?.querySelector<HTMLElement>('.hit')?.click();
    await wait(800);
    if (tab > 0) await click('aside.drawer [role=tab]', tab)();
  };

/** 打开命令面板并输入查询 */
const palette =
  (query: string): Step =>
  async () => {
    overlays.palette = true;
    const input = (await find('.palette input')) as HTMLInputElement | null;
    if (input) {
      input.value = query;
      input.dispatchEvent(new Event('input', { bubbles: true }));
    }
    await wait(600);
  };

const scenario =
  (name: 'normal' | 'empty' | 'error'): Step =>
  async () => {
    demoControls.setScenario(name);
    await wait(600);
  };

const SCENES: Record<string, Step[]> = {
  overview: [go('#/overview')],
  'overview-lower': [go('#/overview'), toBottom],
  instance: [go('#/instance/12346')],
  'instance-restore': [go('#/instance/12360')],
  logs: [go('#/logs')],
  'log-detail': [go('#/logs'), openLog(0)],
  'log-request': [go('#/logs'), openLog(1)],
  'log-response': [go('#/logs'), openLog(2)],
  'log-format': [go('#/logs'), openLog(3)],
  config: [go('#/config/12345')],
  update: [go('#/update')],
  settings: [go('#/settings')],
  'settings-packages': [go('#/settings'), reveal('section.panel.group', 2)],
  pulse: [go('#/ext/io.github.lin.request-pulse/pulse')],
  palette: [go('#/overview'), palette('12346')],
  'start-menu': [go('#/overview'), click('.start-menu > button')],
  empty: [scenario('empty'), go('#/overview')],
};

/** 演示模式下按场景名摆好画面。等界面挂好、第一批数据到了再开始 */
export async function playScene(name: string): Promise<void> {
  const steps = SCENES[name];
  if (!steps) {
    console.warn(`[scene] unknown scene: ${name}`);
    return;
  }
  await find('.page', 0, 10_000);
  await wait(1200);
  for (const step of steps) await step();
}
