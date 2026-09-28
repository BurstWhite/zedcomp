# zedcomp-helper

原生 helper 二进制,供 Zed 扩展 **ZedComp** 使用。它做两件事:

1. **收题**:以最小 LSP server 的形式被 Zed 拉起(stdio),同时在
   `127.0.0.1` 上监听 [Competitive Companion](https://github.com/jmerle/competitive-companion)
   发来的 HTTP POST,按 OJ/比赛/题号生成工作目录。
2. **判题**:`judge` 子命令编译 `main.cpp`,跑全部测试点并比对答案。

无需任何第三方运行库;HTTP server 与 LSP 帧解析都是手写的,唯一依赖是
`serde_json`。

## 构建

```bash
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
cd helper
cargo build --release          # 产物:target/release/zedcomp-helper
cargo test                     # 单元测试(URL 解析 / 答案比对 / 生成 / LSP 帧)
```

## 用法

```
zedcomp-helper                     # serve:stdio LSP + HTTP 监听(被 Zed 拉起时用)
zedcomp-helper serve               # 同上,显式写法(可手动常驻)
zedcomp-helper judge <dir>         # 判题
zedcomp-helper judge <dir> -t 3000 # 覆盖时间限制(ms)
zedcomp-helper --version
zedcomp-helper --help
```

### serve

* **HTTP**:只处理 `POST /`(即 Competitive Companion 的目标),成功返回 `200`
  空 body;额外支持浏览器/curl 的 `OPTIONS` 预检(`204`)与
  `Expect: 100-continue`(立即回 `100 Continue`,大 body 不会被拖慢)。其余路径/
  方法返回 404/405。请求体上限 64 MiB。接受连接后强制切回阻塞模式(macOS 上
  `accept` 会继承监听套接字的 `O_NONBLOCK`,否则分包到达的 body 会误报 EAGAIN)。
* **端口**:默认 `27121`,可用环境变量 `ZEDCOMP_PORT` 覆盖;LSP `initialize` 的
  `initializationOptions.port` 优先级最高(监听线程在 250 ms 内自动重新绑定)。
  端口被占用时 stderr 打印日志并**退化为纯 LSP(no-op)**,进程不退出,端口空出
  后会自动重试绑定。
* **LSP**:处理 `initialize`(回复 `{"capabilities":{}}`)、`initialized`、
  `shutdown`、`exit`;其他请求回 `null`,其他通知忽略。`initialized` 之后会通过
  `window/logMessage` 推送收题结果。
* **工作目录**:`ZEDCOMP_WORKSPACE` 环境变量优先,其次 LSP `initialize` 的
  `initializationOptions.workspaceRoot` / `workspaceRoot`、`rootUri`(file URI,
  支持 `%XX` 解码)、`rootPath`、`workspaceFolders[0].uri`;都没有时退化为进程
  当前目录(手动常驻场景)。
* **模板**:生成 `main.cpp` 用的模板按以下优先级解析,全部可选,取第一个可用者:

  1. LSP `initialize` 的 `initializationOptions.templatePath`(文件路径,读文件内容,
     支持 `~/` 展开);
  2. `initializationOptions.template`(内联字符串);
  3. `$ZEDCOMP_CONFIG_DIR/template.cpp`,默认 `~/.config/zedcomp/template.cpp`
     (`ZEDCOMP_CONFIG_DIR` 覆盖的是**目录**,不读 `XDG_CONFIG_HOME`);
  4. 内嵌默认模板(见 `src/template.rs` 的 `CPP_TEMPLATE`)。

  占位符:`{{PROBLEM_NAME}}`、`{{URL}}`、`{{CONTEST}}`、`{{PROBLEM_ID}}`、`{{OJ}}`;
  缺失值替换为空串,替换值中的换行会被压平成空格。`templatePath` 非法/不可读时
  stderr 打警告并回退到下一级,不会 panic。模板在每次收题时重新解析(改了第 3 级
  的文件无需重启 helper),`initialized` 之后的 `window/logMessage` 会报告实际
  使用的模板来源。

收到 POST 后生成:

```
<workspace_root>/<oj>/[<contest>/]<problem>/
├── main.cpp          # 按上面的优先级渲染模板,已存在则绝不覆盖
├── in1, ans1, in2, ans2, ...   # 来自 tests 数组,每次重新生成
└── problem.json      # 原始 POST body,逐字节原样保存
```

* `<oj>`:`cf`(codeforces.com)/ `ac`(atcoder.jp)/ `luogu`(luogu.com.cn);
  其他站点退化为 host 首段(如 `vjudge`)。
* URL 形态:codeforces `/problemset/problem/<cid>/<letter>`、`/contest/<cid>/problem/<letter>`、
  `/gym/<cid>/problem/<letter>`;atcoder `/contests/<cid>/tasks/<task_id>`;
  luogu `/problem/<PID>`、`/contest/<cid>/problem/<PID>`。
* luogu 的裸 `/problem/P1000` 没有比赛段,因此目录是两段 `luogu/P1000`;有比赛段时才
  是 `luogu/<contest>/<PID>`。
* 同一题目重复收题时,多余的旧 `inK/ansK`(`K > 本次测试点数`)会被删除。
* 写完后尝试 `zed <main.cpp 绝对路径>`(失败忽略;可用 `ZEDCOMP_ZED_CLI` 指定
  CLI,或 `ZEDCOMP_NO_OPEN=1` 禁用;macOS 下会回退到
  `/Applications/Zed.app/Contents/MacOS/cli`)。

### judge

```bash
cd codeforces/118/A
zedcomp-helper judge .
```

1. 读取 `problem.json` 取 `timeLimit`(ms,缺省 2000;可用 `-t/--time-limit` 覆盖)。
2. 编译:`g++ -std=c++17 -O2 -o .main main.cpp`(工作目录 = 题目目录)。
   编译器可用 `ZEDCOMP_CXX` 覆盖(例如 macOS 上 `g++-14` 或 `clang++`;
   注意 `bits/stdc++.h` 需要 GCC 的 libstdc++)。
3. 对目录里每个 `inK`(按数字排序)运行 `.main`,stdin 来自 `inK`,`timeLimit`
   超时即 kill;stdout 落临时文件后与 `ansK` 比对:忽略**行尾空白**与**文件末尾
   空行**(CRLF 兼容)。
4. 输出:

```
Judging /path/to/codeforces/118/A (time limit 2000 ms, 3 test(s))
Test #1: AC (3ms)
Test #2: WA
  first difference at line 1
    expected: .t.r
    actual  : .t.rr
Test #3: TLE
  exceeded 2000 ms (stopped after 2001 ms)
1/3 test(s) passed
```

全部 AC 退出码 `0`,否则 `1`(编译失败、无测试点同样非 0)。`.main` 会保留,便于
后续手动运行。

## 环境变量一览

| 变量 | 作用 |
| --- | --- |
| `ZEDCOMP_PORT` | HTTP 端口(默认 27121,`initializationOptions.port` 优先) |
| `ZEDCOMP_WORKSPACE` | 工作目录根,优先于 LSP 的 rootUri/rootPath |
| `ZEDCOMP_CONFIG_DIR` | 查找 `template.cpp` 的目录(默认 `~/.config/zedcomp`) |
| `ZEDCOMP_CXX` | judge 使用的 C++ 编译器(默认 `g++`) |
| `ZEDCOMP_ZED_CLI` | 打开文件用的 `zed` CLI 路径 |
| `ZEDCOMP_NO_OPEN` | 设为 `1` 则不调用 `zed` |

模板相关(LSP `initializationOptions`):

| 字段 | 作用 |
| --- | --- |
| `templatePath` | 模板文件绝对路径(优先级最高,支持 `~/`) |
| `template` | 内联模板字符串(次优先) |
| `zedcomp.templatePath` / `zedcomp.template` | 同上,允许嵌在 `"zedcomp"` 对象里 |

## 测试

`cargo test` 覆盖:三个 OJ 的 URL 解析(含 mirror / gym / contest 形态)、未知站点
回退、路径分量净化、CC payload 解析(整数/浮点/字符串限制值)、工作目录生成与旧
测试点清理、答案比对与首个差异定位、LSP 帧读写与 `initialize` 参数捕获,以及模板
解析(优先级顺序、`templatePath` 坏路径回退、五个占位符含缺失值、`ZEDCOMP_CONFIG_DIR`
覆盖且忽略 `XDG_CONFIG_HOME`)。
`../fixtures/{cf,ac,luogu}.json` 存在时会作为真实 CC body 参与解析测试。

端到端脚本(需要 python3 / bash,会自行选择空闲端口):

```bash
BIN=target/debug/zedcomp-helper
python3 scripts/e2e_serve.py "$BIN" /tmp/zedcomp-serve 28100 ../fixtures
bash    scripts/e2e_judge.sh "$BIN" /tmp/zedcomp-judge     # clang++ 可用,默认 ZEDCOMP_CXX 可覆盖
```

`e2e_serve.py` 驱动真实 LSP 会话,验证 `capabilities:{}`、`initializationOptions.port`
重绑定、`ZEDCOMP_WORKSPACE` / `rootUri`(含 `%20`)优先级、POST → 目录生成、
`window/logMessage`、端口占用退化、`initializationOptions.templatePath` 渲染自定义
模板以及 stdin EOF 退出。`e2e_judge.sh` 验证 AC/WA/TLE/RE/编译错误与退出码。

## 未实现

`submit` 子命令(计划中包装 cf-tool 等)尚未实现。
