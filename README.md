# DanbooruDownload Tool Pro

本项目是一个仅限本机运行的 Danbooru 只读探索、批量下载与本地媒体管理工具。当前代码只有 Rust/Axum 后端与 Vue 3 前端；仓库中不存在旧版 Python/Gradio 实现。

## 主要功能

- `/explore`：使用 Danbooru 原生查询语法搜索、自动补全、数字页或 `a<ID>/b<ID>` 游标分页、帖子详情与媒体预览。
- `/tasks`：持久任务队列、实时 SSE 状态、暂停/恢复/取消/重试，以及可分页的下载记录和“再次下载”。
- `/library`：显式注册媒体根后再索引；游标分页、标签搜索、缩略图、原文件/视频 Range 访问。
- `/tools`：精确 SHA-256 去重、近似图片确认隔离、完整性检查、按精确标签隔离、安全 Resize、HEIC→JPEG、标签规范化与本地 vLLM 图片打标。
- `/settings`：Windows/Linux 路径映射、系统凭据、代理、下载并发、命名模板、Ugoira 策略及完整 vLLM 执行设置。

Questionable、Explicit 和未知分级默认模糊，必须主动揭示；视频不会自动播放。Ugoira 可保存 WebM 与原始 ZIP，SWF 不下载也不执行。

## 安全边界

- 服务默认且只允许监听 loopback；`HOST=0.0.0.0` 会被拒绝。
- 生产模式不开放 CORS；开发模式只允许 `127.0.0.1:5173` 与 `localhost:5173`。
- API 不接受任意绝对媒体路径，只接受已注册的根目录 ID、媒体 ID 和受校验的相对路径。
- Danbooru/vLLM 密钥优先写入 Windows Credential Manager 或 Linux Secret Service，配置响应只返回 `*_api_key_configured`，永不回显密钥。
- 系统凭据库不可用时只允许会话密钥，不会退化为明文持久化。
- vLLM 默认只允许 loopback；额外 HTTPS 地址必须在设置中显式允许。任务请求不能覆盖 vLLM 地址或密钥。
- 删除、去重和原地处理先生成预检清单，确认后移入媒体根内的 `.danbooru-quarantine`，不会直接永久删除原文件。

本工具不实现上传、改标签、投票、收藏、评论或权限绕过。它仍是本地应用，不应通过反向代理或端口转发暴露到局域网/互联网。

## 技术栈

- 后端：Rust 2021、Axum 0.8、Tokio、reqwest、rusqlite/SQLite、`image`、Rayon。
- 前端：Vue 3、TypeScript、Pinia、Vue Router、Tailwind CSS 4、Vite 8。
- 测试：Rust 单元/集成测试、Vitest + Vue Testing Library、Playwright + axe。

## 快速启动

需要 Rust 工具链、Node.js 20.19+ 或 22.12+，以及 npm。默认的一键启动还会尝试启动 vLLM：Linux 需要可用的 conda `vllm` 环境，Windows 需要 WSL2 内的 root 用户拥有该环境。
HEIC 转换另需在 `PATH` 中提供 `heif-convert`；工具缺失时任务会安全失败并保留/恢复原图。

Linux 一键启动：

```bash
chmod +x run.sh
./run.sh
```

Windows 可直接双击 `run.bat`，或在 PowerShell 中运行：

```powershell
.\run.bat
```

两个脚本都会从自身所在目录定位项目，Linux 在后台启动 `start_vllm.sh`，Windows 则通过 WSL2 打开独立可见窗口展示下载和加载进度。随后脚本检查 Node.js/npm/Rust、按锁文件安装前端依赖、构建生产前端与 release 后端，然后直接运行已构建的二进制。vLLM 模型加载与应用构建并行执行，侧栏状态会在模型可用后自动变绿。启动器会验证真实的 `/v1/models` 接口；默认 `8000` 被其他程序占用时会自动选择后续空闲端口，并把同一地址传给后端。已有模型不会重复启动，也不会杀掉占用端口的进程。依赖锁文件未变化时会跳过重复的 `npm ci`。

首次运行可能需要下载约 22GB 的模型权重；权重下载与 GPU 初始化完成前，接口会暂时显示离线。vLLM 启动失败不会阻止下载与图库功能，错误可在项目的 `logs/` 目录查看。如需只启动主应用，可显式关闭自动启动：

```bash
START_VLLM=0 ./run.sh
```

```powershell
$env:START_VLLM = '0'
.\run.bat
```

启动后访问 <http://127.0.0.1:8888>。首次构建耗时较长，后续启动会复用 npm 依赖与 Cargo 增量缓存。调试构建对应 `dev.sh` / `dev.bat`。

调试构建：

```bash
./dev.sh
```

需要 Vite 热更新时开两个终端：

```bash
cd backend
DEV_CORS=1 cargo run
```

```bash
cd frontend
npm install
npm run dev
```

Vite 地址为 <http://127.0.0.1:5173>，`/api` 会代理到 `127.0.0.1:8888`。

### 运行路径

后端不依赖当前工作目录，可通过环境变量覆盖：

| 变量 | 默认值 | 约束 |
|---|---|---|
| `HOST` | `127.0.0.1` | 必须是 loopback IP |
| `PORT` | `8888` | `1..=65535` |
| `DATA_DIR` | 项目根目录 | 必须是绝对路径 |
| `STATIC_DIR` | `frontend/dist` | 必须是绝对路径 |
| `DEV_CORS` | 未启用 | 仅值 `1` 时允许 Vite 来源 |
| `APP_ISOLATED_MODE` | 未启用 | 仅测试使用；值 `1` 时跳过旧数据迁移并只使用会话密钥 |
| `START_VLLM` | `1` | `1` 自动后台启动，`0` 跳过 |
| `VLLM_PORT` | `8000` | vLLM 本地端口 |
| `VLLM_HOST` | `127.0.0.1` | 默认仅 loopback |
| `MODEL_PATH` | `unsloth/Qwen3.6-27B-NVFP4` | 模型路径或模型 ID |
| `VLLM_CONDA_ENV` | `vllm` | conda 环境名 |
| `CONDA_SH` | 自动查找 | 可显式指定 `conda.sh` 路径 |
| `VLLM_TMPDIR` | `/tmp` | vLLM/ZeroMQ 的 WSL 临时目录，不能放在 `/mnt/c` |

`APP_ISOLATED_MODE=1` 由真实后端 E2E runner 使用，以确保测试不会读取或修改用户的旧 SQLite、配置或系统凭据；日常启动不要设置该变量。

## 配置与数据

新设置保存在 `DATA_DIR/app_settings.json`，SQLite 位于 `DATA_DIR/danbooru_tool.db`。密钥不写入设置文件。

首次启动以事务方式把 SQLite 升级到 schema v2，并迁移合法的非敏感旧设置；旧相对下载路径会解析为绝对路径建议，只有用户在设置页确认后才可注册，不会自动扫描、移动或删除旧媒体目录。旧 `config.json` / `vllm_config.json` 中的密钥只有在系统凭据库写入并回读成功后才会被清空。

媒体根同时保存 Windows 与 Linux/WSL 映射：

```json
{
  "name": "训练素材",
  "windows_path": "C:\\Media\\Danbooru",
  "linux_path": "/mnt/c/Media/Danbooru"
}
```

注册根目录本身、UNC/设备路径、越界符号链接、Windows 重解析点和保留设备名都会被拒绝。

## API 概览

成功体：

```json
{ "data": {}, "meta": {} }
```

错误体：

```json
{
  "error": {
    "code": "stable_machine_code",
    "message": "可读错误信息",
    "retryable": false,
    "fields": null
  },
  "request_id": "uuid"
}
```

主要接口：

| 方法 | 路径 | 用途 |
|---|---|---|
| GET | `/api/health` | 服务与数据库健康状态 |
| GET | `/api/vllm/health` | vLLM 与模型健康状态 |
| GET/PUT | `/api/config` | 读取/保存非敏感设置 |
| PUT/DELETE | `/api/secrets/{danbooru\|vllm}` | 保存/删除系统凭据 |
| GET/POST | `/api/library/roots` | 列出/注册媒体根 |
| PUT | `/api/library/roots/{id}` | 更新媒体根映射 |
| GET | `/api/library/items` | 本地图库游标分页 |
| GET | `/api/library/items/{id}` | 独立获取本地媒体详情 |
| GET | `/api/library/media/{id}/{thumbnail\|file}` | 缩略图或支持 Range 的原媒体 |
| GET/DELETE | `/api/library/quarantine` | 查看/清空隔离区 |
| POST | `/api/library/quarantine/{id}/restore` | 恢复隔离文件 |
| GET/POST | `/api/tasks` | 任务快照/创建任务 |
| GET | `/api/tasks/{id}` | 任务详情、失败项目与游标分页 |
| GET | `/api/tasks/events` | 全局 SSE 任务流 |
| POST | `/api/tasks/{id}/{pause\|resume\|cancel\|retry\|confirm}` | 任务动作 |
| GET | `/api/downloads/history` | 下载记录游标分页 |
| GET | `/api/danbooru/posts` | 原生查询与分页 |
| GET | `/api/danbooru/posts/{id}` | 帖子详情 |
| GET | `/api/danbooru/posts/{id}/media/{variant}` | 可信媒体代理 |
| GET | `/api/danbooru/autocomplete` | 标签自动补全 |
| GET | `/api/danbooru/count` | 查询数量估算 |

下载任务示例：

```json
{
  "type": "download",
  "root_id": "registered-root-id",
  "source": { "type": "query", "query": "1girl rating:g order:score" },
  "limit": 100,
  "concurrency": 8,
  "filename_template": "{id}_score_{score}.{ext}",
  "skip_existing": true,
  "media_policy": { "original": true, "ugoira": "webm_and_zip" }
}
```

`limit` 表示成功新增数；已存在内容会被跳过并继续翻页补足，范围为 `1..=10000`。下载采用有界并发、`.part`、Range 续传、长度/MD5 校验与原子重命名。

## 验证命令

```bash
cd frontend
npm run api:check
npm test
npm run typecheck
npm run build
npm run test:e2e
npm run test:e2e:real
npm audit --audit-level=high
```

```bash
cd backend
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked -- --test-threads=1
cargo audit
```

`npm run api:generate` 会先从 Rust 导出 `frontend/openapi.json`，再生成 `frontend/src/api/generated.ts`；`typecheck` 会拒绝过期契约。常规测试使用本地 mock，`test:e2e:real` 则启动正式 Rust 二进制与真实临时 SQLite，验证 SPA fallback、健康检查、索引、图库 Range 和下载记录，但不会访问 Danbooru/vLLM。

## 项目结构

```text
backend/src/
├── app_paths.rs
├── config.rs
├── database.rs
├── media_root.rs
├── routes/api.rs
├── secrets.rs
├── services/
│   ├── danbooru_client.rs
│   ├── image_processor.rs
│   └── vllm.rs
└── tasks.rs

frontend/src/
├── api/
├── components/
├── stores/
├── utils/
└── views/
    ├── ExploreView.vue
    ├── TasksView.vue
    ├── LibraryView.vue
    ├── ToolsView.vue
    └── SettingsView.vue
```

## License

MIT
