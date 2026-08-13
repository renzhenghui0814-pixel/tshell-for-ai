# tshell 桌面化迁移

把 VS Code 扩展 **tshell 2.2.1** 迁移为 Windows / Linux / macOS 三平台桌面应用。

迁移已完成，这份文档从此是记录而不是计划。

代码已拆成两个仓库：桌面版在这里，扩展留在原处（`D:\workspace\codex`，冻结在 2.2.1）。移植期间作参照的 `src/`、`media/` 跟着扩展走，**下文凡是指向它们的路径都在另一个仓库里**。唯一跨过边界的 `tools/export-strings.mjs` 也留在那边：`ui/vendor/strings.json` 现在就是文案本身，直接编辑，上游没有生成器了。

---

## 技术栈与决策

| 项 | 选择 | 为什么 |
|---|---|---|
| 运行时 | **Tauri v2**（系统 WebView） | 目标是轻量：包体 ~12MB、内存 ~90MB、秒开。Electron 五项指标全部垫底 |
| 后端 | **全 Rust 重写**（~8000 行 TS → Rust） | 只要包里还有 Node 运行时，包体和内存就下不到目标量级。没有捷径 |
| 前端 | **原样复用** `media/` 的 7631 行 | 页面 JS/CSS 几乎零改动，靠 shim 换掉宿主 |
| 扩展 | **砍掉**，不再维护 | 不做配置迁移，用户全新配置 |

**依赖对照**：`ssh2` → `russh` + `russh-sftp`｜`iconv-lite` → `encoding_rs`｜`node:crypto` → `keyring`（系统钥匙链）｜`fetch`/SSE → `reqwest` + `eventsource-stream`

---

## 已完成：阶段 0（可视框架）

窗口能开，四个页面能画，Rust↔WebView 通路已验证。**没有任何真实功能**——不连 SSH、不传文件、不调模型。

```
src-tauri/          Rust 侧
  src/main.rs       三个演示命令：app_info / demo_config / demo_banner
  tauri.conf.json   无边框窗口（decorations: false）
  capabilities/     窗口权限
ui/                 前端（media/ 搬迁而来）
  index.html        新写：自绘标题栏
  shell/            新写：窗口外壳，唯一的 IPC 出口
  shared/host.js    新写：acquireVsCodeApi() 的替身
  shared/theme.css  新写：64 个 --vscode-* 变量的暗/亮两套
  settings/         新写：设置页
  vendor/strings.json  从 i18n.ts 导出的 341 条中英文案
  servers/ terminal/ transfer/ chat/   原样搬迁
```

已实现的外壳能力：自绘标题栏（拖拽/最大化/最小化/关闭）、标签页、可拖拽分栏、服务器面板收起、设置页、主题实时切换、七个按钮的本地化提示。

**窗格拆分**：标签可左右并排成多列——拖标签，或右键标签选"向左/向右拆分"。列宽可拖，最窄 240px。

实现上只有一条要守的规矩：`.panes` 是一整个 CSS Grid，**所有 iframe 从创建到销毁都是它的直接子节点**，换列只改 `grid-column`。原因是 iframe 一旦在 DOM 里换父节点，浏览上下文就被丢弃重建——SSH 会重连、scrollback 会清空。**不要为了"结构清晰"把 iframe 收进每列自己的容器里。**

---

## 三条必须遵守的约定

**1. 页面契约不变。** 每个页面仍然调 `acquireVsCodeApi()`、读 `window.tshellBootstrap`、监听 `message` 事件。`host.js` 提供这三样。**这是 7631 行前端能不改就换宿主的唯一原因，不要破坏它。**

**2. 引导数据走 URL 片段，不走消息。** 页面在自己脚本顶部同步读取 bootstrap，片段在第一行代码前就可读，消息不行。

**3. 主题要同时设两处。**
- `data-theme` on `<html>` → 驱动 `theme.css` 的 CSS 变量
- `vscode-light`/`vscode-dark` class on `<body>` → VS Code 老约定，`terminal.js` 的 MutationObserver 靠它重设 xterm 主题，`highlight.css` 靠它切换代码高亮

xterm 画在 canvas 上，**CSS 变量对它无效**，必须走 JS 通道。

**4. 页面不碰 Tauri。** 页面 → 外壳（`postMessage`）→ Rust（`invoke`）。外壳是全窗口唯一的 IPC 出口。

---

## 待做：四步

每一步结束都应是"能连、能用、能看见"的完整功能。

### ~~1. 服务器配置~~ ✅ 已完成

```
src-tauri/src/
  config.rs   数据模型、归一化、原子读写 + 8 个单元测试
  secrets.rs  钥匙链，及无钥匙链时的降级
  atomic.rs   临时文件 + rename
  i18n.rs     include_str! 复用 ui/vendor/strings.json
```

配置层按设计落地：`version: 1`（未来版本拒绝读取）、密文全无、认证是 tagged enum、原子写入。`demo_config` 已删。

**三处偏离原计划，都是有意的**：

1. **i18n 没有抄进 Rust。** 341 条文案已经导出在 `ui/vendor/strings.json`，Rust 用 `include_str!` 读同一份。手抄 699 行会造出第二份副本，而两份走偏时唯一会发现的人是读到错语言的用户。
2. **明文密码仍然发给页面。** 编辑对话框要靠它回填密码框，旧扩展也是这么做的。"config.json 不含密文"约束的是**磁盘**，不是 IPC——改掉的话要么回填不了，要么"留空=不改"导致密码没法清空。
3. **解析失败不重置配置。** 旧代码遇到坏文件直接写回默认值，等于用一个逗号换掉用户所有服务器。现在是拒绝读取、原样保留、把原因显示在面板上。

**页面契约没动**，代价是 `main.rs` 里一层边界翻译：文件存 tagged `auth`，页面收到的仍是老的 `authType` + `privateKeyPath`。十几行，换 261 行前端零改动。

**新增的外壳工作**（原计划漏了）：`requestAddGroup` / `requestRenameGroup` / `requestDeleteGroup` / `requestDeleteServer` 四条消息原本由 VS Code 的 `showInputBox` / `showWarningMessage` 应答。无边框窗口两样都没有，`shell.js` 自绘了 `ask()` 和 `confirm()`——浏览器原生的那两个会冻结整个 WebView、无法跟随主题，Tauri 窗口也未必允许弹。

### ~~2. xterm 终端命令~~ ✅ 已完成

`src-tauri/src/ssh.rs`。`demo_banner` 与 `fakeInput` / `fakePrompt` 已删。

性能硬约束照做：每会话一条 `Channel`，输出走 `InvokeResponseBody::Raw`，Rust 侧攒到「静默 8ms」或「32KB」才推一次。空闲时不轮询——缓冲区为空就无限阻塞在 `read.wait()` 上，只有非空时才设截止时间。

解码器**跨 flush 复用**，这正是多字节字符跨 chunk 的解法：GB18030 的半个字符留在解码器状态里，等下一块的头部来接。

**状态与字节共用一条 channel**：字节走 `Raw`、状态走 `Json`，前端用 `payload instanceof ArrayBuffer` 区分。省掉第二条通道。状态里只放字符串表的 key，措辞归前端。

**主机密钥不校验**，与旧扩展一致，且是明确决定过的（曾实现 TOFU，评估后移除）。代价要写清楚：连接是加密的，但**没有验证加密给了谁**——能应答该地址的人都可以出示自己的密钥、转发到真服务器，中间明文读到密码和每一次击键。两个 `known_hosts` 文件都不读也不写，所以不影响命令行的 `ssh`。

将来若要恢复，合身的形状是 `Client::check_server_key` 里做检查 + 复用外壳已有的 `confirm()` 弹框，让密钥变更能在窗口内回答，而不是逼用户手工编辑文件。

**推迟**：`agentShell.ts` 的 `eraseLine` 与全屏程序检测原计划在本阶段，但它们只为助手服务（把静默步骤的回显从屏幕上擦掉），没有助手就无处可用。随阶段 4 一起做。

**未做**：会话 `snapshot`。旧扩展保留 transcript 是因为 webview 面板会被销毁重建；这里标签切换只是 `hidden`，iframe 和 xterm 的 scrollback 都还在，没有需要恢复的东西。

### ~~3. 文件传输~~ ✅ 已完成

`src-tauri/src/transfer.rs`（两侧抽象 + 传输作业）、`preview.rs`（文本分块 + DBF）。

**两侧共用一套抽象**，和原版的 `FileSource` 接口同理：传输永远是"从一侧读、往另一侧写"，搬字节的代码不该知道方向。写成 enum 而不是 trait object——实现只有两个，`async fn` 在 trait 里不支持 dyn。

**远端自带一条 SSH 连接**，不借终端那条。否则列目录会排在 shell 正忙的后面，且关任一标签都要判断另一个还需不需要连接。

**路径规则全在 Rust**：Windows 与 POSIX 对分隔符、什么算根、根之上有没有东西都不一致（`C:\` 往上是盘符列表，`/` 往上还是 `/`）。这些不该散在前端。

传输作业的三个要点：扫描必须先于搬运（进度条是承诺，不知总量就许不出来）；每个已存在的文件是一个问题，答案从对话框回来，所以走查要能在中途停下等待；取消要在大文件的拷贝**内部**被察觉，不只是文件之间。

**流式拷贝不走 `read_bytes`**——每 64KB 开一次文件会把整场传输花在往返上。SFTP 的读必须循环：一次 read 按包应答，要 512KB 一定分批回来。

**已知缺陷，从原版继承**：预览分块解码不跨块保持解码器状态，一个多字节汉字骑在 512KB 边界上会变成替换字符。页面可按任意 offset 请求任意块，保持状态就得同时保证顺序；真要修得让读取重叠几字节。

### 4. AI 助手

| 移植 | 来源 |
|---|---|
| 主循环 | `src/ai/agentLoop.ts`（2106 行，产品核心资产） |
| 端点 | `src/ai/httpProvider.ts`（reqwest + SSE 流式） |
| 策略 | `commandPolicy` / `filePolicy` / `riskScanner` / `redaction` |
| 五个 store | `chatStore` / `memoryStore` / `trustStore` / `logStore` / `skillStore` |
| 文件与传输动作 | `fileOps.ts` / `transferOps.ts` |

**好消息**：`AgentSession` 依赖注入的 `AgentDeps` 接口（`agentLoop.ts:229`），不直接碰 SSH 或 vscode。Rust 版把它做成 trait，主循环的移植就是纯逻辑翻译，不掺 I/O。

**已砍掉**：`vscode.lm`（Copilot 模型）无跨平台对应物，`vscodeLmProvider.ts` 和 `vscodeLmModelId` 不移植。发布说明要写。

---

## 行为保真：对拍测试

**风险不是"Rust 不会写"，是"翻译过去行为悄悄变了"。** 三处代价不对称：

| 风险 | 后果 |
|---|---|
| 命令危险度判定跑偏 | **安全事故**——`auto` 模式下该拦的没拦住 |
| 模型回复解析跑偏 | 助手随机失灵，难复现 |
| 终端流解码跑偏 | 中文乱码、vi/top 画面错乱 |

这些代码几乎全是**纯函数，字符串进字符串出**。做法：

1. 写 `tools/fixtures.ts`，用 TS 原版灌语料，把 `输入 → 输出` 全量 dump 成 JSON golden 文件
2. Rust 测试读**同一批 golden 文件**，输出必须逐字节相等
3. 全绿之后才删对应的 TS

覆盖：`parseActions` `repairJson` `readAction` `balancedEnd` `foldOutput` `foldSkill` `skillKeyOf` `estimateTokens` `describeExit` `describeResult` `describeMemory` `describeSkillFailure` `classify` `classifyPath` `riskScanner` `redact` `normalizeConfig` `t()`

有状态的部分（`trim()` 上下文折叠、prompt 拼装）：本地起假的 OpenAI 兼容端点回放固定 SSE，把多步任务的事件流录成快照。

阶段 2、3 需要真机对手：CI 起 `sshd` 容器，对着它跑，不要 mock。

---

## 环境注意点（这台机器）

- **盯住 C: 剩余空间**。页面文件 11.4GB 卡在 C: 上无法扩展，C: 一满，并行 rustc 就以 `0xc0000409` 反复崩溃。曾经只剩几百 MB，现在约 15GB；release 构建开 LTO 后内存峰值更高，别把它当成永远够用
- Rust 装在 `D:\rust`，`RUSTUP_HOME` / `CARGO_HOME` 已写成用户环境变量
- 构建用 `cargo build -j 2`，`[profile.dev] debug = 0`——都是为了压内存峰值
- 撞磁盘问题时临时 `set TMP=D:\tmp`
- **`rust-version` 必须跟工具链一致（当前 1.97）**。写低了会触发 cargo 的 MSRV 感知解析，把依赖锁到老版本，其中 `indexmap 2.14.0` 在新 rustc 上编不过
- 重新构建前先 `taskkill /IM tshell.exe /F`，否则 Windows 锁住 exe
- 改了 `ui/` 也要重新 `cargo build`：前端在编译期嵌进二进制
- **Rust 编译不检查前端**。改完 `ui/` 跑一遍 `node --check`

```bash
cd src-tauri && cargo build -j 2
./target/debug/tshell.exe
```

---

---

## 视觉层：方案 A「星轨」

界面已按一套自有设计系统重做，不再是 VS Code 皮肤的复刻。三个共享文件是唯一来源：

```
ui/shared/theme.css       语义令牌（两层）
ui/shared/components.css  按钮/输入/行/点/菜单/对话框/滚动条
ui/shared/icons.js        38 个图标，一份 sprite
ui/shared/schemes.js      终端外观：内置三套方案，及与用户方案的合并
```

**`theme.css` 是两层的。** 上层是语义令牌（`--bg-elev`、`--tx-dim`、`--ac`），只有它们区分深浅；下层是 64 个 `--vscode-*` 名字，作为**别名**指向上层。369 处旧引用一个没改，页面自动跟着换色。别名只写一遍而不是深浅各写一遍——它们指向令牌而非字面色，所以翻主题时自己就跟着翻了。旧版本要在两个主题里各抄一遍 64 个值，那正是会走偏的形状。

**四条硬规矩：**

1. **`--tx-faint` 不许承载文字。** 它是给图标描边、发丝线和禁用态的，对比度不到 4.5:1。要文字用 `--tx-dim`。
2. **深色下强调色和错误色上压的是深墨，不是白字**（`--ac-tx` / `--err-tx`）。亮到能在深底上当文字读的靛蓝，就亮到白字压不住——白字在 `--ac` 上只有 3.57:1，在悬停态更低到 2.98:1。浅色没这个矛盾，用白字。
3. **终端的颜色只能从 JS 喂。** xterm 画在 canvas 上，CSS 变量对它无效，所以终端配色全部走 `schemes.js` 而不是 `theme.css`——那是全项目唯一一份不在样式表里的调色板。凡是终端和窗口都要用的颜色（光标 `--term-cursor`、选区 `--term-select`），仍然只在 `theme.css` 里写一遍，两边都用 `getPropertyValue` 读，不许各抄一份。
4. **`--brand` 是唯一深浅同值的令牌**（`#F97316`，应用图标那块橙）。其余令牌都是角色名，角色可以随底色换值；这个不是角色，是 logo——任务栏里的 .ico 不会因为用户切浅色就变暗，画同一个标志的地方也不该。只在「画的就是这个标志」时用它；意思是「助手」时用 `--ai`。

**终端外观是可编辑的方案。** 内置三套（星轨 / VS Code 经典 / Solarized）在 `schemes.js` 里，各有深浅两半；用户自建的方案单套颜色，连同对内置方案的改动一起存在 `tshell.schemes.json`，由 `src-tauri/src/schemes.rs` 管版本门、hex 归一化和原子写。**合并发生在前端**——内置色板在 JS 里，xterm 也只认 JS，Rust 拿不到内置那一半。方案含十六色 + 前景/背景/光标/选区 + 字体 + 字号；后四项在内置方案里留空，留空即跟随窗口主题。字体候选由 Rust 的 `fonts.rs` 用 `fontdb` 枚举，Web 平台问不出来。

对比度有脚本可验，全部 ≥ 4.5:1，两处有意豁免并在代码里写明理由：`ansi black`（终端里它就是背景色）和 `--tx-faint`（仅非文字用途）。

**图标走 sprite。** 页面和 JS 里的 `<use href="#i-*">` 引用全部不变，换 `icons.js` 里的 symbol 定义就换了皮——和 `--vscode-*` 别名同一个手法：保住接口，换掉背后的东西。注意 `<use>` 引到的是 shadow tree，**文档 CSS 只能穿透继承属性**，所以靠 `display:none` 藏图标内部零件的写法会静默失效（侧栏图标因此改成两个图标换 `href`）。

---

## 已知偏离

- `servers.js` 的"添加分组""打开配置文件"按钮已删，功能保留在面板空白处的右键菜单
- 未绑任何快捷键。Ctrl+B 是 tmux 前缀和 readline 的光标左移，终端应用不该占用；要配得挑一个 shell 不用的组合
- 状态栏只对终端和传输标签显示连接状态。助手标签有的是「运行」而不是「连接」，两者不是一回事；设置标签什么都没有。宁可空着也不猜
