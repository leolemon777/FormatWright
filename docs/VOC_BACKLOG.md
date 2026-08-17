# FormatWright 开发顺序清单（VOC）

- 状态：负责人排期表，按「大家想要什么」排序
- 更新：2026-08-17
- 产品范围：`SPEC_PLAN.md`（不拼格式数量）
- 本增量完整规格：[`docs/specs/WINDOWS_DAILY_USE_SPEC_PLAN.md`](specs/WINDOWS_DAILY_USE_SPEC_PLAN.md)
- 发布门禁：`docs/MASTER_EXECUTION_PLAN.md`
- 已拍板：拖放简单 + 右键一键转都做；右键 Convert to X = 批准；Open-in 只打开

规则：一次只做一个可验收项。做完再勾选。发布阻断未关闭前，不开始 API / MCP / 新格式大类。

---

## 已经有了（不要回头堆）

- [x] 本地 Plan → 校验 → 不覆盖；CLI 与桌面同一套 Core
- [x] Windows Starter：PDF + Media；Release 不走系统 PATH
- [x] 持久队列、恢复、批量、报告、预设导入导出
- [x] 转换页只显示当前文件能走的目标（HowToConvert 简化）
- [x] 经典资源管理器：打开 + 按扩展名 Convert to …（FileConverter 雏形）
- [x] 本机开发注册脚本：`scripts/register_dev_explorer_convert.ps1`

---

## 第 0 波：把手头改动收成可回滚版本（0.5 天）

工程债，不做后面全是脏树。

| # | 项 | 验收 | 依赖 |
|---|---|---|---|
| 0.1 | 提交本轮未入库改动（认证贯通、负向矩阵、转换页简化、shell-convert） | 工作树干净，一条或少数 Conventional Commit | 无 |
| 0.2 | 用标准配置打一份带新右键的 NSIS（无 DevTools） | 安装后 PDF 右键有 Convert to PNG；卸载清键 | 0.1 |

---

## 第 1 波：第一次就能转成功（约 1 周）

VOC 最痛：两下做完、别迷路、别像坏了。

| # | 项 | 验收 | VOC |
|---|---|---|---|
| 1.1 | 成功后主按钮「打开输出位置」；多页 PDF 写清张数和目录 | ST508S PDF→PNG 转完一键打开文件夹 | 结果找不到 |
| 1.2 | 空状态三张卡：PDF 转图 / JSON 转 YAML / 视频转 MP4（无包则灰并说明） | 新用户不拖文件也知道能干什么 | 第一次不会用 |
| 1.3 | 拖文件夹自动切「文件夹批量」 | 拖目录不再停在单文件页 | 批量入口弱 |
| 1.4 | 右键多选同一类型文件：按同一目标入队并跑窗口 | 资源管理器多选 10 张 jpg → WebP，有批次 | 两下转完 |
| 1.5 | 转完托盘/系统通知：成功或失败可点回应用 | 不必死盯窗口 | FileConverter 手感 |
| 1.6 | Plan 大白话：只换容器 / 会压糊 / 会丢轨道；错误全中文 | `.xls`、缺引擎、不支持三条文案用户能看懂 | 别当骗子软件 |

**第 1 波退出：** 未安装开发环境的人，用新安装包完成：拖 PDF 转图、右键 PDF 转 PNG、拖文件夹批量图。

---

## 第 2 波：Windows 右键像装上了（约 1 周）

VOC 里 FileConverter 被骂最多的是「菜单没了」，不是格式少。

| # | 项 | 验收 | VOC |
|---|---|---|---|
| 2.1 | Windows 11 现代顶层菜单出现 FormatWright / Convert | 不必「显示更多选项」 | Win11 右键 |
| 2.2 | 文件夹右键：按推荐目标整夹转换（仍不覆盖、有磁盘预检） | 右键相册夹 → Convert to JPG | 整夹 |
| 2.3 | 右键预设可配置：默认「小 JPG」「PDF 每页 PNG」；导入导出沿用现有 PresetLibrary | 改一次，右键跟着变 | 预设 |
| 2.4 | 安装/升级/卸载后菜单仍在；假网站/便携 exe 说明用官方包 | 干净机装一次，冷热启动菜单都在 | 菜单突然消失 |

**第 2 波退出：** Win11 新菜单可转；升级不丢菜单；预设能搬走。

---

## 第 3 波：中文用户搜得最多的格式（约 1–2 周）

| # | 项 | 验收 | VOC |
|---|---|---|---|
| 3.1 | 官方 Image/HEIC 包（或合法可分发方案）；去掉「开发机 heif-convert」当认证依赖 | 干净机 HEIC→JPG/PNG | iPhone 照片 |
| 3.2 | 批量 HEIC，保留拍摄时间；地点/ICC 按 Plan 明示保留或丢弃 | 100 张 HEIC，输出日期不是「今天」 | 元数据 |
| 3.3 | 可选 Document 包：xlsx/docx/pptx→PDF（LibreOffice 隔离配置） | 新版 Office 能转；`.xls` 明确叫用户另存 xlsx | Office |
| 3.4 | 加密 PDF：专用密码框，密码不进 Plan/日志/SQLite | 加密说明书可转，报告无口令 | 规格已有、未做 |

**第 3 波退出：** 宣传可以写 HEIC 批量、Office→PDF，且每条有验证报告。未过的仍标 Experimental。

---

## 第 4 波：发布认证（与第 2–3 波可交错，但不能跳）

没有这些，不能叫 Beta。

| # | 项 | 谁 | 验收 |
|---|---|---|---|
| 4.1 | 冻结 `PRODUCT_DECISIONS.md`（名、域、最低 OS、签名预算） | 负责人 | 12 项都有 Decision + 日期 |
| 4.2 | 干净离线 VM：安装 → 无系统引擎 → PDF/JSON/一条媒体 | 工程 | R-008/R-009 Closed |
| 4.3 | 正式密钥仪式 + 签名 Starter | 负责人 + 工程 | 出厂包 Trusted，审查未完成仍不 Certified |
| 4.4 | FFmpeg/Poppler 法律/专利签字；不能捆的降为可选包 | 法务/负责人 | `sources.json` 不再只是 incomplete 自动写 |
| 4.5 | Authenticode 签 NSIS；升级/回滚/卸载 | 工程 | 发布清单全绿 |

**第 4 波退出：** Windows 长期自用稳定版可给熟人用。仍不是三平台 Public Beta。

---

## 第 5 波：以后再说（先禁止开工）

- API / MCP / Docker 自托管
- 电子书、CAD、字幕、压缩包大类
- macOS Finder / Linux 文件管理器（Windows 菜单不稳之前不做）
- 用格式数量对标 HowToConvert

---

## 建议的日历（1 人 + AI）

| 周 | 只做 |
|---|---|
| 本周 | 0.1 → 0.2 → 1.1 → 1.2 → 1.3 |
| 下周 | 1.4 → 1.5 → 1.6 → 开始 2.1 |
| 第 3 周 | 2.2 → 2.3 → 2.4 |
| 第 4–5 周 | 3.1 → 3.2（HEIC） |
| 并行（等你拍板） | 4.1 决策；有 VM 就插 4.2 |

下一件该动手的：**0.1 提交，然后 0.2 打安装包**。未点头之前不自动 commit、不自动跑安装器覆盖你现有环境。
