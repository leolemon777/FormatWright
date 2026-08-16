# FormatWright Agent 交接说明

- 交接日期：2026-08-15
- 仓库：`E:\Users\Administrator\Desktop\FormatWright`
- 分支：`main`
- 当前 HEAD：`ed9afd1 test: certify installed shell and desktop accessibility`
- 工作树：**有一组尚未提交的 Engine SBOM / Release UI E2E 改动，必须保留**
- 产品状态：Windows 自包含开发候选可用；尚未达到 Public Beta / Certified Release

## 1. 一句话结论

FormatWright 的 Windows 主链路已经从“依赖系统工具且 PDF 无法转换”推进到：安装器内嵌固定版本 PDF/Media Starter，引擎首次启动时按哈希验证并原子安装，Release 只解析激活包的精确二进制路径，PNG/JPG 路线由前后端共同门控；真实安装、Explorer 入口、无障碍和本机转换已有证据。

当前正在收口的是每个引擎包的确定性 SPDX 文件 SBOM、来源侧车和安装后完整性验证。代码与绝大多数测试已通过，但本批次尚未提交，最终标准 NSIS 也需要在最后一处前端修正后重新构建。

## 2. 已稳定完成并提交

最近三个稳定提交：

1. `ed9afd1`：真实 current-user NSIS 安装、Explorer 文件/目录 Shell verb、Unicode/空格路径、冷/热单实例、卸载精确清理；真实 Tauri/WebView2 无障碍、200% 等效视口、RTL、reduced-motion/高对比/forced-colors。
2. `4c83f14`：Windows Explorer 壳入口和单实例路径转发。
3. `f9ee20f`：真实转换阶段和调度等待信息，不伪造进度、速度或 ETA。

更早已完成的核心能力见 `docs/MASTER_EXECUTION_PLAN.md`，包括：

- SQLite v5 持久任务、Batch、Selection、Bulk Action、幂等键和追加式 revalidation 证据。
- `ConversionService`、`ReportService`、`JobExecutionService` 统一 CLI/Desktop 生命周期。
- Windows 进程树取消、原子 no-clobber 提交、恢复、备份/恢复/完整性检查。
- 10,000 项结构化和混合转换门禁、多进程原子认领、真实强退恢复。
- Windows Starter PDF/Media 包、Release 精确引擎解析、capability snapshot 前后端门控。
- 真实 15 页用户 PDF 的 PNG/JPG 修复和验证；R-010 已关闭。

## 3. 当前未提交批次：Engine 文件 SBOM 与来源侧车

### 3.1 已实现

- 新增 `scripts/generate_engine_sbom.py`：从 Engine Manifest 确定性生成 SPDX 2.3 JSON；记录 Manifest 声明的 executable、runtime、license/source-offer 和 `sources.json` 的 SHA-1/SHA-256；拒绝穿越、重复、缺项和篡改。
- 新增 `scripts/test_engine_sbom.py`：覆盖字节级确定性、完整清单、runtime 篡改和路径穿越。
- Engine Manifest v1 新增可选 `supply_chain`：`sbom_path`、`sbom_sha256`、`sources_path`、`sources_sha256`。
- SDK 校验安全相对路径、SHA-256、包内路径唯一性；license/source-offer 也纳入重复路径检查。
- Core 在 import/install 时验证：
  - 两个侧车文件哈希；
  - SPDX 2.3 / CC0-1.0 和 engine/version 身份；
  - executable、runtime、license/source-offer、`sources.json` 与 SBOM 的精确路径+SHA-256 集合；
  - `sources.json` schema/engine/version/artifacts；
  - 原子安装后侧车仍存在且可再次验证。
- Windows Starter builder 会生成 `sbom.spdx.json` 和明确标记 `review_status=incomplete` 的 `sources.json`，再将二者哈希写入 Manifest。
- CI 和仓库合同已加入 Engine SBOM 回归。
- `scripts/test_windows_explorer_integration.ps1` 已增强：真实安装后检查两个 Starter 包、四个侧车哈希、`review_status=incomplete`，并用 CLI 逐包复验。

### 3.2 已通过的验证

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 全 workspace 普通 Rust 测试：155 Core + 9 Schema + 22 Desktop + 4 Engine SDK，全部通过。
- 前端 8 项测试、TypeScript check、Vite production build 通过。
- `python scripts/test_engine_sbom.py` 通过。
- `python scripts/check_repository.py` 通过。
- 正式 Starter 已重建为 311 个文件、243,777,389 bytes；PDF/Media Manifest 均通过真实 CLI verifier。
- 增强后的真实 NSIS 安装烟测通过：
  - 证据：`.artifacts/windows-explorer-installed-smoke/suite-4f3ed785783e4f2492f528921652d2dc`
  - 安装器：279,412,699 bytes
  - SHA-256：`25679dd0b4b8b0dd2918ed2fee4e34d79df629b231b98ecd02cc13309b630859`
  - 两个 pack、四个侧车、Shell verb、单实例、零自动 Job、卸载与应用状态恢复全部通过。

### 3.3 确定性 Starter 哈希

| Pack | Manifest SHA-256 | SBOM SHA-256 | sources.json SHA-256 |
|---|---|---|---|
| PDF | `f5856f84c902dc45b038109f529ae8721b33da58ff6c9e9cdd93b0928ca9a498` | `8a8ae7125871893dbd86b18a0b702ea282cbe129d793669520de88cd3b812f63` | `7f5ccbe63e841bb4025d95c664bfd4876e1eb0b7b340dfbbb0735e3759ea60db` |
| Media | `1af7d4e7d2f8a229d7326ba821b9a2ba439102bc76f60866b639b7a824889810` | `715e474e6efe89bdd0984de059a62124269aa179e158e13bf562e09e30c977cc` | `0b343967f57c25c8152a44d72d46fe69263b1ac3d0aa0942df1a1fb77e4b0979` |

`bundle.json` SHA-256：`21f46f92f63ae9fc31a059b3139b4edcf27d2fa9b7b6522fc34f13cb43c48823`。

## 4. 当前工作树（不要丢弃）

已修改：

- `.github/workflows/ci.yml`
- `apps/desktop/src/App.tsx`
- `crates/core/src/engine_pack.rs`
- `crates/core/tests/schema_contracts.rs`
- `crates/engine-sdk/src/lib.rs`
- `docs/MASTER_EXECUTION_PLAN.md`
- `docs/release/SBOM.md`
- `docs/security/ENGINE_SUPPLY_CHAIN.md`
- `docs/testing/WINDOWS_STARTER.md`
- `schemas/engine-manifest/v1.schema.json`
- `scripts/build_windows_starter_pack.ps1`
- `scripts/check_repository.py`
- `scripts/test_windows_explorer_integration.ps1`

未跟踪新文件：

- `apps/desktop/src-tauri/tauri.release-e2e.conf.json`
- `scripts/cdp_desktop_conversion_e2e.mjs`
- `scripts/generate_engine_sbom.py`
- `scripts/test_desktop_release_conversion.ps1`
- `scripts/test_engine_sbom.py`
- 本交接文档

不得使用 `git reset --hard`、`git checkout -- .` 或删除未跟踪文件。先用 `git diff` 逐项审查并保留本批次。

## 5. 当前尚未收口的两个点

### 5.1 Release UI E2E 的 JPG 第二轮自动化

新增的 CDP Release UI 测试已经证明 PNG 从真实界面经过“检查并预览计划 → 开始转换 → ValidationReport”成功：

- 证据目录：`.artifacts/desktop-release-conversion/suite-d63e2a52cb6a41cba0961d31b29c500b`
- 输出：3 个确定性命名 PNG 页面。
- 截图：`png-report.png`。
- 报告状态：Pass。

同一进程返回 Convert 页面后执行 JPG 时，自动化没有等到第二个 Plan card；没有错误横幅，也没有 JPG 输出。现有 CLI/旧本机 E2E 已证明 JPG 引擎和 validator 可用，因此当前证据更像 CDP 脚本的 React 状态切换问题，**不能直接登记为产品 JPG 故障，也不能宣称新的 Release UI JPG 已通过**。

建议最稳妥的修法：让 `test_desktop_release_conversion.ps1` 为 PNG 和 JPG 分别启动一套隔离的 Release 进程/状态目录，或在设置 `<select>` 后等待一次 React 重渲染并核对 Plan target。不要继续依赖同一页面内的原生 setter 瞬时值。

### 5.2 最终安装器已落后于最后一处前端修正

`apps/desktop/src/App.tsx` 最后补了显式 `<option value={value}>`，避免不可用选项的 value 混入本地化“— 缺失”文案；Preset 的 color-mode/stream 变更也会使旧 preview 失效。

上面 SHA-256 为 `25679dd0…0859` 的安装器是在这处前端修正之前构建的。当前 `target/release/formatwright-desktop.exe` 又是带 `tauri.release-e2e.conf.json` 远程调试参数的测试构建。因此交接后必须：

1. 先修好并跑通 PNG/JPG Release UI E2E；
2. 再用标准配置重建 NSIS（不能带 release-e2e 远程调试配置）；
3. 重新计算安装器 hash；
4. 重新跑增强后的安装/首次启动/卸载烟测；
5. 更新文档里的旧安装器大小、hash、测试数和证据目录。

## 6. 下一位 Agent 的执行计划

### 批次 A：收口当前 SBOM + Release UI 改动（最高优先级）

1. 阅读 `SPEC_PLAN.md`、`docs/MASTER_EXECUTION_PLAN.md`、本交接文档和 `docs/security/ENGINE_SUPPLY_CHAIN.md`。
2. `git status --short`、`git diff --check`、逐文件审查当前工作树；不得覆盖用户/前 agent 改动。
3. 修复 Release UI E2E 的 JPG 自动化，最好每个格式使用独立应用进程和隔离状态。
4. 为 `<option value>` 行为增加一个前端测试，断言 unavailable 文案不改变提交 value，并断言 preset 参数变化会使旧 preview 失效。
5. 将新 E2E 配置/脚本加入 `scripts/check_repository.py` 的必需文件，评估是否加入 Windows CI；若 CI 成本过高，至少加入手动 RC workflow 和文档命令。
6. 重跑：Rustfmt、Clippy、全 workspace tests、pnpm check/test/build、两个 Python 回归、repository contract、`git diff --check`。
7. 用标准配置重建正式 Starter 与 NSIS；确认标准安装器没有开启 remote debugging。
8. 跑增强安装烟测和 Release UI PNG/JPG E2E，核对输出/报告/状态恢复。
9. 更新 `WINDOWS_STARTER.md`、`WINDOWS_PACKAGING.md`、`MASTER_EXECUTION_PLAN.md`、`TRACEABILITY.md`、`DEFECT_REGISTER.md` 的准确证据和边界。
10. 将当前批次提交为一个可回滚 commit；提交前确认工作树无遗漏。

批次 A 验收条件：

- 两个 Engine SBOM byte-identical rebuild；Core 精确 inventory 校验通过。
- 标准 NSIS 安装后有两个 pack 和四个有效侧车。
- 安装/卸载无用户状态污染。
- Release UI 的 PDF→PNG 与 PDF→JPG 均为 Pass 且页数/命名正确。
- 安装器 hash、大小、命令和证据路径已写入文档。
- 工作树干净并有单独 commit。

### 批次 B：可信引擎签名、吊销和回滚（Public Beta 阻断）

1. 冻结 canonical Manifest bytes 和签名 envelope；禁止签名中包含安装绝对路径。
2. 设计 release keyring：`key_id`、用途、有效期、算法、轮换和吊销清单。
3. 实现可信签名验证；仅 hash 完整或“signature_present”不能提升为 Certified。
4. 区分 Unverified / Trusted / Revoked / Expired / Wrong target / Downgrade blocked。
5. 实现多版本 active registry、原子 activate、启动失败自动回滚和手动回滚。
6. 覆盖断电/half-install、并发安装、篡改、旧 key、revoked key、降级、已运行 Job pin 旧 pack。
7. 将 certification 状态贯通 Doctor、Planner、UI、报告和发布证据。

### 批次 C：完整供应链与法律审查（Public Beta 阻断，需人工决策）

1. 将 file-level SBOM 扩展为完整 transitive component 归属，尤其是静态 FFmpeg build。
2. 为每个组件记录版本、源码 URL/revision、build flags、许可证、notice、对应源码提供方式。
3. 完成 Poppler/FFmpeg 及 codec 的许可证、源码义务和区域专利审查。
4. 将 `sources.json.review_status` 从 `incomplete` 提升必须基于签字记录，不能由代码自动宣称。
5. 无法合法再分发的组件移出 Starter，改为可选 pack 或用户自备。

### 批次 D：Windows 干净离线 VM 认证（Public Beta 阻断）

1. 新建无 FFmpeg/Poppler/LibreOffice/Pandoc/libvips、无开发缓存的 Windows 11 VM。
2. 断网安装最终 unsigned/signed RC；确认安装不访问网络。
3. 从安装后的真实 UI 完成 JSON→YAML、PDF→PNG、PDF→JPG、视频或音频转换。
4. 覆盖 Unicode/空格/长路径、Explorer Shell verb、取消、重启恢复、升级、卸载和零残留。
5. 保存 VM 快照、屏幕证据、报告、输出 hash、进程/网络观测和安装器 hash。
6. 只有该 Gate 与供应链 Gate 都通过后，R-008/R-009 才能从 Fixed 关闭为 Closed。

### 批次 E：长期自用稳定性

1. 真实物理 10 GiB 顺序读写、低内存、磁盘满、权限丢失、目标卷拔出。
2. 验证后/提交前外部创建目标的竞态；目录多文件提交恢复。
3. 长时 soak、掉电、进程强退、SQLite WAL/备份/恢复/迁移矩阵。
4. 拆分过大的 `runner.rs`，保持公共 Plan/Report 合同不变。
5. 扩展 PDF/Office/高分辨率黄金语料和视觉差异阈值；Unknown 不算 Pass。
6. 建立兼容矩阵、弃用周期、数据迁移政策、备份恢复演练和月度依赖更新节奏。

### 批次 F：跨平台与后续产品阶段

1. macOS arm64/x64 和 Ubuntu LTS 的真实引擎包、进程树、文件系统、Unicode 和离线认证。
2. Finder/Linux 文件管理器集成、macOS 签名公证、Linux 包分发。
3. OS 强制网络/文件/进程隔离，不只做零 socket 观测。
4. 完成 Private Beta 用户研究后再进入 Public Beta。
5. API/MCP、自托管 Worker 和浏览器 WASM 属于 Gate 6/7，不能抢在本地桌面发布阻断之前。

## 7. 建议给下一位 Agent 的启动提示词

~~~text
你接手的是 E:\Users\Administrator\Desktop\FormatWright 仓库。先完整阅读：
1) SPEC_PLAN.md
2) docs/MASTER_EXECUTION_PLAN.md
3) docs/AGENT_HANDOFF_2026-08-15.md
4) docs/security/ENGINE_SUPPLY_CHAIN.md

当前 main HEAD 是 ed9afd1，工作树有一组必须保留、尚未提交的 Engine SBOM / Release UI E2E 改动。禁止 reset、checkout 丢弃或删除未跟踪文件。

先执行交接文档“批次 A”，不要直接开始新功能：审查现有差异，修复 JPG Release UI 自动化，增加前端回归，跑完整门禁，用标准配置重建 Starter 和 NSIS，完成安装后 PNG/JPG UI 转换，更新准确证据并提交为独立 commit。

任何 file-level SBOM 都不得宣称完整法律认证；sources.json 当前必须保持 review_status=incomplete。当前产品是 Windows 自包含开发候选，不是 Public Beta。每完成一个 Gate，都要同步 MASTER_EXECUTION_PLAN、TRACEABILITY、DEFECT_REGISTER 和对应 testing/release 文档。
~~~

## 8. 不应误报为已完成的事项

- 完整 transitive engine SBOM / 法律与专利认证。
- Trusted release keyring、签名验证、密钥轮换和吊销。
- 多版本自动回滚、升级/降级故障矩阵。
- 离线干净 Windows VM 的安装后真实 UI 认证。
- Authenticode 正式签名、macOS 公证和 Linux 正式包。
- macOS/Linux 完整黄金工作流认证。
- OS 强制网络/文件隔离。
- Public Beta、Certified、可用于唯一副本等发布承诺。
