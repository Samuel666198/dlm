# 扩展悬浮球点击失效：三轮排障与修复（0.2.9 – 0.2.11）

**一句话结论**：不是事件传播问题。`render()` 里两处「按有没有内容决定渲染」的代码
（`host.hidden = !any` 与 `if (!any) return;`）导致页面上没检测到下载时，悬浮球既被
`display:none` 藏掉、面板的 `hidden` 也永不更新 —— 于是点球没反应，只有点页面里的
下载链接（会让 `any` 变 true）面板才冒出来。

| 项 | 值 |
|---|---|
| 涉及文件 | `browser/extension/content.js`、`browser/test/verify-ball-composed-events.mjs` |
| 最终版本 | 扩展 **0.2.11** |
| 回归测试 | `node browser/test/verify-ball-composed-events.mjs` |
| 截图 | `2026-11-10-ext-0211-ball.png`（真实页面上面板展开，v0.2.11） |

> 三轮里我错了两次才定位到真因。文末「复盘」记了每次错在哪，接手时值得先读，
> 因为它解释了为什么当时所有自动化测试都是绿的而用户始终点不开。

---
## 第一轮：误判为站点拦截事件（0.2.9，诊断有误）

> **注意：这一轮的诊断是错的。** window 捕获改造没有解决问题，真正原因见第二轮。
> 改造本身无害且更健壮，予以保留（站点拦 document 事件确实会让挂在元素上的监听器失效）。

用户实测反馈：**点悬浮球没反应，只有点页面里的下载链接才会展开面板，而展开后按钮一个都点不动。**

### 根因

不是配色改坏的，是更早就存在的结构问题 —— 而且**一直没被测出来**。

`content.js` 把点击监听挂在**元素自己身上**（球、折叠、标签页、任务按钮）。
悬浮球与面板住在页面的 shadow DOM 里，事件从真实目标冒泡。站点只要在
document 捕获阶段 `stopPropagation()`，事件就到不了 shadow DOM，于是球点不开、
面板按钮全哑。而"点链接会展开面板"是因为那条路走的是我们自己的
`onDocumentClick`（同样在 document 捕获，但它本身就在那一站）。

**为什么旧测试没发现**：`verify-widget-ball.mjs` 用 `element.click()`
程序化触发 —— 只发一个 `click`，绕过整条指针链路，所以永远测不出真实点击的问题。

### 实测复现（CDP 真实输入事件）

| 页面 | 球点击开面板 |
|---|---|
| 干净页 | ✅ true |
| 站点在 document 捕获拦 `click` | ❌ false |
| 再拦 `pointerdown`/`pointerup` | ❌ false |

### 修复

把 host 内**所有**点击统一到 **window 捕获阶段**路由（`onWindowClick` +
`runHostAction`）。`window` 是捕获路径上 `document` 的上一站，任何 document
级拦截都盖不住它。

- 移除球、折叠、本页、复位、确认、标签页的元素级监听，按 id / `data-*` 统一路由
- 动态生成的「停止」「下载」按钮改用 `data-job` / `data-dl-url` 承载参数
- 顺手修掉一个竞态：拖拽结束后 `suppressClick` 原本用 `setTimeout(0)` 清零，
  常抢在 `click` 到达之前就清了，导致拖完反而开面板。改成只置位、由随后的
  `click` 消费，下一次 `pointerdown` 兜底清零

### 回归测试

新增 `browser/test/verify-ball-composed-events.mjs`：三种页面（干净 / 拦 click /
拦 pointer）各跑一遍，用 `Input.dispatchMouseEvent` 点**真鼠标**，断言
球能开面板、标签能切换、「×」能关面板。

```
clean:           球点击开面板=true  标签切换=true  折叠按钮=true
click-blocked:   球点击开面板=true  标签切换=true  折叠按钮=true
pointer-blocked: 球点击开面板=true  标签切换=true  折叠按钮=true
```

旧验收 `verify-widget-ball.mjs` 仍然通过（拖动、位置记忆、闲置淡出、面板方向）。

### 残留限制

拖动仍然靠挂在球上的 `pointerdown`。站点若连 `pointerdown` 都拦，
**拖不动**（但能点开，因为点击走 window 捕获）。要一并修需把指针链路也搬到
window 捕获，代价是 `setPointerCapture` 的时序要自己兜。
## 第二轮：空页面上 render() 提前返回（0.2.10）

上一节（附二）的 window 捕获改造没有解决用户的问题。用户反馈「哪个网站都不行」，
这条信息推翻了「站点拦截事件」的判断。重新调查后找到真正原因，与事件传播无关。

### 根因

```js
function render() {
  ...
  const any = state.items.length > 0 || anyActive || state.finished.length > 0 || !!state.confirm;
  host.hidden = !any;
  if (!any) return;                                              // ← 提前返回

  shadow.getElementById("dlm-panel").hidden = state.collapsed;   // ← 永远执行不到
  ...
}
```

页面上**没检测到任何下载**时 `any === false`，`render()` 直接返回，`panel.hidden`
永远不会被更新。于是点悬浮球 → `togglePanel()` → `openPanel()` → `state.collapsed = false`
→ `render()` 提前返回 → 面板仍未显示。

而点页面里的下载链接会写入 `state.confirm`，`any` 变 true，`render()` 才会走到那两行 ——
这就是「**只有点下载面板才会展开，展开后按钮也点不动**」的全部原因：
按钮不是点不动，是面板本来就是靠 `state.confirm` 那条路才显示出来的。

### 为什么一直没被发现

1. 所有验收脚本的测试页都带可下载链接，扩展会识别成 `state.items`，`any` 恒为 true，
   恰好绕过这个分支。
2. `:host { all: initial; display: block; }` 显式设置了 `display`，覆盖了
   UA 样式表里 `[hidden] { display: none }` 的效果，所以 `host.hidden = true` 并不会
   真的把球藏起来 —— 球一直可见，掩盖了「无内容时整块渲染被跳过」的事实。

### 修复

把面板开合的两行移到 `host.hidden = !any;` 之前，并删掉 `if (!any) return;`。
面板是否可见只由用户点击（`state.collapsed`）决定，与有没有内容无关；
各分区渲染函数本来就会自己处理空列表。

### 验证（在用户的真实浏览器 Tabbit 里）

用 CDP 反解出屏幕↔视口映射（`屏幕 = 视口*1.5 + (23, 318.5)`，150% 缩放），
再用 `agent-desktop` 发**操作系统级真实点击**打中球心：

```
A) OS 物理点击 (1700,1217) → 面板打开 = true
B) CDP 点击 (视口 1118,599) → 面板打开 = true
```

回归测试 `verify-ball-composed-events.mjs` 的测试页已改成**不放任何可下载链接**
（原来的链接页正是掩盖 bug 的原因），三种页面全绿：

```
clean:           球开面板=true  标签切换=true  折叠按钮=true
click-blocked:   球开面板=true  标签切换=true  折叠按钮=true
pointer-blocked: 球开面板=true  标签切换=true  折叠按钮=true
```

### 教训

诊断「点了没反应」时，不要停在「事件没收到」的假设上。这次的两次误判分别是
（1）归因于站点事件拦截，（2）用 CDP 合成事件当成真实点击的等价物。
真正有效的是把状态一层层打印出来 —— 标记显示路由命中、`openPanel` 执行、
`collapsed=false`，但 `any=false` 且 `panel.hidden` 仍为 true，问题就此定位。
## 第三轮：悬浮球被整块 display:none 藏掉（0.2.11）

附三修掉「空页面上 render() 提前返回」之后，在真实页面上仍然看不到球。
用 CDP 读计算样式，拿到了决定性的一行：

```
{ host: true, hostHidden: true, hostDisplay: "none", ballRect: { x:0, y:0, w:0, h:0 } }
```

`render()` 里还有一句：

```js
host.hidden = !any;   // any 为 false 时 → display:none
```

页面上没检测到任何下载时，整个宿主元素被设成 `display:none`，**悬浮球直接消失**，
用户连点都点不到。之前以为 `:host { display: block }` 会盖住 `[hidden]` 的
`display:none`，实测并不会 —— 真机上 `getComputedStyle(host).display` 就是 `"none"`。

恒驻的悬浮球不该按「有没有内容」决定存亡。闲置状态本来就由透明度下降表达，
不需要消失。删掉这一行。

### 两处修改合起来才完整

| 版本 | 改了什么 | 效果 |
|---|---|---|
| 0.2.10 | 面板开合两行移到提前返回之前，删掉 `if (!any) return;` | 球可见时，点它能开面板 |
| 0.2.11 | 删掉 `host.hidden = !any;` | 没内容时球也在，点得到 |

只做 0.2.10 时，在「无内容」页面上球根本不可见，用户看到的现象仍然是「没反应」。

### 验证

真实页面（`http://samueldemac-mini:7863/panel/#logs`，SPA 控制台）：

```
{ host: true, hostHidden: false, hostDisplay: "block", ballRect: { x:1094, y:575, w:48, h:48 } }
```

OS 物理点击球心（映射 屏幕 = 视口*1.50 + (23,266)）：

```
面板起始 hidden=true  → 点击后 面板打开 = true
```

截图见 `docs/evidence/2026-11-10-ext-0211-ball.png`：面板在真实页面上展开，
显示「本页」、三个标签、重置位置、v0.2.11。

回归测试新增两条断言，钉住这次的教训：

```
host display 必须是 block（不能是 none）
球尺寸必须 > 0（能点到）
```

三种页面全绿，球尺寸均为 48px。

### 三轮误判的复盘

同一个问题我错了三次，值得记下来：

1. **归因于站点拦截事件** —— 用 CDP 合成事件测出「干净页能开」，就推断是被站点
   `stopPropagation` 干掉了。实际上 CDP 合成点击和真实点击并不等价，而且我的测试页
   带可下载链接，`any` 为 true，恰好绕过了故障分支。
2. **加了 window 捕获路由** —— 改造本身无害（更抗站点干扰），但完全没碰到真因。
3. **以为 `:host{display:block}` 覆盖了 `[hidden]`** —— 没有实测就下结论，实际是
   `display:none`。

真正管用的是**把状态一层层打出来**：路由命中 → `openPanel` 执行 → `collapsed=false`
→ 但 `any=false` 且 `panel.hidden` 仍为 true。以及**直接读计算样式**，
`getComputedStyle(host).display === "none"` 一量就结案。

教训：验收脚本的测试页不能只用「有内容」的情形；`hidden` 属性在自定义元素上
是否生效必须实测；合成输入不能当作真实输入的等价物。
