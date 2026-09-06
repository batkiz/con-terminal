# Windows 会话恢复与 ConPTY 历史噪声 —— 调查结论

> 状态：历史实验记录，产品决策未定；本文结论基于当时样本，不代表已在 beta.95 重新验证。
> 本层仅提议 Windows 默认关闭文本恢复；显式配置保留，macOS/Linux 默认值不变。
> 原始按键转录实验 `7669fdd` 暂不移植：无法准确还原行编辑、历史选择、TUI 与不回显输入；新版 `send_key` / `paste_text` 路径也已变化。
> 日期：2026-08-13
> 范围：con-terminal Windows（libghostty-vt + ConPTY + D3D11）

## 一句话结论

会话「保存 / 恢复」的整条数据链路本身没有问题；现象的根源是 **Windows ConPTY 会把控制台行级重绘 / 重放作为新的事件喂给 VT 解析器**，导致历史里充满「显示过程」而不是「显示结果」——PSReadLine 补全重绘、裸 `\r` 重写、横幅/提示符整屏重放都会成为一行行历史。无论从字节流捕获还是从 libghostty-vt 网格捕获，污染都在；完整 libghostty 在 Windows 上不可用，ConPTY 也没有替代品。业界参照（Alacritty / Windows Terminal / kero 默认值）均不恢复终端画面内容。

## 现象时间线

1. 需求：重启后能看到上一会话的 shell 画面内容。
2. 重启后「旧会话残留可见，但一滚动就只剩提示符」。
3. 诊断逐层下钻：
   - 单元级验证（VtScreen 喂 576 行 + 滚动）：**libghostty-vt 的 scrollback 与滚动视口 API 正常**（`total=1101`，滚动后可取回历史行）。
   - 渲染层：滚动后每一帧的内容与 scrollbar offset 相符（`RENDER_VIEW` 日志逐帧正确），**渲染不是问题**。
   - 数据统计：某次保存的 session.json 中 589 行里 504 行是空白、68 行是提示符类、只有 17 行真实内容——「滚动后只剩提示符」是**保存内容本身如此**。
   - 命令粘连（`codex --profile… cd ~/…` 挤在一行）：ConPTY 用裸 `\r` 原地重绘命令时，字节流里没有换行，保存与恢复原样还原为粘连。
   - 切到「逐页网格采集」（等价 kero 的 grid snapshot 思路）后，新文件**仍有**横幅三连与重复输出——说明噪声在 `Vt`（乃至 ConPTY 重放）里就是如此：
   历史「脏」在采集点之前。
4. ConPTY 重定向陷阱（值得记录）：若 con 进程以 stdout/stderr 被重定向的方式启动（调试器等），PowerShell 横幅/输出会打到继承的 stdout 上而非 ConPTY，pane 只收到约 144 字节——与本问题无关，但会污染探针实验。

## 根因总结

- ConPTY 伪控制台基于「写入序列」工作；宿主 console 在编辑/Resize/window 事件时会**重放**整行/整屏状态，这些重放以新输出进入解析器。
- 我们之前「字节 transcript」和「网格采集」都从同一个被污染的 VT 状态取数，所以二者结论一致：**没有干净的源头可抄**。
- 完整 libghostty（含 Surface/渲染/终端模型）目前随 macOS 绑定（Metal/AppKit）；Windows 版上游移植尚未形成一个可嵌入的 libghostty，当前 con 用的 `libghostty-vt` 只是解析+网格核心，没有上游的「终端视图状态」清洗层。
- ConPTY（`CreatePseudoConsole`）是 Windows 10 1809+ 唯一受支持的原生伪终端 API；Windows Terminal、VS Code、Alacritty 在 Windows 上无一例外使用，没有替代品。

## 参考实现（kero：macOS + 完整 libghostty / alacritty_terminal）

- 统一表面协议 `TerminalBackendSurface`：
  - `readVisibleText`：视口网格快照
  - `exportScreenFile()` / `exportScrollbackFile()`：导出整体为 styled VT 流
  - `scroll(toFraction:)`、`clearScreen()`、find…
- Alacritty 后端通过 Rust bridge（`alacritty-bridge`）的 `kero_alacritty_snapshot` / `kero_alacritty_buffer_text` 直接从**网格**序列化 styled VT，不是从 PTY 字节流。
- 历史保存：`TerminalHistoryStore`（`terminal-history.json`，按 session id 存 normalize 后的 VT）；恢复时写回 + 「Session Contents Restored」分隔线。
- 默认 `terminal.restore-history = false`。
- Alacritty 本身（Windows 也只用 ConPTY）**没有会话恢复功能**；它在说明中明确历史交给 shell 自己。

## 当时工作树中的未提交实验（历史记录）

| 改动 | 说明 | 取舍建议 |
| --- | --- | --- |
| renderer 滚动强制全帧重绘（`last_scroll_offset`）| 滚动时禁止脏行增量 | 可保留（防御性） |
| transcript 折叠/去重/提示符切分（`collapse`/`split`/三连去重） | 字节层清洗 | 若选方案 A/C 可大幅删除 |
| `VtScreen::capture_scrollback_text` + pane_tree 走网格 | 思路正确但不能去根 | 方案 C 保留，方案 A 删除 |
| `con-scroll-debug.log` 调试钩子 | 诊断用 | 必须移除 |

以上状态记录于 2026-08-13，不描述本 PR 工作树。

## 决策选项（产品待定）

- **A. 默认关闭终端文本恢复**（= Windows 生态、Alacritty、kero 默认做法）：打开永远是新 shell；命令历史依赖 PSReadLine / shell 自身。
- **B. 保留恢复，接受残留噪声**：继续字节清洗 + 上限截断（副作用：命令仍可，但显示不全）。
- **C. 恢复「当前可见一屏 + 分隔线」**：打开后能回到上一「最后一屏」，滚动历史从新会话开始积累；改动量和代码面最干净的折中。

## 建议

选 A 最简单且符合行业现状；若要保留「把上一次看到的界面装过来」的直觉体验，选 C（在采集状态 + 恢复时带分隔线）。两种都应：默认关闭/移除字节 transcript 相关代码，清掉调试钩子，再提交。等上游 Ghostty Windows 完整版及其终态模型可用时再做「完整恢复」。

## 附录：本轮涉及文件/证据位置

- con-terminal：`crates/conghostty/src/windows/{host_view,render/vt}.rs`、`con/app/src/pane_tree.rs`、`con-core/src/session.rs`
- kero 参考：`C:\code\kero\kero\TerminalHistory.swift`、`TerminalBackend.swift`、`Alacritty/AlacrittyTerminalView.swift`、`Vendor/alacritty-bridge`
- Alacritty README：Windows ConPTY（Win10 1809+）
- 实测样本：`%APPDATA%\con-terminal\session.json` 多轮内容与统计
