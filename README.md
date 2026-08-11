# DanbooruDownload Tool Pro

一个只在本机运行的 Danbooru 素材工作台：从检索与下载，到图库管理、标签处理、训练数据集增广、vLLM 二次打标和 LoRA 训练，都在同一个 Rust + Vue 应用中完成。

当前实现是 Rust/Axum 后端与 Vue 3 前端。仓库不包含旧版 Python/Gradio 应用；Python 只作为受 Rust 调度的训练和动漫检测运行时使用。

## 能做什么

- **探索**：使用 Danbooru 原生查询语法搜索、标签自动补全、游标分页、帖子详情和受控媒体预览。
- **下载与任务**：可暂停、恢复、取消和重试的持久任务队列；通过 SSE 实时显示进度、失败项和下载记录。
- **图库**：先显式注册媒体根，再索引、预览和按标签搜索本地媒体；可按文件夹浏览，根目录图片不会再和子目录图片混在一起。
- **工具**：完整性检查、精确去重、相似图片检查、隔离式删除、安全缩放、HEIC 转换、标签流水线与本地 vLLM 图片打标。
- **数据集增广**：原图保护、family 防泄漏切分、GPU 动漫检测/分割/智能裁剪、无损派生图、重新打标门控和训练子集清单。
- **训练**：受支持的 kohya_ss 训练入口、运行时诊断、GPU 独占队列、实时日志/指标/样图；可把原图和每种已重新打标的派生数据作为独立子集，并分别设置 repeat。
- **设置**：媒体根的 Windows/WSL 映射、Danbooru/vLLM 凭据、代理、下载命名、vLLM 服务和训练运行时配置。

Questionable、Explicit 和未知分级默认模糊，必须主动揭示；视频不会自动播放。Ugoira 可保存为 WebM 和原始 ZIP；SWF 不会下载或执行。

## 本地安全边界

- 服务只监听 `127.0.0.1`；设置 `HOST=0.0.0.0` 会被拒绝。
- 生产模式不开放 CORS；开发模式只允许 Vite 本地来源。
- API 只接受已注册媒体根、媒体 ID 和受校验的相对路径，不提供任意绝对路径读写。
- Danbooru 与 vLLM 密钥优先保存在 Windows Credential Manager 或 Linux Secret Service；配置接口不会回显密钥。系统凭据库不可用时只允许会话密钥。
- 删除、去重和原地处理先生成预检清单，确认后移入媒体根内的 `.danbooru-quarantine`，而非直接永久删除。
- 动漫检测 Python worker 只接收 Rust 验证过的图片清单并返回 JSONL；它不拥有媒体目录写权限。
- vLLM 默认仅允许 loopback 地址；额外 HTTPS 地址必须在设置中显式允许。

这是本地桌面应用，不应通过反向代理、端口转发或公网暴露。

## 技术栈

- 后端：Rust 2021、Axum 0.8、Tokio、SQLite、reqwest、`image`、Rayon、utoipa。
- 前端：Vue 3、TypeScript、Pinia、Vue Router、Tailwind CSS 4、Vite 8。
- 训练：随应用管理的 kohya_ss v26.0.0 运行时，以及可发现的 Windows、WSL、Conda 和 Python venv 运行时。
- 动漫裁剪：`dghs-imgutils[gpu]==0.19.0`、`rtmlib==0.0.16`、ONNX Runtime CUDA 与 PyTorch CUDA。
- 测试：Rust 单元/集成测试、Vitest + Vue Testing Library、Playwright + axe。

## 启动

需要：Rust 工具链、Node.js **20.19+ 或 22.12+**、npm。首次启动会安装前端依赖、构建前端和 release 后端。

Windows：双击 `run.bat`，或在 PowerShell 中运行：

```powershell
.\run.bat
```

Linux / WSL：

```bash
chmod +x run.sh
./run.sh
```

启动完成后访问 <http://127.0.0.1:8888>。启动脚本从自身位置定位项目，默认不会加载 vLLM 或动漫检测模型，因此不会因为打开应用而占用 GPU 显存。

调试启动：

```bash
./dev.sh
```

需要 Vite 热更新时，分别运行：

```bash
cd backend
DEV_CORS=1 cargo run
```

```bash
cd frontend
npm run dev
```

Vite 服务位于 <http://127.0.0.1:5173>，`/api` 会代理至后端。

### 常用运行环境变量

| 变量 | 默认值 | 说明 |
|---|---|---|
| `HOST` | `127.0.0.1` | 必须为 loopback IP。 |
| `PORT` | `8888` | 主服务端口。 |
| `DATA_DIR` | 项目根目录 | 配置、数据库、训练运行记录的绝对目录。 |
| `STATIC_DIR` | `frontend/dist` | 前端构建产物的绝对目录。 |
| `DEV_CORS` | 未启用 | 仅 `1` 时允许本地 Vite 来源。 |
| `START_VLLM` | `0` | 设为 `1` 才会随启动器尝试后台启动 vLLM。 |
| `VLLM_HOST` / `VLLM_PORT` | `127.0.0.1` / `8000` | 本地 vLLM 地址。 |
| `MODEL_PATH` | `unsloth/Qwen3.6-27B-NVFP4` | vLLM 模型路径或模型 ID。 |
| `VLLM_CONDA_ENV` | `vllm` | vLLM conda 环境名。 |

HEIC 转换另需将 `heif-convert` 放入 `PATH`。缺失时该任务会失败并保留原文件。

## 首次配置

1. 打开“设置”，配置 Danbooru 账号/API Key（如需下载）、代理和下载命名规则。
2. 在“图库”或“设置”注册素材根目录。根目录需要显式确认；应用不会自动扫描、移动或删除你的任意磁盘目录。
3. Windows 与 WSL 同时使用时，为同一媒体根填写两侧映射，例如：

   ```json
   {
     "name": "训练素材",
     "windows_path": "C:\\datasets\\anime",
     "linux_path": "/mnt/c/datasets/anime"
   }
   ```

4. 通过任务或图库操作刷新索引。图库会保存受控媒体记录；增广输出也会自动注册，可直接按输出文件夹预览。

设置写入 `DATA_DIR/app_settings.json`，SQLite 数据库位于 `DATA_DIR/danbooru_tool.db`。密钥不写入设置文件。

## 数据集增广与智能裁剪

入口为“工具 → 数据集增广”。该任务在所选媒体根内创建唯一任务文件夹，**绝不覆盖输入图片或输入标签**。默认分辨率门槛为 1.8 MP、长边 1536、短边 768；只处理 PNG、JPEG、WebP 和 BMP 静态图片。

### 输出与训练门控

每个任务大致生成如下结构：

```text
dataset-expanded/<task-id>/
├── raw/
│   ├── images/                 # 保留原格式的原图副本
│   └── labels/                 # 原图 Caption
├── derived/
│   ├── horizontal_flip/images/
│   ├── portrait/images/
│   ├── upper_body/images/
│   └── full_body_tight/images/
├── ready/
│   ├── train/ validation/ test/
│   └── .../<variant>/images/   # 仅可安全训练的派生子集
├── metadata/
│   ├── dataset.jsonl
│   ├── families.jsonl
│   ├── retagging.jsonl
│   └── training-subsets.json
├── splits/
├── rejected/rejections.jsonl
└── READY.json
```

任务创建时先写入 `INCOMPLETE.json`；只有正常完成才生成 `READY.json`。训练页面会拒绝不完整输出。

- 原图保留原文件和原 Caption，可直接进入 `ready`。
- 翻转、肖像、上半身与紧凑全身等派生图统一以**无损 PNG**保存，避免二次 JPEG 压缩丢失细节。
- 派生图不会复制原 Caption，因为构图变化后原标签可能已不匹配。它们会被标记为 `requires_retagging=true`，记录到 `metadata/retagging.jsonl`，且不会进入 `ready`。
- 可在创建任务时选择“发送派生图到 vLLM 二次打标”。仅 vLLM 真正写入非空新 Caption 的派生图才会进入相应 `ready/<split>/<variant>/images` 子集；失败项保持待打标，不会回退使用原标签。
- 可选择将原图的 artist / character 标签放在 vLLM 新标签最前，保持逗号分隔并去重。
- `metadata/dataset.jsonl` 记录原生尺寸、推荐 bucket、切分、来源、family 与重打标状态。bucket 仅作为训练建议，任务不缩放图片来匹配 bucket，更不会非等比拉伸。
- 通过稳定的 `family_id` 分配 train / validation / test。同源原图、翻转和裁剪始终位于同一 split，不会发生 family 泄漏。

### GPU 动漫检测、分割与裁剪

智能裁剪默认开启，默认运行时为 `conda:lora`、GPU `0`、质量档 `anime-quality`。首次使用在任务配置中点击“安装并预热检测模型”，随后使用“检查运行时”确认环境。

安装器会在所选 Python 环境中安装并锁定 `dghs-imgutils[gpu]==0.19.0` 与 `rtmlib==0.0.16`，然后预热模型。健康检查会验证 Python、CUDA provider、物理 GPU、实际 ONNX CUDA session、模型下载和一次推理；不满足条件时裁剪任务会在写入工作区前失败，**不会静默退回 CPU**。

裁剪证据来自：

- DeepGHS 高精度动漫头部模型 `head_detect_v2.0_x_yv11`；
- 动漫人物、人脸、半身和手部检测；
- 单主体可信时使用 ISNet 动漫前景分割作为保护边界；
- HumanArt 人物检测与 RTMPose，检查躯干、脚踝和人物边缘截断。

Rust 端会关联同一主体的检测结果，最多尝试肖像、上半身、紧凑全身三种构图。候选必须满足原生分辨率、构图比例与明显裁剪幅度，并完整保留高置信脸、头部、手部及相应的完整身体关键点。多人严重重叠、第二人明显进入候选框、主体太小、关键部位可能被切断、姿态不完整或候选接近原图时，系统宁可拒绝，原因会写入 `rejected/rejections.jsonl`。

智能裁剪与 LoRA 训练使用同一 GPU 独占队列。检测预检发现显存不足或有外部进程占用时会提示释放显存；应用不会终止外部 vLLM 或其他进程。

### 推荐的正式 LoRA 工作流

1. 注册并索引原始数据集；先在图库检查分辨率、损坏图片和重复图。
2. 在“工具 → 数据集增广”选择源目录，保留默认严格门槛，按需要启用翻转和三种裁剪。
3. 需要派生图时，启用 vLLM 二次打标；想保留身份信息时勾选 artist / character 前置。
4. 在任务结果中查看 `retagging_pending` 和 `training-subsets.json`。未重新打标的派生图应继续保留在待处理状态，不能作为训练输入。
5. 在“训练 → 从图库引用”中选择 `ready/train/images` 与需要的 `ready/train/<variant>/images`。每个目录作为独立子集，分别设置 repeat 与 Caption 扩展名；应用会生成多 `[[datasets.subsets]]` 的训练配置，不复制这些文件。
6. 选择底模、运行时与 GPU，先运行诊断；训练进入 GPU 独占队列后，在训练监控页查看日志、指标、样图与产物。

## vLLM 图片打标

vLLM 是可选功能。可以在“设置 → 本地服务”按需加载，也可设置 `START_VLLM=1` 交给启动器尝试后台加载。默认端点为 `http://127.0.0.1:8000/v1`。

常规图库打标可由设置决定语言、系统提示词、最大标签数、并发、Danbooru 校验以及覆盖/追加写入模式。数据集增广的二次打标始终以覆盖方式写入全新派生 Caption，不引用原 Caption，避免把裁剪前的描述带入新样本。

## API 概览

成功响应：

```json
{ "data": {}, "meta": {} }
```

错误响应：

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

常用端点包括：

| 方法 | 路径 | 用途 |
|---|---|---|
| GET | `/api/health` | 应用与数据库健康状态。 |
| GET | `/api/vllm/health` | vLLM 模型服务状态。 |
| GET/POST | `/api/library/roots` | 列出或注册媒体根。 |
| GET | `/api/library/roots/{id}/directories` | 获取媒体根下可选目录。 |
| GET | `/api/library/items` | 分页读取图库，支持目录与标签过滤。 |
| GET/POST | `/api/tasks` | 创建任务与读取任务快照。 |
| GET | `/api/tasks/events` | 全局 SSE 任务流。 |
| POST | `/api/tasks/{id}/{pause|resume|cancel|retry|confirm}` | 控制任务。 |
| GET | `/api/vision-crop/runtime-profiles/{id}/health` | 检查动漫裁剪 CUDA/模型运行时。 |
| POST | `/api/vision-crop/runtime-profiles/{id}/install` | 安装并预热动漫检测、分割和姿态模型。 |
| GET | `/api/training/gpus` | 读取可训练 GPU 与进程状态。 |
| GET/POST | `/api/training/*` | 训练适配器、运行时、预设、数据集预检和训练任务。 |

完整的 OpenAPI 契约由 Rust 导出到 `frontend/openapi.json`，并生成 `frontend/src/api/generated.ts`。

## 项目结构

```text
backend/src/
├── routes/api.rs                 # REST、SSE、任务执行与训练/裁剪调度
├── services/
│   ├── danbooru_client.rs
│   ├── dataset_augmentation.rs   # family、派生图、训练门控与 metadata
│   ├── image_processor.rs
│   └── vllm.rs
├── training.rs                   # 训练适配器、运行时与 dataset TOML
├── database.rs
├── media_root.rs
└── tasks.rs

frontend/src/
├── views/                        # Explore、Tasks、Library、Tools、Training、Settings
├── api/                          # Axios 封装与生成的 OpenAPI 类型
├── stores/
└── components/

training_runtime/anime_crop_worker.py
                                # CUDA 动漫检测、分割、RTMPose JSONL worker
```

## 验证命令

前端：

```bash
cd frontend
npm run api:check
npm test
npm run typecheck
npm run build
npm run test:e2e
npm run test:e2e:real
```

后端：

```bash
cd backend
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked -- --test-threads=1
```

`test:e2e:real` 会启动正式 Rust 二进制和临时 SQLite，以验证 SPA fallback、健康检查、图库 Range、索引和下载记录；它不会访问 Danbooru 或 vLLM。GPU 动漫裁剪须在实际 `conda:lora` 与目标显卡环境中，通过界面的安装/预热与运行时检查验收。
