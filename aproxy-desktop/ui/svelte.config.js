import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

export default {
  // 让 <script lang="ts"> 经 Vite 的 TS 管线处理
  preprocess: vitePreprocess(),
};
