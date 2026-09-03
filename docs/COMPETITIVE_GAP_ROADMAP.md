# 竞品差距路线图（v0.2+）

- 状态：执行中（2026-09-01 两波已落地：G-01/02/03/05/10/11/12/13/20/21/22/30/33 全部完成——含 qpdf 四操作、REST API、ODF/RTF 输入、starter 断言、updater、官网；第三波亦落地：G-23 水印、G-31 轨道 UI、G-32 目标体积、G-34 CI 泛化、G-35 demo 页；第四波落地：Word 全目标导出、docx↔odt、7z、PDF 元数据、HEIC、OCR 代码就绪（引擎按用户决定暂缓安装）；三平台 CI 首次全绿。剩余：G-04 正式签名账户、G-24 OCR 引擎安装、CI 上的 nightly 长跑）
- 版本：0.1
- 更新：2026-09-01
- 来源：2026-09-01 竞品调研（VERT 15.4k★ / File Converter / Stirling-PDF / Gotenberg / HandBrake）+ 源码深度审读
- 上游主表：[`MASTER_EXECUTION_PLAN.md`](MASTER_EXECUTION_PLAN.md)（本文只管 v0.2+ 的差距补齐；v0.1 收尾仍以主表为准）
- 对齐文档：[`specs/FORMAT_SUPPORT_MATRIX.md`](specs/FORMAT_SUPPORT_MATRIX.md)、[`../engines/README.md`](../engines/README.md)、[`../implementation-notes.md`](../implementation-notes.md)

## 0. 定位原则（决定取舍）

不与 VERT 拼"格式数量"（WASM 浏览器路线的主场）。所有新增能力延续本项目独有叙事：**每次操作产出机器可读的验收证明**。因此每个新 lane 必须自带验收检查（下文逐项列出），这是进入本文档的准入条件。

编号规则：G-xxx（Gap），与主表 R-xxx 不冲突。规模：S ≤ 3 天 / M 1-2 周 / L ≥ 3 周（单人）。

## 1. Wave 1 — 发布收尾（P0，阻断开源首版 Release）

与主表 §4.1 P0 合流，此处只列竞品压力催生的增量：

| 编号 | 内容 | 源码/脚本落点 | 验收标准 | 规模 |
|---|---|---|---|---|
| G-01 | GW-10 浏览器打印 lane 的正式沙箱工件：`test_browser_print_sandbox.ps1` + `docs/testing/BROWSER_PRINT_SANDBOX.md`，含已提交 HTML/SVG 试样与 pinned 引擎身份（msedge 版本目录 + Poppler 26.02.0 哈希） | scripts/、docs/testing/ | 沙箱脚本在干净 shell 全绿；`FORMAT_SUPPORT_MATRIX.md` GW-10 行去掉 evidence caveat | S |
| G-02 | starter pack 生成进入发布流：`release-candidate.yml` 调 `prepare/build_windows_starter_pack.ps1`；RELEASE_CHECKLIST 增加非空 `dist/engine-packs` 断言（空目录绕过不得入发行版） | .github/workflows/、docs/release/RELEASE_CHECKLIST.md | CI 产出的安装包含 Poppler/FFmpeg starter，首次启动激活通过 | S |
| G-03 | Tauri 自动更新（updater 插件 + 签名密钥，密钥路线复用 ADR-0011 keyring 体系） | apps/desktop/src-tauri/ | 升级+回滚烟测通过；更新签名可验证 | M |
| G-04 | 正式代码签名（Windows）与公证（macOS，随 G-34） | docs/release/WINDOWS_PACKAGING.md | 签名 exe 通过 SmartScreen 信誉路径 | M |

## 2. Wave 2 — 快赢四项（P1：小投入、高感知，全部带验收）

| 编号 | 内容 | 源码落点 | 引擎 | 验收标准 | 规模 |
|---|---|---|---|---|---|
| G-10 | **epub 目标格式**：md/html/docx → epub | `capabilities.rs`（KNOWN_TARGETS + `required_engines` 放行 epub）、`runner.rs` pandoc 执行、`document.rs`（无需新识别，输入已支持） | pandoc（已登记） | 输出为合法 zip 容器 + mimetype 首项 + XHTML 可解析 + 章节文字抽样与输入一致（新增 EPUB_* 检查组） | S |
| G-11 | **归档互转**：zip / tar.gz / 7z 打包与解包 | 新 `crates/core/src/archive.rs`（Rust 原生 `zip`/`tar` crate，7z 视 crate 许可审查）；GW-11 同款零外部引擎路线 | 无（原生） | 条目数守恒 + 逐条目内容哈希清单写入验收报告 | M |
| G-12 | **PDF 合并/拆分/提取页**：对 Stirling 核心功能的第一击 | `capabilities.rs` 新 qpdf lane（操作型路由，需扩展路由模型从"格式转换"到"PDF 操作"）、新 `pdf_ops.rs`、`doctor.rs` qpdf 已在发现清单 | qpdf（Apache-2.0，engines/README 已标"首选结构候选"） | **页数守恒**（合并=各输入和 / 拆分=输入页数）+ pdfinfo 复验 + pdftotext 文字层保留率 ≥ 基线；操作型验收报告 PDF_OPS_* 检查组 | M-L |
| G-13 | **视频/音频参数暴露**：编码器（H.264/265/AV1/VP9）、CRF/码率、帧率、preset，进 PlanRequest、UI 与预设 | `domain.rs`（PlanRequest 扩展，schema 版本升级）、`runner.rs:2175` 解除 `-preset medium -crf 20` 硬编码、App.tsx 参数区、i18n | ffmpeg（已有） | Plan 显式记录所选参数（可解释性不回退）+ ffprobe 对输出侧复核编码参数一致；预设可保存/迁移 | M |

依赖：G-12 需要先设计"操作型工作流"的路由模型（输入=多文件+操作名，输出=单 PDF），是本 Wave 唯一的架构决策点，建议配 ADR-0013。

## 3. Wave 3 — PDF 工具箱（P2a：Stirling 对位，逐项可独立发布）

全部构建在 G-12 的操作型路由与 qpdf lane 之上：

| 编号 | 内容 | 技术路线 | 验收标准 | 规模 |
|---|---|---|---|---|
| G-20 | 旋转/裁剪/N 页取 M | `qpdf --rotate/--pages` | 页数守恒 + 渲染抽页方向验证（pdftoppm 首页像素宽高比） | S |
| G-21 | PDF 压缩 | qpdf 流重组/线性化（图像重压缩有限；Ghostscript 因 AGPL 需单独 ADR 决策，默认不用） | 压缩率写入报告 + pdftotext 文字层不退化 | M |
| G-22 | 加密/解密/权限 | `qpdf --encrypt/--decrypt`（crypto-provider 选择按 engines/README 决策点） | 加密后 pdfinfo 拒读明文 + 解密回读与源哈希对比 | M |
| G-23 | 水印/页眉页脚 | `qpdf --overlay/--underlay`（水印 PDF 由浏览器打印 lane 从 HTML/SVG 模板生成——两条自有 lane 组合，竞品无此能力） | 每页水印存在性（渲染抽检）+ 文字层保留 | M |
| G-24 | OCR（扫描件 → 可搜索 PDF） | Tesseract 新引擎登记：manifest 模板 + 许可证审查（引擎 Apache-2.0；训练数据许可单独盘）；集成参考 OCRmyPDF 思路但自建管线 | **pdftotext 输出非空且抽样词命中源图 OCR 文本**——与本项目验收文化天然契合 | L |
| G-25 | PDF 元数据查看/编辑 | Rust 原生或 qpdf；低优先 | 元数据回读一致 | S（低优） |

## 4. Wave 4 — 广度与服务化（P2b/P3）

| 编号 | 内容 | 说明 | 验收标准 | 规模 |
|---|---|---|---|---|
| G-30 | ODT/RTF/TXT 输入白名单 | soffice/pandoc 本身支持，`capabilities.rs` 放行即可，Office 通道边际成本极低 | 既有 Office 验收链直接复用 | S |
| G-31 | 轨道选择 UI | Probe 已产出完整流清单（音频/字幕轨），UI 勾选 + PlanRequest 传递，MP4 baseline 从"自动决策"升级为"默认自动+可覆盖" | Plan 中每条 dropped/preserved 轨道与用户选择一致 | M |
| G-32 | 压缩到目标大小 | 迭代参数循环（视频 CRF 步进 / qpdf 压缩档位），上限 N 次迭代防振荡 | 输出体积 ≤ 目标（或报告明确"最近可达档位"）+ 质量抽检 | M |
| G-33 | REST API 服务 | 对齐主表 Phase 6 既有规划（Axum + OpenAPI + SSE）：薄层包 `workflow.rs`；**每个响应附带验收报告**是相对 Gotenberg 的差异化 | OpenAPI 契约测试 + 沙箱重放 | L |
| G-34 | macOS/Linux | 主表 Phase 1 CI 既有方向：`known_install_location` 泛化（Chromium 系布局）、Finder/Nemo 集成 | 三平台 doctor + 冒烟矩阵 | L |
| G-35 | Web 前端 / 在线 demo（可选） | Phase 7 WASM 方向；仅当 API（G-33）落地后评估 | — | L（可选） |

## 5. 排序依据与关键路径

```text
G-01..G-04 (W1 发布) ──→ 开源首版 Release
G-10 ─┐
G-11 ─┼─ 互相独立，可并行
G-13 ─┘
G-12 (操作型路由 ADR-0013) ──→ G-20..G-25 (PDF 工具箱逐项)
G-33 (API) ──→ G-35 (Web, 可选)
G-34 独立，随时可插入 CI 空档
```

- 建议节奏：**W1 → G-10/G-11/G-13 并行 → G-12 → W3 按需求热度逐项 → W4**。
- 唯一的新架构决策：G-12 的操作型路由（多输入+操作名的 Plan/验收模型扩展），先行 ADR。
- 所有新增引擎（qpdf、Tesseract）走 engines/README 既有登记流程（manifest/许可/哈希审查），不改"不打包"立场。

## 6. 明确不做（本轮调研结论）

- CAD（DXF/DWG）转换：VERT 亦无，受众窄，管线重。
- PDF → DOCX"可编辑还原"：质量无法用本项目验收标准证明，违背定位。
- Ghostscript：AGPL 决策未做之前不引入（G-21 用 qpdf 顶）。
- 拼格式数量：VERT 的 WASM 路线主战场，不跟进。

## 7. 竞品复核：howtoconvert.co（2026-09-02）

$29 买断闭源本地转换器，5,438 种转换（大量多阶段链 + RAW 长尾排列），三平台 GUI。复核结论：格式广度被碾压，但零输出验证、闭源、仅 GUI。我方护城河：每条路由带机器可读验收、开源（Apache-2.0，业主明确不采用买断定价）、CLI+GUI+API 三形态、持久队列工程。采纳的打法（不做格式数量竞赛）：
- **转换链图搜索**（C-01）：lane 架构天然支持多段转换，一段中间格式即可把可达组合从 68 扩到数百，且每段保留独立验收——竞品的链没有证明，我们的链有。
- **EML 邮件族**（C-02）：竞品 113 条邮件转换全部无安全处理；我方 EML→HTML 剥 script/远程资源（邮件是不可信输入），EML→PDF 经链获得。
- 明确不做：买断定价（业主秉持开源）；真矢量化暂缓——potrace 为 GPL 与 Apache-2.0 打包冲突，且"真矢量化"需专门算法，留待评估。
