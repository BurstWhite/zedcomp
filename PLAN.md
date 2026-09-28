# ZedComp: Zed 版 CPH 可行性方案

> 目标:仿照 [cph](https://github.com/agrawal-d/cph),为 Zed 实现与
> [Competitive Companion](https://github.com/jmerle/competitive-companion) 配合的自动拉题扩展。
> 参考:[Zed Developing Extensions](https://zed.dev/docs/extensions/developing-extensions)

## 结论

**可行**,但因 Zed 扩展沙盒限制,架构与 VSCode 版 CPH 不同:采用
**WASM 扩展 + 原生 helper 二进制**双进程方案。CPH 的 webview 测试面板无法复刻
(Zed 扩展无自定义 UI 能力),以终端 task 替代。

## 已验证的关键事实

1. Zed 扩展编译为 `wasm32-wasip2`,使用 `zed_extension_api`(当前 0.7.x)。
   **WASM 沙盒内无法监听 TCP 端口** → 扩展不能直接接收 Competitive Companion 的 POST。
2. 扩展可通过 `language_server_command()` 启动**任意原生二进制**作为 language server
   子进程,生命周期由 Zed 管理 —— 这是启动后台 listener 的合法通道。
3. 扩展能力(capability 系统):`process:exec`(执行外部命令)、
   `download_file`(下载文件,可用于从 GitHub Releases 拉 helper)、`npm:install`。
   见 https://zed.dev/docs/extensions/capabilities
4. Competitive Companion 协议:向 localhost 固定端口发 HTTP POST,默认端口:
   `1327 (cpbooster), 4244 (Hightail), 6174 (Mind Sport), 10042 (acmX),
   10043 (Caide), 10045 (CP Editor), 27121 (CPH)`,另支持用户自定义端口。
   POST body 为 JSON,结构(实测 codeforces 样例):

   ```json
   {
     "name": "A. String Task",
     "group": "Codeforces - Codeforces Beta Round 89 (Div. 2)",
     "url": "https://codeforces.com/problemset/problem/118/A?locale=en",
     "interactive": false,
     "memoryLimit": 256,
     "timeLimit": 2000,
     "tests": [{ "input": "tour\n", "output": ".t.r\n" }],
     "testType": "single",
     "input": { "type": "stdin" },
     "output": { "type": "stdout" },
     "languages": { "java": { "mainClass": "Main", "taskClass": "AStringTask" } }
   }
   ```

   注意:抓取 contest(整场比赛)时会逐题发多个 POST。
5. `Extension` trait 可用钩子:`language_server_command`、`run_slash_command`、
   `context_server_command`、`get_dap_binary` 等。
6. Zed 扩展**无自定义 UI**(无 webview/panel)→ 判题结果走集成终端输出。
7. helper(原生进程)可调用 `zed <path>` CLI 在已打开的窗口中打开文件,
   弥补扩展 API 无法操作编辑器标签页的问题。

## 架构

```
浏览器 Competitive Companion ──HTTP POST──> helper(原生二进制,Rust)
                                              ├─ 监听 27121(可配置)接收题目 JSON
                                              ├─ 生成目录: <contest>/<problem>/{main.cpp, in1, ans1, ..., problem.json}
                                              ├─ 调 `zed` CLI 自动打开源文件
                                              ├─ 最小 LSP server(stdio,保活 + logMessage 通知)
                                              └─ 子命令:
                                                  judge  编译 + 跑全部测试点 + diff 比对
                                                  submit 包装 cf-tool / 其他 OJ CLI
                                               ▲ stdio(LSP)
Zed ──> extension(WASM, zed_extension_api) ──┘
           ├─ extension.toml + src/lib.rs
           ├─ language_server_command():返回 helper 路径(必要时先下载)
           ├─ 挂载到 C/C++/Python(附加 LS,不替代 clangd)
           ├─ LSP 设置项:端口、目录命名模板、代码模板
           └─ snippets/ 竞赛代码片段
```

## 工作流(用户体验)

1. 用户用 Zed 打开竞赛 workspace,打开任意 `.cpp` → 扩展自动拉起 helper。
2. 浏览器点 Competitive Companion 图标 → POST → helper 生成
   `codeforces/round-891/a/{main.cpp, in1, ans1, ...}`,并自动在 Zed 中打开 `main.cpp`。
3. 写题;`cmd+shift+r` 运行 task **Judge** → helper 编译运行所有测试点,
   终端输出 `Test #1: AC (12ms)` / `WA + diff`。
4. task **Submit** 调用 submit 子命令提交。

## 风险与限制

| 风险 | 影响 | 缓解 |
|---|---|---|
| helper 依赖 LS 生命周期,需先打开对应语言文件才启动 | 首次使用需引导 | 文档说明;helper 同时支持独立常驻模式(用户自启) |
| 多 Zed 窗口拉起多个 helper → 端口冲突 | 后者收不到题目 | helper 检测端口占用,退化为纯 LSP no-op 并 logMessage 提示 |
| 无自定义 UI | 无法复刻 CPH 面板 | 终端 task 输出 + 可选 markdown 报告文件 |
| WASM 内 `std::env::var`、`cfg!` 行为异常 | 已知坑 | 用 `Worktree` / `current_platform()` API |
| helper 跨平台分发 | macOS quarantine / 架构差异 | GitHub Releases 预编译 + `make_file_executable`;提供源码编译回退 |
| 27121 与真实 CPH 冲突(同装 VSCode 的用户) | 端口抢占 | 默认端口可配;Competitive Companion 支持 customPorts |

## 里程碑

- **M0 Spike(约 0.5 天)** ★ 关键验证
  - 最小扩展:注册到 C++ 的 LS,`language_server_command` 返回 helper
  - 最小 helper:监听端口 → 打印收到的 POST;回 LSP initialize
  - 验证:CC 插件 POST 可达、文件能写入 workspace、`zed` CLI 能打开文件
- **M1 helper core(1–2 天)**:CC JSON 解析、目录/文件生成、模板渲染、
  最小 LSP(initialize/shutdown/logMessage)、端口冲突处理
- **M2 extension 集成(1 天)**:extension.toml、GitHub Releases 下载/缓存/更新、
  设置项(LSP initialization options)
- **M3 judge + tasks(1 天)**:judge 子命令(编译/运行/比对/计时)、
  生成 `.zed/tasks.json`(Judge / Submit / Stress)
- **M4 打磨与发布**:submit(cf-tool 等)、snippets、README、
  按 https://zed.dev/docs/extensions/publishing-extensions 发布

总估 4–5 天。M0 通过后方案即成立。
