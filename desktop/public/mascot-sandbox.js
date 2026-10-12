/*
 * 自定义吉祥物的沙箱脚本（mascot-sandbox.html 里唯一的脚本）。
 *
 * 界面（MascotFrame）经 postMessage 交来两种消息：
 *   { type: 'load', code }                              吉祥物的 ES 模块源码，默认导出一个吉祥物定义（契约见 src/lib/mascot/types.ts）
 *   { type: 'render', state | icon, replay, freezeAt }  按 MascotState 渲染，或按图标参数出静态图标
 * 这里回报三种消息：ready（脚本就绪，可以交代码了）、loaded（定义加载完）、error（加载或渲染失败，附原因）。
 *
 * 为什么这样写：
 *   - 代码转成 blob: URL 再 import()，整个过程不发网络请求。不透明源的页面去加载同站的模块文件要跨源许可（CORS），
 *     blob: URL 却和本页同源；正式版的代码来自扩展文件夹，经 IPC 读出来，本来也是一段文字。
 *   - 本文件是经典脚本而不是模块，理由同上：经典脚本跨源加载不需要许可。
 *   - 渲染出的 SVG 只放在本页里，界面拿不到也不需要它；吉祥物的样式也只作用于本页。
 *   - 宿主可能在吉祥物定义加载完之前就发来 render，那时先记下最新的一条，加载完立刻补画。
 */
(function () {
  'use strict';

  /** 让吉祥物的 <svg> 填满 iframe；背景透明，透出界面。沙箱里没有界面的字体，SVG 里的文字（角标）用系统无衬线体 */
  var BASE =
    'html,body{margin:0;width:100%;height:100%;overflow:hidden;background:transparent}' +
    'body{line-height:0;font-family:system-ui,sans-serif}body>svg{display:block;width:100%;height:100%}';

  var sheet = new CSSStyleSheet();
  sheet.replaceSync(BASE);
  document.adoptedStyleSheets = [sheet];

  var def = null;
  var pending = null;
  var shownMarkup = null;
  var shownReplay = null;

  function send(msg) {
    // 父窗口是界面本身；对方的源（tauri:// 或开发服务器）这里不必知道，消息里也没有任何敏感内容
    parent.postMessage(msg, '*');
  }

  function reason(err) {
    return String((err && err.message) || err);
  }

  function load(code) {
    var url = URL.createObjectURL(new Blob([code], { type: 'text/javascript' }));
    import(url)
      .then(function (mod) {
        var d = mod && mod.default;
        if (!d || typeof d.render !== 'function' || typeof d.icon !== 'function') {
          throw new Error('default export must provide render() and icon()');
        }
        def = d;
        sheet.replaceSync(BASE + (typeof d.css === 'string' ? d.css : ''));
        send({ type: 'loaded' });
        if (pending) render(pending);
      })
      .catch(function (err) {
        send({ type: 'error', message: reason(err) });
      })
      .finally(function () {
        URL.revokeObjectURL(url);
      });
  }

  function render(msg) {
    if (!def) {
      pending = msg;
      return;
    }
    var markup = '';
    try {
      markup = msg.icon ? def.icon(msg.icon) : def.render(msg.state);
      if (typeof markup !== 'string') throw new Error('render() must return a string');
    } catch (err) {
      markup = '';
      send({ type: 'error', message: reason(err) });
    }
    // 标记不变就不动 DOM，动画不会从头播；replay 变了才强制重新挂载（重播一次性动画）
    if (markup !== shownMarkup || msg.replay !== shownReplay) {
      document.body.innerHTML = markup;
      shownMarkup = markup;
      shownReplay = msg.replay;
    }
    if (typeof msg.freezeAt === 'number') {
      document.getAnimations().forEach(function (a) {
        a.pause();
        a.currentTime = msg.freezeAt;
      });
    }
  }

  addEventListener('message', function (ev) {
    if (ev.source !== parent) return;
    var msg = ev.data;
    if (!msg || typeof msg !== 'object') return;
    if (msg.type === 'load' && typeof msg.code === 'string') load(msg.code);
    else if (msg.type === 'render') render(msg);
  });

  send({ type: 'ready' });
})();
