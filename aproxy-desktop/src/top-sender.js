// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0 OR MIT
// WebView2 会把初始化脚本装进子文档。只在受信的顶层入口创建带密钥的发送闭包，
// 不能先装默认发送器再删除；发送器本身的 toString 也不能包含密钥的字面值。
;(function () {
  if (window !== window.top || location.origin !== 'http://tauri.localhost' ||
      (location.pathname !== '/' && location.pathname !== '/index.html') || location.search) return
  const key = __INVOKE_KEY__
  let fallback = false
  // 与固定版本的 Tauri 使用同一序列化约定，特别是通道和二进制结果。
  function encode(value) {
    if (value instanceof ArrayBuffer || ArrayBuffer.isView(value) || Array.isArray(value))
      return ['application/octet-stream', value]
    const data = JSON.stringify(value, (_k, v) => {
      if (v instanceof Map) return Object.fromEntries(v.entries())
      if (v instanceof Uint8Array) return Array.from(v)
      if (v instanceof ArrayBuffer) return Array.from(new Uint8Array(v))
      if (v !== null && typeof v === 'object' && '__TAURI_TO_IPC_KEY__' in v) return v.__TAURI_TO_IPC_KEY__()
      return v
    })
    return ['application/json', data]
  }
  function send({cmd, callback, error, options, payload}) {
    if (fallback) {
      window.ipc.postMessage(encode({cmd, callback, error, options:{...options, customProtocolIpcBlocked:true},payload,__TAURI_INVOKE_KEY__:key})[1])
      return
    }
    const [type, body] = encode(payload)
    const headers = new Headers(options && options.headers)
    headers.set('Content-Type',type); headers.set('Tauri-Callback',callback)
    headers.set('Tauri-Error',error); headers.set('Tauri-Invoke-Key',key)
    fetch(window.__TAURI_INTERNALS__.convertFileSrc(cmd,'ipc'),{method:'POST',body,headers}).then(async r => {
      const id = r.headers.get('Tauri-Response') === 'ok' ? callback : error
      const mime = (r.headers.get('content-type') || '').split(',')[0]
      return [id, await (mime === 'application/json' ? r.json() : mime === 'text/plain' ? r.text() : r.arrayBuffer())]
    }).then(([id,value]) => window.__TAURI_INTERNALS__.runCallback(id,value), () => {
      fallback = true
      // 丢失应答不等于 Rust 未收到写入。让本次调用报告不确定，绝不通过备用传输重发副作用。
      if (['extension_start','extension_stop','extension_restart','extension_install','extension_answer'].includes(cmd)) {
        window.__TAURI_INTERNALS__.runCallback(error,{code:'desktop_unavailable',details:{}})
      } else send({cmd,callback,error,options,payload})
    })
  }
  Object.defineProperty(window.__TAURI_INTERNALS__,'postMessage',{value:send})
})()
