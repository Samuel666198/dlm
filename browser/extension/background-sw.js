// 稳定的 Service Worker 入口：实现文件带版本号 import。
// Chrome 会把未打包扩展的 SW 脚本缓存到 profile 里，只改文件内容、重启浏览器也可能仍跑旧代码；
// 把版本号放进 import URL，升版本就换 URL，强制重新取实现。
importScripts(`background-impl.js?v=${chrome.runtime.getManifest().version}`);
