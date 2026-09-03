# FormatWright — 下一阶段执行计划：通往 v0.1 Public Beta

> 状态基线：2026-09-02 · 319 条可达转换组合 · 三平台 CI 绿 · 双执行环境（Windows 权威源 + Linux 执行机）

## 文档信息

| 字段 | 内容 |
|---|---|
| 文档类型 | 阶段执行计划（可直接开工的批次分解） |
| 版本 | 1.0 |
| 更新日期 | 2026-09-02 |
| 权威性 | 执行事实以本文件为准；产品范围仍以 [`SPEC_PLAN.md`](../../SPEC_PLAN.md) 为准 |
| 执行环境 | Windows（权威源、GUI 验证、发布构建）+ macair-away Linux（编译/测试/长跑，规则见用户授权） |
| 上游文档 | [`COMPETITIVE_GAP_ROADMAP.md`](../COMPETITIVE_GAP_ROADMAP.md)（已完成 19 项）、[`MASTER_EXECUTION_PLAN.md`](../MASTER_EXECUTION_PLAN.md)（R-00x 门槛） |

## 0. 当前状态快照（全部有证据）

| 能力 | 状态 | 证据 |
|---|---|---|
| 转换路由 | 228 直接 + 91 链 = **319 组合** | 矩阵 68/68 实测（Windows）、28/29（Linux） |
| PDF 操作 | 7 种（合并/拆分/旋转/压缩/加密/解密/水印） | 全部守恒验收 + 独立 Poppler 复核 |
| 文档族 | md/html/txt/docx/odt/ods/odp/pptx/xlsx/rtf/eml/epub/svg | e2e 全绿（含 Word→四目标、EML 安全净化） |
| OCR | 图片→txt、pdf-ocr | Linux e2e 一字不差 |
| 归档 | zip/tar.gz/7z 三向互转 | 清单守恒 + 往返逐字节 |
| 形态 | 桌面 GUI + CLI + REST API + 官网/demo | 全部运行中 |
| 质量 | 244 测试 + 4 环境基线 | 三平台 CI 绿（run 33626245570） |
| 待外部 | 正式签名证书（购买）、格式长尾 | — |

## 1. 阶段目标

**发布 v0.1.0 Public Beta**：GitHub Release 带签名安装包 + SBOM + 官网链接生效，达到 SPEC_PLAN §0.3 的成功定义中可关闭项的全部关闭。

排序原则：先解锁/闭环（批次 A），再收发布门槛（批次 B），格式长尾在发布后持续（批次 C）。竞品（howtoconvert.co 5,438 条）追赶不设数量指标——每条路由必须带验收才计数。

---

## 批次 A — 跨平台闭环 + 链集成（预估 1 个工作日）

### A1 Linux/macOS 浏览器引擎发现（G-34 收尾）
- **范围**：`doctor.rs` 的 `known_install_location` 增加分支：Linux 上发现 `google-chrome`/`chromium`/`microsoft-edge`（`/usr/bin`、`/opt/google/chrome`、snap 路径）；复用 macOS 已有的 Chromium bundle 布局辅助函数。SVG→PDF 浏览器通道接受 Chromium 家族，不只 Edge（ADR-0012 语义不变：系统发现、绝不打包）。
- **环境**：Windows 改码 + Linux e2e（Linux 需装 chromium——conda 无浏览器，用 apt（需用户 sudo 授权）或评估下载 Chrome for Testing 独立构建到用户目录）。
- **验收**：Linux doctor 报告浏览器 available；Linux 矩阵 28/29 → **29/29**（svg→pdf 转绿）；macOS CI clippy 保持绿。

### A2 转换链入持久队列与 GUI
- **范围**：`--queue-only` 支持链（当前链仅即时执行）；capability snapshot 对链可达路由标注 `via <mid> chain`，GUI 目标下拉自动显示（前端已由 snapshot 驱动，只加标注字段）；staging 目录 `.fw-chain-*` 纳入现有崩溃恢复清理清单（`staged_output_candidates`）。
- **验收**：链任务崩溃后重启无残留；GUI 对 eml 显示 pdf 目标（经链）；单测覆盖队列化链的恢复。

### A3 Linux 侧矩阵脚本入库
- **范围**：`scripts/test_conversion_matrix_linux.sh`（今日临时脚本转正）+ README 记录双平台矩阵跑法。
- **验收**：Windows/Linux 双矩阵脚本各自全绿文档化。

---

## 批次 B — v0.1 发布收口（预估 1–2 个工作日，含等待）

### B1 10,000 混合长跑（R-008 部分）
- **范围**：在 Linux 执行机跑 `cargo test -p formatwright-core --test ten_thousand_conversions --release -- --ignored --nocapture` 与 mixed 十千测试（引擎用 conda 环境）。修复暴露的问题，记录 P50/P95/RSS 到证据文档。
- **环境**：Linux（长跑不占 Windows）。
- **验收**：两个 10k 测试全绿；`docs/testing/TEN_THOUSAND_LINUX.md` 记录证据。

### B2 供应链与签名流程就绪（R-009）
- **范围**：`generate_engine_sbom.py` 产出全部实际使用引擎（Poppler/qpdf/libheif/LibreOffice/pandoc/tesseract/ffmpeg）的 SBOM 汇总；CI 增加 release-candidate 流程演练（已存在，跑一次 workflow_dispatch 验证全链）；正式证书购买后替换测试签名（自签链路已验证通）。
- **验收**：SBOM 覆盖 7 引擎 + 版本 + 哈希；release-candidate run 绿；发布清单勾选。

### B3 v0.1.0 发布日
- **范围**：tag `v0.1.0` → release-candidate workflow 产出 NSIS 安装包 + `dist/SHA256SUMS` + SBOM 附件 → GitHub Release 发布 → 官网 `REPO_URL` 下载链接指向 Release 资产 → website 部署 GitHub Pages → demo 页指向公网 API 说明（本地 API 文档化）。
- **依赖**：正式签名证书（B2）可选——无证书时按"Unsigned Alpha"语义发布并在 Release 说明标注。
- **验收**：Release 页可下载安装包，官网下载按钮可用，三平台 CI 在 tag 上绿。

### B4 发布后公告物料（可选）
- **范围**：README 徽章（CI/license/matrix）、Release Notes（中英）、社交文案存 `docs/release/v0.1.0_notes.md`。

---

## 批次 C — 格式长尾第一波（发布后持续，每波 0.5–1 日）

> 原则：每条路由必须带验收才计入；许可证不兼容的依赖一律走"引擎发现"不打包。

### C1 图像长尾（对标竞品 2,181 条图像转换的常用子集）
- TIFF/BMP 输入（LibreOffice draw lane 已能转 pdf；tiff→jpg 评估 ffmpeg 已能解）→ 先测现有引擎能覆盖多少，再补路由。
- RAW（DNG/CR2/ARW）：调研 LibRaw（LGPL-2.1，只能引擎发现不能打包）vs dcraw 公有域；倾向 dcraw/RawTherapee CLI 作为可选发现引擎。
- PSD：评估 ImageMagick（Apache-2.0 可打包！）作为新引擎 lane。
- **验收**：每格式至少一条 e2e + 矩阵脚本扩充。

### C2 MSG（Outlook）输入
- **范围**：调研纯 Rust 的 OLE 复合文档解析（`cfb` crate 等）+ MSG→EML 转换路径（转 EML 后链自动覆盖 txt/html/pdf）。
- **验收**：真实 Outlook 导出的 .msg fixture e2e；无法安全解析时 fail-closed 并文档化。

### C3 邮箱聚合（MBOX/EML→PDF 合并）
- **范围**：MBOX 解析拆 EML + 链 + PDF 合并操作组合 = "整个邮箱导出一个 PDF"——竞品没有的打法和我们有 PDF 合并证明的独有组合。
- **验收**：多样 MBOX fixture e2e + 页数守恒证明。

---

## 明确不做（本阶段）

| 项 | 理由 |
|---|---|
| 买断/订阅定价 | 业主决定秉持开源（2026-09-02） |
| 真矢量化 | potrace GPL 与 Apache-2.0 打包冲突；真矢量化需专门算法，单独立项评估 |
| Web 在线服务托管 | 属 SPEC_PLAN Phase 7；demo 页已够当前叙事 |
| 5,000+ 格式数量竞赛 | 无验收的广度不是护城河 |

## 执行顺序与依赖

```
A1 ──→ A2 ──→ A3
        │
        └──→ B1（Linux 长跑，可与 A2 并行）
              └──→ B2 ──→ B3（发布）──→ B4
                                └──→ C1/C2/C3（发布后持续）
```

外部依赖唯一项：**正式签名证书**（B3 可无证书先行发布 Unsigned Alpha，证书到位后补 v0.1.1 签名版）。
