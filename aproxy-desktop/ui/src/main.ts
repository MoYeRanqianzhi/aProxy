import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { startShell } from './lib/shell';

// 桌面窗口以 visible:false 创建：最先让它显示出来，后面挂载界面时出什么错都不该让窗口一直不出现
// （浏览器里什么也不做）
startShell();

const app = mount(App, { target: document.getElementById('app')! });

export default app;
