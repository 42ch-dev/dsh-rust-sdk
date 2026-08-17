# deepseek-harness-sdk

[English](README.md) | 中文

[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Language](https://img.shields.io/badge/language-Rust-orange)](Cargo.toml)
[![crates.io](https://img.shields.io/crates/v/deepseek-harness-sdk)](https://crates.io/crates/deepseek-harness-sdk)

发布历史见 [CHANGELOG](CHANGELOG.md)。

面向 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)
（DSH）运行时的 Rust 客户端 SDK：把官方运行时作为子进程拉起，并讲它的
stdio JSON-RPC 2.0 协议。一个 crate，两层 API：Python 对齐的高层 API
（`DeepSeekHarness` / `Session::run` / `RunResult`）与底层协议客户端
（`HarnessClient`）。

本 crate 是官方
[Python SDK](https://github.com/deepseek-ai/deepseek-harness/tree/master/python/sdk)
的设计孪生，共享同一运行时对端、同一线上协议、同一分层；凡是泄漏进公开
API 的类型与错误，均以 **Python SDK 表面为对齐基线**。TypeScript SDK 的
差异有明确记录（最典型的是 `RunResult`，见下文）。

本 crate 是**纯客户端**。它不包含任何 agent、LLM 或持久化逻辑——这些全部
由被拉起的运行时进程完成。运行时二进制为自带：本 crate 从不下载、捆绑或
随包分发运行时。

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
        runtime_bin: std::env::var("DSH_RUNTIME_BIN").ok(),
        request_timeout: Some(Duration::from_secs(120)),
        ..Config::default()
    })
    .await?;

    let session = harness.start_session(None);
    let result = session
        .run(Input::Text("Reply with exactly: ok".into()))
        .await?;

    println!("finish_reason: {:?}", result.finish_reason);
    println!("final_response: {}", result.final_response);

    harness.close().await?;
    Ok(())
}
```

`DeepSeekHarness::start` 是急切的：它在返回前完成解析运行时、拉起子进程
并完成 `initialize` 握手。与其他 SDK 一样，运行时从环境中继承
`DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY`，因此调用方可以直接使用真实模型
端点，或把这些变量指向本地代理。

## 运行时获取

运行时为自带；SDK 只负责定位它。先分清概念：交互式 `dsh` CLI
（`@deepseek-ai/dsh`——即你从 `npx` / 全局安装认识的那个程序）**不是**
本 SDK 的运行时；它不提供 SDK 所讲的 stdio JSON-RPC 协议。SDK 需要的是
无头 JSON-RPC 运行时（`dsh-jsonrpc-agent`）——官方 Python SDK 捆绑的
同一个运行时载体。上游以两种方式分发它：Python SDK 平台 wheel 中的
自包含单文件可执行文件，以及 npm 分发的 Node.js 程序。任选一条途径：

### 途径 A —— 预构建可执行文件（默认）

上游把运行时打包为自包含的 Node.js 单文件可执行文件（运行时不需要
Node.js；插件树已内嵌），经 Python SDK 的 `deepseek-harness-runtime-bin`
平台 wheel 分发（linux-x64、linux-arm64、macos-arm64）：

```sh
python -m pip install deepseek-harness-runtime-bin
export DSH_RUNTIME_BIN="$(python -c 'import deepseek_harness_runtime as r; print(r.bundled_runtime_path())')"
```

那条 `python -c` 只是**定位**已安装的可执行文件并打印其路径——**SDK
运行时不跑任何 Python**。macOS 上该可执行文件需要同目录下的伴生
`-spawn-helper` 文件（wheel 会一并安装）——如果你把可执行文件复制到
别处，请把 helper 一起复制。

本途径配合 SDK 注入的捆绑默认 `cordis.yml`（见下文）开箱即用：不需要
`DSH_CORDIS_CONFIG`。

### 途径 B —— npm（免构建）

运行时 bin 发布为
[`@deepseek-ai/dsh-sdk-jsonrpc-demo`](https://www.npmjs.com/package/@deepseek-ai/dsh-sdk-jsonrpc-demo)
（bin：`dsh-jsonrpc-agent`）。需要 Node.js ≥ 22.19。请从 **`next`
dist-tag** 安装：这些包的 `latest` tag 目前指向一套更旧的混合版本矩阵，
而 `next` 会把整套解析到同一条发布线（即交互式 `dsh` CLI 所在的发布线）。
两种跑法任选：

- **`npx`（免安装）**——`npx` 本身就是程序，因此用
  `Config::launch_args_override`：

  ```rust
  Config {
      launch_args_override: Some(vec![
          "npx".into(),
          "--yes".into(),
          "@deepseek-ai/dsh-sdk-jsonrpc-demo@next".into(),
      ]),
      ..Config::default()
  }
  ```

- **`npm install -g`**——bin 上 `PATH`：

  ```sh
  npm install -g @deepseek-ai/dsh-sdk-jsonrpc-demo@next
  export DSH_RUNTIME_BIN=dsh-jsonrpc-agent
  ```

无论哪种跑法，npm bin 都从**配置项目**（即 `cordis.yml` 所在目录）解析
配置中点名的插件，因此该途径还需要一个装有插件集的小型配置项目：

```sh
mkdir dsh-runtime && cd dsh-runtime
npm init -y >/dev/null
npm install @deepseek-ai/dsh-sdk-jsonrpc-server@next @deepseek-ai/dsh-agent-spine-demo@next \
  @deepseek-ai/dsh-llm-deepseek@next @deepseek-ai/dsh-session-persistence-jsonl@next \
  @deepseek-ai/dsh-session-checkpoint-policy@next @deepseek-ai/dsh-subprocess-local@next \
  @deepseek-ai/dsh-bash-local@next @deepseek-ai/dsh-fs-local@next
# 把默认 cordis.yml 放到 package.json 旁（见下文），然后：
export DSH_CORDIS_CONFIG="$PWD/cordis.yml"
```

配置文件用上游默认配置——DSH 仓库中的
[`python/sdk-runtime/src/deepseek_harness_runtime/runtime/cordis.yml`](https://github.com/deepseek-ai/deepseek-harness/blob/master/python/sdk-runtime/src/deepseek_harness_runtime/runtime/cordis.yml)
——或自行组合（保留 `@deepseek-ai/dsh-sdk-jsonrpc-server` 条目；缺了它
运行时不提供任何服务）。

> **npm 途径注意：** npm bin 没有内建插件树——插件加载失败是致命的，而
> SDK 捆绑的默认配置（解压到临时目录，旁边没有 `node_modules`）无法满足
> 插件解析。这就是本途径要求显式设置 `DSH_CORDIS_CONFIG` 的原因。

### SDK 如何解析运行时（参考）

二进制解析遵循 Python `HarnessClient` 的对齐语义，外加 Rust 独有的
`DSH_RUNTIME_BIN` 途径：

1. `Config::launch_args_override`（非空）—— 完整 argv，原样使用
   （途径 B 的 npx 变体即用此项）；
2. `Config::runtime_bin`；
3. 父进程环境中的 `DSH_RUNTIME_BIN`；
4. 否则返回 `Error::RuntimeNotFound`，其错误消息会点名上述获取途径。

空的 `launch_args_override` 与空的 `DSH_RUNTIME_BIN` 均视为不存在
（Python truthiness），因此解析永远不会产生无法启动的空程序。

**捆绑默认配置。** 当不存在有效的 `DSH_CORDIS_CONFIG` 时，
`DeepSeekHarness::start` 会注入一份随包携带的运行时默认 `cordis.yml`
（与官方默认逐字节一致），首次使用时解压到系统临时目录，且每次使用都会
做字节校验——运行时在缺少显式配置时拒绝启动，因此该注入是必需的；解压
或校验失败会以 `Error::Io` 传播（绝不会静默地无配置启动）。注意
[npm 途径注意](#途径-b--npm免构建)：捆绑默认配置只有在运行时自带插件树
（途径 A 的可执行文件）时才能完成插件解析；用 npm bin 时请务必自行提供
`DSH_CORDIS_CONFIG`。

> **与 Python SDK 的刻意分歧**（已记录；请勿"修正"为与 Python 一致）：
> Python 仅在捆绑运行时载体被使用时才注入其默认配置。本 crate 没有捆绑
> 载体（运行时自带），因此只要没有有效配置就会注入默认配置，无论二进制
> 是如何解析得到的。

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

`start` 是**急切**的：它在返回前完成解析运行时、组装环境注入集、拉起
子进程并执行 `initialize` 握手。（这与 Python 与 TypeScript SDK 不同——
它们首次使用时才惰性启动。）握手失败时，错误在传播前会先跑完关闭阶梯，
因此拉起的子进程绝不会泄漏（Python 对齐）。

`Config::cwd` 会被解析为绝对路径（Python `Path(cwd).resolve()`），同时
供给 `DSH_CWD` 与 `initialize.cwd`；cwd 不存在时以 `Error::Io` 失败。
`Config::request_timeout` 约束每一个请求，包括 `session/prompt`；
`None`（默认值）表示无限等待。

`start_session` 创建的会话可以并发运行：harness 在异步互斥锁后持有拉起的
子进程，各会话在 `session/prompt` 写入处交错，并各自在自己的订阅上等待。

### `Session::run` —— 一次活动区间

`run` 逐字实现 Python `Session.run` 算法：

1. **在写入 prompt 之前**订阅会话树，保证本轮的每条通知都不会漏掉。
2. 发送 `session/prompt`（受 `Config::request_timeout` 约束）。
3. 等待持久化的 `agent/inbox/spliced` 回执，其 `inserted[].id` 等于返回的
   消息 id（字段名是 `id`，**不是** `messageId`）；回执之前的通知会从
   `events` 与 `notifications` 中一并丢弃。
4. 从回执（**含**回执本身）开始收集全部树通知，直到**根**会话上报
   `session.status == "idle"`（这条 idle 通知也会被收集；非根会话的 idle
   不会终止本次 run）。

`events` 只包含根会话的 `session.event` 载荷；`notifications` 包含全部树
通知（根会话 + 发现的子代会话，含 `session.status` / `subagent.*`），按
传输顺序排列。

两段等待——等回执与等 idle——都是**无界**的（Python 对齐）；只有
`session/prompt` 请求受 `Config::request_timeout` 约束。需要边界的调用方
请用 `tokio::time::timeout` 包住该调用——这只约束本地等待，不约束运行时
侧的执行。`session.event` / `session.status` 通知的载荷若不符合线上形状，
本次 run 会以 `Error::SdkProtocol` 失败（Python SDK 会 raise；Rust 把同一
情形以类型化错误呈现，而不是静默丢弃事件或误判 idle 终止）。

### `RunResult`

`RunResult` 遵循 **Python** SDK 的字段集。TypeScript SDK 的 `RunResult`
缺少 `finish_reason` 与 `session_root`；Rust 有意跟随 Python 而非
TypeScript：

| 字段 | Python | TypeScript | Rust（本 crate） |
|---|---|---|---|
| `session_id` / `sessionId` | yes | yes | `session_id: String` |
| `final_response` / `finalResponse` | yes | yes | `final_response: String` |
| `finish_reason` | yes（Python 扩展） | no | `finish_reason: Option<String>` |
| `events` | yes（仅根会话） | yes | `events: Vec<serde_json::Value>` |
| `notifications` | yes（根 + 子代，传输顺序） | yes | `notifications: Vec<Notification>` |
| `session_root` | yes（Python 扩展） | no | `session_root: Option<PathBuf>` |

（该表在 crate rustdoc 的 `# Compatibility` 中镜像；两处副本需保持同步。）

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
| `Error::TransportClosed` | 运行时进程未运行，或 stdio 意外关闭；携带诊断信息（退出状态与捕获的 stderr 尾部） |
| `Error::RequestTimeout` | 请求在配置的超时时间内未得到响应；携带方法名 |
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

树通知经由一条容量上限为 4096、drop-oldest 语义的广播通道。如果高流量
会话树在 SDK 两次读取之间灌入超过容量的通知，被丢弃集合可能包含某次 run
所依赖的收件回执或根 idle 通知——此时 `Session::run` 不会永远挂起或返回
静默截断的结果，而是**快速失败**，报 `Error::SdkProtocol`。预期超大突发
量的调用方只能经由底层 `HarnessClient::spawn_with_broadcast_capacity`
绕过该上限，而不是用 `DeepSeekHarness::start`。

## 环境变量

父进程环境整体继承；SDK 只注入或覆盖下表所列键（SDK 键优先于调用方
`Config::env` 条目——Python `dict.update` 语义）：

| 变量 | 作用 | 语义 |
|---|---|---|
| `DSH_RUNTIME_BIN` | 运行时二进制解析 | 当 `Config::launch_args_override` 与 `Config::runtime_bin` 均未设置时被查阅；空值视为不存在 |
| `DSH_CORDIS_CONFIG` | 运行时组合配置 | `Config::cordis_config`（非空）优先；否则继承 `Config::env` 或父进程环境中非空的值。空字符串视为不存在——复制时跳过空字符串的 `Config::env` 条目，使其永远不会覆盖父进程中非空的值。没有有效值时，SDK 注入捆绑的默认 `cordis.yml` |
| `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` | 模型端点与凭据 | 原样继承；仅在配置了 `Config::base_url` / `Config::api_key` 时被覆盖 |
| `DSH_CWD` | agent 工作目录 | 始终注入，取自 `Config::cwd`（解析为绝对路径） |
| `DSH_SESSION_ROOT` | 会话根目录 | 仅在配置了 `Config::session_root` 时注入；在每次 `RunResult` 上呈现 |

## 测试

- `cargo test` —— 针对脚本化 fake runtime 的线上协议、生命周期与
  `Session::run` 语义测试套件（无需真实运行时）。
- `tests/real_runtime.rs` —— 单个冒烟测试，仅在同时设置了
  `DSH_RUNTIME_BIN` 与 `DEEPSEEK_API_KEY` 时运行；否则打印显式跳过说明
  并直接通过，因此没有运行时与凭据时 `cargo test` 也是绿的。

## 平台支持与 MSRV

SDK 本体是纯 Rust、平台负担很小；平台矩阵由所消费的运行时决定。途径 A
（预构建可执行文件）分发 linux-x64、linux-arm64、macos-arm64；途径 B
（npm）在一切能跑 Node.js ≥ 22.19 的平台上可用。**不支持 Windows**：
上游没有 Windows 运行时构建。

MSRV：当前 stable Rust（`Cargo.toml` 未固定最低版本；本 crate 跟随稳定版
工具链）。

## 已知限制

- **预发布软件** —— 在运行时协议稳定之前，crate 以预发布版本发布；
  `0.1.0` 之前 API 仍可能变化。真实运行时冒烟测试按环境门控（见
  [测试](#测试)）；协议正确性由 fake-runtime 套件承担。
- **不支持中途取消** —— 线上协议没有 session-close / cancel RPC。
  `Session::run` 会一直等到根会话上报 `idle`；中途关闭 harness 会放弃
  进行中的回合。`Config::request_timeout` 只会放弃本地等待——服务端工作
  仍会继续运行直到关闭。
- **没有版本协商** —— 运行时以 `serverInfo` 0.0.1 预发布身份标识，
  `initialize` 执行严格的 `serverInfo.name` 检查
  （`deepseek-harness-sdk-runtime`）且 `version` 必填：协议声明该名称在
  线上稳定、无协商机制，因此身份不符是硬性 `Error::SdkProtocol`。
- **无运行时二进制分发 / 捆绑 / 下载** —— 运行时配套 crate 是路线图事项，
  不属于本版本。请按[运行时获取](#运行时获取)自行获取运行时。
- **不提供 TypeScript 对齐辅助** —— 不带 `finish_reason` / `session_root`
  的 TS 形状 `RunResult` 不在提供范围内。

## License

Apache-2.0。
