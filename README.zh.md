# deepseek-harness-sdk

[English](README.md) | 中文

[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Language](https://img.shields.io/badge/language-Rust-orange)](Cargo.toml)
[![crates.io](https://img.shields.io/crates/v/deepseek-harness-sdk)](https://crates.io/crates/deepseek-harness-sdk)

发布历史见 [CHANGELOG](CHANGELOG.md)。

面向 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)
（DSH）运行时的 Rust 客户端 SDK。运行时即**在具名 profile 下启动的 `dsh`
CLI**——本 crate 以 `dsh --profile sdk`（profile 默认值）把运行时作为
子进程拉起，并讲它的 stdio JSON-RPC 2.0 协议。一个 crate，两层 API：
Python 对齐的高层 API（`DeepSeekHarness` / `Session::run` /
`RunResult`）与底层协议客户端（`HarnessClient`）。

本 crate 是官方
[Python SDK](https://github.com/deepseek-ai/deepseek-harness) 的设计孪生，
共享同一运行时对端、同一线上协议、同一分层；凡是泄漏进公开 API 的类型
与错误，均以 **Python SDK 表面为对齐基线**。TypeScript SDK 的差异有明确
记录（最典型的是 `RunResult`，见下文），本 crate 相对两个参考实现自身的
刻意分歧也有记录（见[刻意分歧](#刻意分歧)）。

本 crate 是**纯客户端**。它不包含任何 agent、LLM 或持久化逻辑——这些全部
由被拉起的运行时进程完成。运行时为自带：本 crate 从不下载、捆绑或随包
分发运行时（见[运行时获取](#运行时获取)）。

## 安装

```sh
cargo add deepseek-harness-sdk
```

或写入 `Cargo.toml`：

```toml
[dependencies]
deepseek-harness-sdk = "*"
```

版本由你选择（`cargo search deepseek-harness-sdk` 或
[crates.io 页面](https://crates.io/crates/deepseek-harness-sdk) 可查最新版）。
crate 处于预发布线时，裸的 `cargo add deepseek-harness-sdk` 可能不会解析到
最新的预发布版本——需要时请显式指定（例如
`cargo add deepseek-harness-sdk@0.1.0-alpha`）。`0.1.0` 之前 API 仍可能变化。

首次运行前的两个前置条件：一个 DSH 运行时（见
[运行时获取](#运行时获取)）与模型凭据（环境变量 `DEEPSEEK_API_KEY`，或
`Config::api_key` / `Config::base_url`）。

## 快速开始

```rust
use deepseek_harness_sdk::{Config, DeepSeekHarness, Input};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut harness = DeepSeekHarness::start(Config {
        dsh_bin: std::env::var("DSH_RUNTIME_BIN").ok(),
        request_timeout: Some(Duration::from_secs(120)),
        ..Config::default()
    })
    .await?;

    let session = harness.start_session(None);
    let result = session
        .run(Input::Text("Reply with exactly: ok".into()), None)
        .await?;

    println!("finish_reason: {:?}", result.finish_reason);
    println!("final_response: {}", result.final_response);

    harness.close().await?;
    Ok(())
}
```

`DeepSeekHarness::start` 是急切的：它在返回前完成解析运行时、解析（并在
缺失时创建）harness 主目录、拉起子进程并完成 `initialize` 握手。除非
crate 注入了覆盖值，运行时从环境中继承 `DEEPSEEK_BASE_URL` /
`DEEPSEEK_API_KEY`，因此调用方可以直接使用真实模型端点，或把这些变量
指向本地代理。

## 运行时获取

运行时为自带；SDK 只负责解析与启动它。**不存在独立的 JSON-RPC agent
程序**：SDK 所讲的 stdio JSON-RPC 服务器只是运行时 profile bundle 里的一
行插件。运行时就是
[deepseek-harness](https://github.com/deepseek-ai/deepseek-harness) 仓库里
普通的 `dsh` CLI，在 `sdk` profile 下启动（或用 `Config::profile` 指定
任意 profile）。

上游把运行时打包为自包含的 Node.js 单文件可执行文件（运行时不需要系统
Node.js；插件树已内嵌），经 `deepseek-harness-runtime-bin` 平台 wheel
分发——wheel 安装的是普通 `dsh` CLI，名为
`deepseek-harness-sdk-runtime-<platform>-<arch>`。已发布目标为 **Linux
x64、Linux arm64、macOS arm64、macOS x64、Windows x64**（Windows 使用
`.exe` 后缀）。macOS 需要可执行文件旁的伴生 `-spawn-helper`
（`node-pty`），Linux/macOS wheel 携带 `-rg` ripgrep 伴随文件（Windows 为
`-rg.exe`）——移动可执行文件时请把伴随文件一并复制。

两条获取途径：

### 途径 A —— 平台 wheel（推荐）

```sh
python -m pip install deepseek-harness-runtime-bin
export DSH_RUNTIME_BIN="$(python -c 'import deepseek_harness_runtime as r; print(r.bundled_runtime_path())')"
```

那条 `python -c` 只是**定位**已安装的可执行文件并打印其路径——**SDK
运行时不跑任何 Python**。SDK 直接启动该可执行文件（始终注入解析后的
`DSH_HOME`，因此即使首次启动主目录也是显式的）。

### 途径 B —— 从源码构建

用[官方仓库](https://github.com/deepseek-ai/deepseek-harness)中的
`build-exe-for-python-sdk` 脚本构建运行时可执行文件，然后把
`DSH_RUNTIME_BIN`（或 `Config::dsh_bin`）指向构建产物。当已发布的 wheel
不覆盖你的平台时，使用这条途径。

### SDK 如何解析运行时

1. `Config::dsh_bin`（非空）；
2. 父进程环境中的 `DSH_RUNTIME_BIN`（非空）；
3. 否则返回 `Error::RuntimeNotFound`，其错误消息会点名获取途径并引用
   官方仓库。

空的 `Config::dsh_bin` 与空的 `DSH_RUNTIME_BIN` 均视为不存在，因此解析
永远不会产生无法启动的空程序。`DSH_RUNTIME_BIN` 在 `runtime_bin` →
`dsh_bin` 改名后**被显式保留**：它是受支持的产品表面，不是已移除字段的
兼容垫片。

启动 argv 恰为 `--profile <profile>`（默认 `"sdk"`），后接按调用方顺序
排列的、每条 `Config::patches` 对应的 `--patch <绝对路径>` 对。patch 路径
在启动前解析为绝对路径。crate 从不传应用参数，也从不发出诊断性子命令
`--dump-config` / `--dump-default-config`。空的 `profile` 会在启动前被本地
拒绝。

## `DSH_HOME` 解析

harness 主目录遵循**运行时自身的优先级**，从高到低：

1. 显式配置的 `Config::dsh_home`；
2. **非空**的 `$DSH_HOME`（先看 `Config::env`，再继承父进程环境）——
   空白或仅空白的值视为**未设置**；
3. `~/.dsh`。

解析结果被规范化为绝对路径（`~` 已展开），缺失时会被创建以便全新主目录
可直接启动，并且**始终**以 `DSH_HOME` 注入运行时子进程环境。

选择结果**可观察，绝不静默**：调用 `Config::resolve_dsh_home(&parent_env)`
可读取一次启动会选中的值；对运行中的实例调用 `DeepSeekHarness::dsh_home()`
可读取它实际启动所用的主目录。

> **与 Python SDK 的刻意分歧**（已记录；请勿"修正"为与 Python 一致）：
> Python 会抛 `ValueError` 而拒绝回退，本 crate 则解析 `~/.dsh`——
> `~/.dsh` 是运行时自己文档化的默认值。crate 遵循运行时的优先级（与
> TypeScript SDK 一致），而不是 Python 的拒绝行为。Python 的拒绝真正防的
> 是*静默*选择主目录；上面的可观察性正是为此而设。

## API 走读

### 分层

- `HarnessClient`（底层）：拉起运行时进程，持有 stdio 传输层，讲
  JSON-RPC 2.0 线上协议，并把通知扇出到各订阅。暴露 `LaunchSpec`、
  `ClientTimeouts` 与 `NotificationSubscription`。
- `DeepSeekHarness` / `Session`（高层）：构建在 `HarnessClient` 之上的
  Python 对齐 owned-run API。
- `Input` 接受纯文本（`Input::Text`）或原始内容块（`Input::Blocks`），
  镜像 Python 的 `normalize_input`。

### `DeepSeekHarness::start`

`start` 是**急切**的：它在返回前完成解析运行时、解析并创建 harness 主
目录（见 [`DSH_HOME` 解析](#dsh_home-解析)）、组装子进程环境、拉起
子进程并执行 `initialize` 握手。（这与 Python 与 TypeScript SDK 不同——
它们首次使用时才惰性启动。）握手失败时，错误在传播前会先跑完关闭阶梯，
因此拉起的子进程绝不会泄漏。

`initialize` 握手受 `Config::initialize_timeout` **约束**（默认 **30 s**，
与 Python 一致；`None` 表示无界，是刻意的选择退出）。该约束只作用于握手
本身——绝不作用于活动区间或 `session/prompt`。到期时 `start` 返回
`Error::RequestTimeout { method: "initialize", .. }`，其错误消息会点名
**所选 profile**（如 `(selected dsh profile 'sdk')`），并且子进程会被关闭
而非遗留运行。

`Config::reasoning_effort`（`Option<String>`）仅在值为非空字符串时作为
线上键 `reasoningEffort` 随 `initialize` 发送：未设置或空白值一律省略。
运行时拒绝非字符串或空的 `reasoningEffort`，因此空白值会被丢弃而非发送。

`Config::cwd` 会被解析为绝对路径并以 `initialize.cwd` 发送；
`Config::runtime_cwd` 设置子进程工作目录，默认取 `cwd`。
`Config::request_timeout` 约束其余每一个请求，包括 `session/prompt`；
`None`（默认值）表示无限等待。

`start_session` 创建的会话可以并发运行：harness 在异步互斥锁后持有拉起的
子进程，各会话在 `session/prompt` 写入处交错，并各自在自己的订阅上等待。

### `Session::run` —— 一次活动区间

`run` 实现 Python `Session.run` 算法：

1. **在写入 prompt 之前**订阅会话树，保证本轮的每条通知都不会漏掉。
2. 发送 `session/prompt`（受 `Config::request_timeout` 约束）。
3. 等待持久化的 `agent/inbox/spliced` 回执，其 `inserted[].id` 等于返回的
   消息 id（字段名是 `id`，**不是** `messageId`）；回执之前的通知会从
   `events` 与 `notifications` 中一并丢弃。
4. 从回执（**含**回执本身）开始收集全部树通知，直到**根**会话上报
   `session.status == "idle"`（这条 idle 通知也会被收集；非根会话的 idle
   不会终止本次 run）。

`events` 只包含根会话的 `session.event` 载荷；`notifications` 包含全部树
通知（根会话 + 发现的子代会话，含 `session.status` / `subagent.started` /
`subagent.finished`），按传输顺序排列。

`run` 接受可选的**逐通知回调**：

```rust
pub async fn run(
    &self,
    input: Input,
    on_notification: Option<&(dyn Fn(&Notification) + Send + Sync)>,
) -> Result<RunResult, Error>
```

该回调观察**本次 run 的会话树订阅收到的每一条通知，按线上顺序**，并且
**随通知到达即时调用**，不会推迟到 run 结束。它是纯观察者：不会改变返回
的 `RunResult`（有回调与无回调时，`events` 与 `notifications` 都持有相同
集合、相同顺序）；它是既有通知路径之上的一层，而非第二个订阅；它是可选
且附加的（传 `None` 与普通 run 路径行为完全一致）。回调内 panic 属调用方
缺陷，会向外传播；crate 不会吞掉它。

两段等待——等回执与等 idle——都是**无界**的（Python 对齐）；只有
`session/prompt` 请求受 `Config::request_timeout` 约束。需要边界的调用方
请用 `tokio::time::timeout` 包住该调用——这只约束本地等待，不约束运行时
侧的执行。

### 会话格式 v2

运行时的 `session.event` 词汇是**会话格式 v2**（非线上变更）。crate 记录
v2 词汇并保持事件载荷无类型，因此 run 路径不受影响：

- `assistant/message` 现在携带内嵌的 `stream: AssistantStreamRecord[]`；
- 新增 `assistant/attempt`；
- 移除 `assistant/chunk`——crate 不声称支持 `assistant/chunk`，本迭代也
  不解析内嵌的 v2 stream（非目标）。

### 内容块词汇

`ContentBlock` 以 **六个**类型化变体建模 DSH `ContentBlockMap`，外加供
未知标签与畸形载荷回退的 `Unknown`：

| 变体 | 形状 |
|---|---|
| `text` | `{type:"text", text}` |
| `reasoning` | `{type:"reasoning", text}` |
| `image` | `{type:"image", attachment: ImageAttachmentRef}` |
| `file` | `{type:"file", attachment: FileAttachmentRef}` |
| `tool-call` | `{type:"tool-call", id, name, arguments}` —— `arguments` 是**原始 JSON 字符串** |
| `tool-result` | `{type:"tool-result", toolCallId, content: ContentBlock[], isError?}` —— `content` 可递归 |

`FileAttachmentRef` 为 `{attachmentId, name, bytes}`。`ImageAttachmentRef`
为 `{attachmentId, mediaType, bytes, width, height, name?, originalDimensions?}`；
`originalDimensions` 仅在归一化缩小了图片时存在，且解析 → 序列化往返不丢
失。未识别的 `type`（或已知 tag 但载荷畸形）会回退到 `ContentBlock::Unknown`，
原样保留原始对象。

### `RunResult`

`RunResult` 遵循 **Python** SDK 的字段集——恰好五个字段，**没有**
`session_root`（上游已移除并断言其不存在）。TypeScript SDK 的 `RunResult`
缺少 `finish_reason`；Rust 跟随 Python：

| 字段 | Python | TypeScript | Rust（本 crate） |
|---|---|---|---|
| `session_id` / `sessionId` | yes | yes | `session_id: String` |
| `final_response` / `finalResponse` | yes | yes | `final_response: String` |
| `finish_reason` | yes | no | `finish_reason: Option<String>` |
| `events` | yes（仅根会话） | yes | `events: Vec<serde_json::Value>` |
| `notifications` | yes（根 + 子代，传输顺序） | yes | `notifications: Vec<Notification>` |
| `session_root` | no（已移除） | no | **不存在** |

两个派生字段描述的是本次拥有的活动区间，而非因果归属于该 prompt 的输出：
`final_response` 是区间内最后一条已提交的根会话 assistant 文本——steering、
注入的上下文及其他排队的工作都可能先于 idle 产生贡献；`finish_reason` 是
区间内最后一条根会话 `turn/end` 的 `kind`（如 `completed`、`max-tokens`、
`error`），没有 `turn/end` 时为 `None`。`turn/end` 缺少字符串形式的
`data.reason.kind` 违反运行时协议，以 `Error::SdkProtocol` 失败。

### 类型化错误

所有失败路径都返回 `Error` 变体，而非临时字符串：

| 变体 | 含义 |
|---|---|
| `Error::RuntimeNotFound` | 任何地方都没有配置运行时二进制；消息会点名获取途径 |
| `Error::Config` | 无效的启动配置（如空的 `profile`），在启动前于本地拒绝 |
| `Error::TransportClosed` | 运行时进程未运行，或 stdio 意外关闭；携带诊断信息（退出状态与捕获的 stderr 尾部） |
| `Error::RequestTimeout` | 请求在配置的超时时间内未得到响应；携带方法名，且对 `initialize` 会点名所选 profile（如 `(selected dsh profile 'sdk')`） |
| `Error::SdkProtocol` | 协议级违规（服务器身份缺失、`messageId` 缺失、`finish_reason` 提取失败、畸形通知、订阅滞后）；可用 `Error::is_protocol()` 检测 |
| `Error::JsonRpc` | JSON-RPC 错误响应，保留 `code`（`Option<i64>`）与可选 `data` |
| `Error::Io` / `Error::Json` | I/O（spawn、stdio、传输）与 JSON 序列化/反序列化错误 |

### 关闭阶梯

`DeepSeekHarness::close`（与 `HarnessClient::close`）执行关闭阶梯：协作式
`shutdown` 请求（受 `shutdown_timeout` 约束，默认 1s，失败仅作诊断）→
关闭 stdin（EOF）→ 等待 `eof_grace`（默认 6s——运行时在 stdin 关闭后
有时间冲刷持久化状态）→ SIGTERM → 等待 `term_grace`（默认 3s）→
SIGKILL → 等待。该阶梯幂等，是无条件清理（任何一层的失败仍会回收子进程
——子进程还会在 drop 时被杀，阶梯失败不会遗留游离进程），并会以
`Error::TransportClosed` 解析所有挂起请求。

### 通知

线上有四种服务器到客户端的通知：`session.event`、`session.status`、
`subagent.started`、`subagent.finished`。树通知经由一条容量上限为 4096、
drop-oldest 语义的广播通道。如果高流量会话树在 SDK 两次读取之间灌入超过
容量的通知，被丢弃集合可能包含某次 run 所依赖的收件回执或根 idle
通知——此时 `Session::run` 不会永远挂起或返回静默截断的结果，而是
**快速失败**，报 `Error::SdkProtocol`。预期超大突发量的调用方只能经由
底层 `HarnessClient::spawn_with_broadcast_capacity` 绕过该上限，而不是用
`DeepSeekHarness::start`。如需按到达顺序观察每条通知，请给 `Session::run`
传入逐通知回调（见上文）。

## 环境变量

父进程环境整体继承；SDK 只注入或覆盖下表所列键。crate 注入值先应用，
调用方 `Config::env` 条目随后应用，因此冲突时调用方的值优先（Python
`dict.update` 语义）——`DSH_HOME` 除外：它走解析，绝不被解析后覆盖：

| 变量 | 作用 | 语义 |
|---|---|---|
| `DSH_HOME` | harness 主目录（子进程环境） | 始终写入解析后的绝对、`~` 已展开主目录。调用方的值只是**解析输入**（`Config::dsh_home` → 非空 `DSH_HOME` → `~/.dsh`），不是解析后的覆盖；可用 `Config::resolve_dsh_home` / `DeepSeekHarness::dsh_home` 读回所选值 |
| `DSH_RUNTIME_BIN` | 运行时二进制解析 | 当 `Config::dsh_bin` 未设置时被查阅；空值视为不存在。在 `runtime_bin` → `dsh_bin` 改名后被显式保留（非兼容垫片） |
| `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` | 模型端点与凭据 | 原样继承；配置了 `Config::base_url` / `Config::api_key` 时注入覆盖值，且调用方在 `Config::env` 中提供的同名条目在注入之后应用，冲突时以调用方为准 |
| `DSH_CORDIS_CONFIG` / `DSH_SESSION_ROOT` / `DSH_CWD` | 已移除——绝不写入 | 上游已无读取方；见[移除表面（v0.1 → 当前）](#移除表面v01--当前) |

## 刻意分歧

以下每条分歧都是刻意的并已记录；未经取代性的 spec 决策，贡献者不得将其
"修正"回参考实现的行为。

1. **`DSH_HOME` 回退** —— Python 抛 `ValueError` 时，crate 解析 `~/.dsh`
   （见 [`DSH_HOME` 解析](#dsh_home-解析)）。
2. **无客户端定向请求 API** —— Python 暴露 `next_request` / `respond` /
   `notify`；crate 一个都不暴露（运行时不会发出客户端定向请求；此类请求
   一律自动应答 `-32601`）。非目标，不是缺口。
3. **更严格的畸形通知策略** —— 载荷不符合形状检查的 `session.event` /
   `session.status` 会使本次 run 以 `Error::SdkProtocol` 失败。Python 会
   静默跳过畸形 event/status，只在最后一条 `turn/end` 畸形时 raise；
   TypeScript 对畸形 `session.event` 会 raise，但忽略畸形的
   `session.status`。这能把静默挂起转为类型化失败，**不是** Python 对齐。
   相关的本地健壮性选择：内嵌的 stderr 尾部有上限（8 KiB，最新行在前），
   广播缓冲有界并在观测到滞后时快速失败。
4. **严格 `serverInfo.name` 相等** —— `initialize` 要求身份恰为
   `deepseek-harness-sdk-runtime`；Python 把这些字段视为可选，TypeScript
   只检查存在性。上游改名会响亮地失败，而不是被静默接受。
5. **无 `run()` 便捷方法、无惰性启动** —— crate 要求显式
   `DeepSeekHarness::start`；Python 与 TypeScript 可以在首次使用时惰性
   启动。非目标，不是缺口。

## 移除表面（v0.1 → 当前）

以下 v0.1 标识符**已移除——无别名、无 deprecated 垫片**。每行注明它曾
是什么、用什么取代：

| 移除项 | 它是什么 | 取代物 |
|---|---|---|
| `Config::session_root` | 声称控制会话落盘位置 | `Config::dsh_home` —— 会话位于 `$DSH_HOME/sessions` 下 |
| `Config::cordis_config` | `cordis.yml` 配置文件的路径 | profile 树（`Config::profile` + `Config::patches`）—— 不再向运行时传配置文件 |
| `DSH_CORDIS_CONFIG` 注入 | 向子进程环境写入 `DSH_CORDIS_CONFIG` | profile 树 —— 上游已无读取方 |
| `DSH_SESSION_ROOT` 注入 | 向子进程环境写入 `DSH_SESSION_ROOT` | `Config::dsh_home` —— 会话位于 `$DSH_HOME/sessions` 下 |
| `DSH_CWD` 注入 | 向子进程环境写入 `DSH_CWD` | `Config::cwd` —— 以 `initialize.cwd` 发送 |
| `Config::launch_args_override` | 用不透明列表整体替换 argv | `Config::dsh_bin` + `Config::profile` + `Config::patches` —— 启动由类型化字段组合 |
| `Config::runtime_bin` | 运行时路径覆盖字段 | `Config::dsh_bin`（改名；`DSH_RUNTIME_BIN` 环境途径保留） |
| `RunResult::session_root` | 在每次结果上呈现会话目录 | **移除——无取代物**（上游已移除） |
| `assets/cordis.yml` | 捆绑的默认配置文件 | profile bundle —— 它挂载的上游包已被删除 |
| `DEFAULT_CORDIS_YML` | 内嵌上面的配置文件 | profile bundle |
| `bundled_default_config_path` | 该配置的临时解压路径 | profile bundle —— 其全部用途就是那条已删除的注入通道 |
| `Cargo.toml [package] include` 里的 `assets/cordis.yml` 条目 | 随包分发已删除的文件 | —（该文件已不存在） |

## 测试

- `cargo test` —— 针对脚本化 fake runtime 的线上协议、生命周期与
  `Session::run` 语义测试套件（无需真实运行时）。
- `tests/real_runtime.rs` —— 一个**无密钥握手层**（针对真实 `dsh` 的
  start → `initialize` → close，无需 API key），在可取得 `dsh` 二进制时
  运行；外加一个由 `DEEPSEEK_API_KEY` 门控的在线回合层。否则打印显式跳过
  说明并直接通过，因此没有运行时与凭据时 `cargo test` 也是绿的。

## 平台支持与 MSRV

SDK 本体是纯 Rust、平台负担很小；平台矩阵由所消费的运行时决定。上游为
运行时发布 **5 个目标**：Linux x64、Linux arm64、macOS arm64、macOS
x64、Windows x64（见[运行时获取](#运行时获取)）。

MSRV：当前 stable Rust（`Cargo.toml` 未固定最低版本；本 crate 跟随稳定版
工具链）。

## 已知限制

- **预发布软件** —— 在运行时协议稳定之前，crate 以预发布版本发布；
  `0.1.0` 之前 API 仍可能变化。真实运行时测试按环境门控（见
  [测试](#测试)）；协议正确性由 fake-runtime 套件承担。
- **不支持中途取消** —— 线上协议没有 session-close / cancel RPC。
  `Session::run` 会一直等到根会话上报 `idle`；中途关闭 harness 会放弃
  进行中的回合。`Config::request_timeout` 只会放弃本地等待——服务端工作
  仍会继续运行直到关闭。
- **没有版本协商** —— 运行时以 `serverInfo` 0.0.1 预发布身份标识，
  `initialize` 执行严格的 `serverInfo.name` 检查
  （`deepseek-harness-sdk-runtime`）：协议声明该名称在线上稳定、无协商
  机制，因此身份不符是硬性 `Error::SdkProtocol`。
- **无运行时二进制分发 / 捆绑 / 下载** —— 运行时配套 crate 是路线图事项，
  不属于本版本。请按[运行时获取](#运行时获取)自行获取运行时。

## License

Apache-2.0。
