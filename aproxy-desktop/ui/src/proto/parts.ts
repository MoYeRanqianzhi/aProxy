/*
 * 原型件的总出口：只在浏览器开发模式（vite dev）里由 App 动态导入。
 * 生产构建里导入它的那个条件是常量 false，这个模块与 proto/ 下的组件整个不进产物；
 * 桌面窗口里由真窗口提供标题栏按钮与 Mica，也用不着它们。
 */
export { default as MascotLab } from './MascotLab.svelte';
export { default as ProtoBackdrop } from './ProtoBackdrop.svelte';
export { default as ProtoFrame } from './ProtoFrame.svelte';
export { default as ReviewToolbar } from './ReviewToolbar.svelte';
