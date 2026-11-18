// 扩展「上下文失效」守卫的回归测试（2026-11-18）。
//
// ## 这个测试为什么存在
//
// 用户截到一张图：某个页面控制台里每 1.5 秒刷一条
// `Uncaught Error: Extension context invalidated`，堆栈指向 `content.js:770`
// 的 `refreshMedia()`。
//
// 根因不是"用错了 API"，而是 **扩展重载后旧 content script 还活着**：
//
// 1. 扩展被重载/更新 → 浏览器让旧的 content script 失去扩展上下文，
//    `chrome.runtime` 失效，再调用就抛 `Extension context invalidated`；
// 2. 但旧脚本注册的 **`setInterval(refreshMedia, 1500)` 照旧触发**；
// 3. 于是每 1.5 秒抛一次未捕获异常，直到标签页被关掉。
//
// 修法是给 content.js 加一层存活守卫：所有 `chrome.*` 调用走 `guarded`，
// 周期任务走 `guardedInterval`，一旦确认上下文没了就 `die()`（清定时器 + 停手）。
//
// ## 这个测试怎么跑
//
// 不启动浏览器 —— 因为要测的东西（守卫生效、定时器停掉、不再抛错）
// 全是**纯逻辑**，用 vm 沙箱造一个假的 `chrome` 更快也更可靠。
// 浏览器里"重载扩展"那一步无法在 CI 里稳定复现，那部分只能靠人工确认
// （见本仓库 README 的验证清单）。
//
// 用法：node browser/test/verify-content-guard.mjs
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const contentPath = path.join(here, "..", "extension", "content.js");
const source = fs.readFileSync(contentPath, "utf8");

const failures = [];
const ok = (cond, msg) => {
  if (!cond) failures.push(msg);
};

// --------------------------------------------------------------------
// 造一个最小 DOM/chrome 环境
// --------------------------------------------------------------------

/** 造一份假的 chrome：`alive` 为 false 时所有调用都抛 "Extension context invalidated"。 */
function makeChrome({ alive = true, onSend } = {}) {
  const sent = [];
  const chrome = {
    _setAlive(v) {
      chrome.__alive = v;
    },
    __alive: alive,
    runtime: {
      get id() {
        // 真实的 Chrome 在重载后会把 id 变成 undefined
        if (!chrome.__alive) return undefined;
        return "fake-extension-id";
      },
      get lastError() {
        return chrome.__lastError || null;
      },
      getManifest() {
        if (!chrome.__alive) throw new Error("Extension context invalidated.");
        return { version: "0.2.16" };
      },
      sendMessage(_payload, cb) {
        if (!chrome.__alive) throw new Error("Extension context invalidated.");
        sent.push(_payload);
        if (onSend) onSend(_payload, cb);
        else if (cb) cb({ ok: true });
      },
      onMessage: {
        addListener() {
          if (!chrome.__alive) throw new Error("Extension context invalidated.");
        },
      },
    },
    storage: {
      local: {
        get(_keys, cb) {
          if (!chrome.__alive) throw new Error("Extension context invalidated.");
          if (cb) cb({});
        },
        set(_v, cb) {
          if (!chrome.__alive) throw new Error("Extension context invalidated.");
          if (cb) cb();
        },
        remove(_k, cb) {
          if (!chrome.__alive) throw new Error("Extension context invalidated.");
          if (cb) cb();
        },
      },
      onChanged: {
        addListener() {
          if (!chrome.__alive) throw new Error("Extension context invalidated.");
        },
      },
    },
  };
  chrome.__sent = sent;
  return chrome;
}

/**
 * 造一份够跑完 content.js 顶层的最小事 DOM。
 *
 * 要点：`content.js` 用 `root.innerHTML = \`...\`` 建面板结构，然后靠
 * `shadow.getElementById(...)` 取里面每个节点。所以假 DOM 必须**解析 id 属性**，
 * 否则 render() 会在 `null.textContent` 上炸 —— 那是测试脚手架的问题，不是代码问题。
 */
function makeDom() {
  const intervals = [];
  let intervalSeq = 0;
  const cleared = new Set();

  class El {
    constructor(tag) {
      this.tagName = String(tag).toUpperCase();
      this.children = [];
      this.style = {};
      this.dataset = {};
      this.attrs = {};
      this.textContent = "";
      this.className = "";
      this.id = "";
      this.hidden = false;
      this._byId = new Map();
      this.classList = {
        add: () => {},
        remove: () => {},
        toggle: () => {},
        contains: () => false,
      };
    }
    set innerHTML(html) {
      // 只认 id="..." —— 够 render() 找到它要的每个节点。
      this._html = html;
      this._byId = new Map();
      for (const m of String(html).matchAll(/id="([^"]+)"/g)) {
        const node = new El("div");
        node.id = m[1];
        this._byId.set(m[1], node);
      }
    }
    get innerHTML() {
      return this._html || "";
    }
    getElementById(id) {
      if (this._byId && this._byId.has(id)) return this._byId.get(id);
      for (const c of this.children || []) {
        const got = c.getElementById ? c.getElementById(id) : null;
        if (got) return got;
      }
      return null;
    }
    appendChild(c) {
      this.children.push(c);
      return c;
    }
    append(...nodes) {
      for (const n of nodes) this.children.push(n);
    }
    prepend(n) {
      this.children.unshift(n);
    }
    insertBefore(n) {
      this.children.push(n);
      return n;
    }
    removeChild(n) {
      this.children = this.children.filter((c) => c !== n);
    }
    replaceChildren(...nodes) {
      this.children = nodes.length ? [...nodes] : [];
    }
    replaceWith() {}
    cloneNode() {
      return new El(this.tagName);
    }
    contains() {
      return false;
    }
    get firstChild() {
      return this.children[0] || null;
    }
    get lastChild() {
      return this.children[this.children.length - 1] || null;
    }
    get childElementCount() {
      return this.children.length;
    }
    scrollIntoView() {}
    remove() {
      this.removed = true;
    }
    setAttribute(k, v) {
      this.attrs[k] = String(v);
    }
    getAttribute(k) {
      return k in this.attrs ? this.attrs[k] : null;
    }
    hasAttribute(k) {
      return k in this.attrs;
    }
    removeAttribute(k) {
      delete this.attrs[k];
    }
    attachShadow() {
      const sh = new El("shadow");
      this.shadowRoot = sh;
      return sh;
    }
    querySelector() {
      return null;
    }
    querySelectorAll() {
      return [];
    }
    closest() {
      return null;
    }
    addEventListener() {}
    removeEventListener() {}
    dispatchEvent() {
      return true;
    }
    setPointerCapture() {}
    releasePointerCapture() {}
    focus() {}
    blur() {}
    getBoundingClientRect() {
      return { left: 0, top: 0, width: 48, height: 48, right: 48, bottom: 48 };
    }
  }

  const document = {
    documentElement: new El("html"),
    body: new El("body"),
    head: new El("head"),
    createElement: (t) => new El(t),
    createTextNode: () => new El("text"),
    addEventListener() {},
    removeEventListener() {},
    querySelectorAll: () => [],
    querySelector: () => null,
    getElementById: () => null,
    readyState: "complete",
  };

  const window = {
    addEventListener() {},
    removeEventListener() {},
    location: { href: "https://example.test/page", assign() {}, host: "example.test" },
    getComputedStyle: () => ({ getPropertyValue: () => "" }),
  };

  const global = {
    document,
    window,
    chrome: null,
    location: window.location,
    setTimeout: (fn) => setTimeout(fn, 0),
    clearTimeout: (id) => clearTimeout(id),
    setInterval(fn, ms) {
      // 测试不真的等 1.5s：记下回调，由用例手动触发。
      const id = ++intervalSeq;
      intervals.push({ id, fn, ms });
      return id;
    },
    clearInterval(id) {
      cleared.add(id);
    },
    requestAnimationFrame: (fn) => setTimeout(fn, 0),
    cancelAnimationFrame: (id) => clearTimeout(id),
    console: {
      // 静音：让测试输出只剩它自己的结论
      log() {},
      warn() {},
      error() {},
      info() {},
      debug() {},
    },
    Math,
    JSON,
    Date,
    URL,
    URLSearchParams,
    Set,
    Map,
    WeakMap,
    Array,
    Object,
    String,
    Number,
    Boolean,
    RegExp,
    Error,
    TypeError,
    Promise,
    parseInt,
    parseFloat,
    isNaN,
    encodeURIComponent,
    decodeURIComponent,
    Element: El,
    Node: El,
  };
  global.globalThis = global;
  global.self = global;

  return { global, El, intervals, cleared };
}

/** 在沙箱里跑 content.js，返回环境句柄。 */
function boot(chrome) {
  const env = makeDom();
  env.global.chrome = chrome;
  // content.js 里 `window` 与 `document` 是裸全局，vm 需要它们都在 global 上。
  const ctx = vm.createContext(env.global);
  vm.runInContext(source, ctx, { filename: "content.js" });
  return { ...env, ctx };
}

// --------------------------------------------------------------------
// 用例 1：上下文失效后，周期任务不再抛错，并停止自己
// --------------------------------------------------------------------
{
  const chrome = makeChrome({ alive: true });
  let env;
  let bootError = null;
  try {
    env = boot(chrome);
  } catch (e) {
    bootError = e;
  }
  ok(bootError === null, `content.js 顶层不该抛错：${bootError && bootError.message}`);

  if (env) {
    // 刚启动：两个周期任务都注册了（updateIdle 1000 / refreshMedia 1500）。
    const msList = env.intervals.map((i) => i.ms);
    ok(msList.includes(1000), `缺 updateIdle 的 1s 定时器：${JSON.stringify(msList)}`);
    ok(msList.includes(1500), `缺 refreshMedia 的 1.5s 定时器：${JSON.stringify(msList)}`);

    // 此时还活着：跑一次 refreshMedia 应当正常发出消息。
    const before = chrome.__sent.length;
    const refresh = env.intervals.find((i) => i.ms === 1500);
    refresh.fn();
    ok(
      chrome.__sent.length > before,
      "上下文正常时 refreshMedia 应当真的发消息",
    );

    // **模拟扩展重载**：上下文失效。
    chrome._setAlive(false);

    // 关键断言：再触发周期任务**不许抛错**（原来这里就是刷屏那条）。
    let threw = null;
    try {
      refresh.fn();
    } catch (e) {
      threw = e;
    }
    ok(
      threw === null,
      `上下文失效后周期任务仍然抛错（这正是用户截图里的刷屏）：${threw && threw.message}`,
    );

    // 而且它应当把自己停掉：clearInterval 被调用过。
    ok(
      env.cleared.size > 0,
      "失效后没有清理定时器 —— 会一直每 1.5 秒报一次错",
    );

    // 停掉之后再触发，也不许抛（幂等：不能因为清理本身再报一次）。
    let threw2 = null;
    try {
      refresh.fn();
      refresh.fn();
    } catch (e) {
      threw2 = e;
    }
    ok(threw2 === null, `退役后重复触发仍抛错：${threw2 && threw2.message}`);
  }
}

// --------------------------------------------------------------------
// 用例 2：`chrome` 整个不见了（最极端：扩展被卸载）
// --------------------------------------------------------------------
{
  const chrome = makeChrome({ alive: true });
  const env = boot(chrome);
  const refresh = env.intervals.find((i) => i.ms === 1500);

  // 连 chrome 都拿掉
  delete env.global.chrome;
  let threw = null;
  try {
    refresh.fn();
  } catch (e) {
    threw = e;
  }
  ok(threw === null, `chrome 不存在时周期任务抛错：${threw && threw.message}`);
}

// --------------------------------------------------------------------
// 用例 3：静态纪律 —— 业务代码里不许有裸 chrome.* 调用
// --------------------------------------------------------------------
{
  // 守卫层自身的实现（`guarded` 内部那几行）是允许直接调 chrome 的；
  // 之外的任何 `chrome.runtime.sendMessage(` / `chrome.storage.local.get(` 都是漏网。
  const guardStart = source.indexOf("/** 脚本是否还活着");
  const guardEnd = source.indexOf('document.documentElement.setAttribute("data-dlm-version"');
  ok(guardStart > 0 && guardEnd > guardStart, "守卫层的起止标记找不到，这条静态检查失效");

  // **按真实行号检查**：`body` 是把守卫层挖掉后拼起来的，行号会错位 ——
  // 第一版就踩了这个坑（报了 3 处"漏网"，其实全都包在 guarded 里）。
  const allLines = source.split("\n");
  const offenders = [];
  for (let i = 0; i < allLines.length; i++) {
    // 跳过守卫层区间
    const offset = source.slice(0, source.split("\n").slice(0, i).join("\n").length).length;
    if (offset >= guardStart && offset < guardEnd) continue;
    const line = allLines[i];
    if (/^\s*(\/\/|\*|\/\*)/.test(line)) continue;
    const isRaw =
      /chrome\.runtime\.sendMessage\s*\(/.test(line) ||
      /chrome\.storage\.local\.(get|set|remove)\s*\(/.test(line) ||
      /chrome\.(runtime|storage)\.on\w+\.addListener\s*\(/.test(line);
    if (!isRaw) continue;
    // **允许跨行包法**：上一行或本行前面出现 `guarded(() =>` / `guarded(() => {`。
    // content.js 里 onMessage 那几个监听就是这种写法（为了把长闭包包起来）。
    const prev = (allLines[i - 1] || "").trim();
    const wrapped = prev.startsWith("guarded(") || /guarded\(\(\)\s*=>/.test(line);
    if (wrapped) continue;
    offenders.push([i + 1, line.trim()]);
  }
  ok(
    offenders.length === 0,
    `守卫层之外还有裸 chrome 调用（必须走 sendMessage/storageGet/guardedInterval）：\n` +
      offenders.map(([n, l]) => `  content.js:${n}  ${l}`).join("\n"),
  );
}

// --------------------------------------------------------------------
// 用例 4：裸 setInterval 不许出现在业务代码里
// --------------------------------------------------------------------
{
  const guardStart = source.indexOf("/** 脚本是否还活着");
  const guardEnd = source.indexOf('document.documentElement.setAttribute("data-dlm-version"');
  const body = source.slice(0, guardStart) + source.slice(guardEnd);
  const offenders = [];
  body.split("\n").forEach((line, i) => {
    if (/^\s*(\/\/|\*|\/\*)/.test(line)) return;
    // 允许 guardedInterval 内部那一次（它在守卫层里，不在 body 范围内）
    if (/(?<!guarded)\bsetInterval\s*\(/.test(line)) offenders.push([i + 1, line.trim()]);
  });
  ok(
    offenders.length === 0,
    `业务代码里还有裸 setInterval（扩展重载后会继续触发并报错）：\n` +
      offenders.map(([n, l]) => `  content.js:${n}  ${l}`).join("\n"),
  );
}

// --------------------------------------------------------------------
if (failures.length) {
  console.error(`extension content guard: FAILED (${failures.length})`);
  for (const f of failures) console.error("  - " + f);
  process.exit(1);
}
console.log("extension content guard ok: 定时器受守卫、失效即退役、无裸 chrome 调用");