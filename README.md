# yi [译]

一个面向 **macOS 26+** 的本地 HTTP 翻译服务。使用 Rust、Axum、[`translation-rs`](https://docs.rs/translation-rs/0.5.2/translation/)；无 CTranslate2，不需要单独管理模型权重。

## 系统要求

- macOS **26 或更新版本**；Rust 工具链（支持 edition 2024）。
- 已经安装翻译所需的系统语言包（例如英语和简体中文）。终端直接构造 `TranslationSession` **无法弹窗请求下载**；必须提前在系统中安装。
- 建议先检查系统设置中的翻译语言，或通过 Apple 翻译 App 安装语言包。不同系统版本中界面位置可能有所差异。

## 启动

```bash
cargo run --release
```

默认只监听 `127.0.0.1:3000`。也可以指定：

```bash
TRANSLATE_BIND=127.0.0.1:8090 cargo run --release
```

使用 `Ctrl-C` 请求优雅退出。

## API

`GET /healthz`

```bash
curl http://127.0.0.1:3000/healthz
```

响应示例：

```json
{"status":"ok","backend":"Apple Translation.framework"}
```

`POST /translate`

```bash
curl -sS http://127.0.0.1:3000/translate \
  -H 'Content-Type: application/json' \
  -d '{"text":"Hello world!", "from":"en", "to":"zh-Hans"}'
```

响应示例（译文因系统模型而异）：

```json
{"text":"你好，世界！","from":"en","to":"zh-Hans"}
```

中译英：

```bash
curl -sS http://127.0.0.1:3000/translate \
  -H 'Content-Type: application/json' \
  -d '{"text":"今天天气很好。", "from":"zh-Hans", "to":"en"}'
```

`from` 默认 `en`，`to` 默认 `zh-Hans`。请求文本上限 16 KiB；JSON Body 上限 64 KiB。

## 常见错误

格式：`{"error":{"code":"...","message":"..."}}`。

- `400 EMPTY_TEXT`：文本为空。
- `400 INVALID_LANGUAGE`：语言代码格式不合要求。
- `409 LANGUAGE_PACK_NOT_INSTALLED`：系统语言包未安装。
- `422 UNSUPPORTED_LANGUAGE_PAIR`：系统不支持这个语言方向。
- `429 QUEUE_FULL`：翻译队列已满（容量 16）。
- `503 MAIN_RUN_LOOP_NOT_RUNNING`：主线程没有及时服务 RunLoop。
- `503 MACOS_VERSION_UNSUPPORTED`：系统版本不符合要求。
- `504 TRANSLATION_TIMEOUT`：Translation.framework 的翻译调用超时。
- `504 REQUEST_TIMEOUT`：HTTP 请求等待超过 90 秒（含排队时间）。

## 为什么要用专用翻译线程？

```text
            macOS 主线程
       CFRunLoopRun() [主队列回调]
                 ^
                 |
    Apple Translation.framework
                 |
      translation 工作线程
       HashMap<Pair, Session> (4项缓存)
                 ^
             SyncChannel (16)
                 ^
      Axum/Tokio 服务线程
                 ^
             HTTP 客户端
```

`translation-rs` 同步翻译等待苹果的主队列回调。不能用 `#[tokio::main]` 占住 macOS 主线程；应在独立线程启动 Tokio，让主线程持续运行 CFRunLoop。

`TranslationSession` 不跨线程移动、不放进 `Arc<Mutex<_>>`，所有会话在专用翻译线程创建并复用。HTTP Handler 只负责把 Job 送进有限容量的队列，再 `await` Tokio oneshot。

- 同时只运行一个翻译任务，避免重复同步访问底层会话。
- Translation.framework 的一次同步调用有自己的 60 秒超时；HTTP 额外设置 90 秒等待超时。请求超时不能中断已经在底层运行的翻译；队列中的请求在接收端关闭后会被跳过。
- 缓存最多 4 个不同语言方向的会话，避免无限占用内存。队列容量为 16，过载立即返回 HTTP 429。
- 健康检查只说明 HTTP 服务存活，**不保证**语言包已安装，也不保证特定语言方向可翻译。

