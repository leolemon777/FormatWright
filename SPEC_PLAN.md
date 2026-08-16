# FormatWright — Master Specification & Delivery Plan

> 把文件转换正确：本地优先、可验证、可恢复、可自动化。

## 文档信息

| 字段 | 内容 |
|---|---|
| 项目名称 | FormatWright |
| 名称状态 | 正式工作名称；公开发布前完成商标、域名、GitHub 组织名与包名复核 |
| 文档类型 | 产品规格 + 技术规格 + 交付总计划 |
| 文档版本 | 0.3-draft |
| 更新日期 | 2026-08-12 |
| 当前阶段 | Windows 开发 Alpha；现有安装包无引擎、不可开箱转换。先关闭 R-008/R-009 建立 Starter 纵向闭环，再整改 R-001–R-007 |
| 决策状态 | 有条件 Go |
| 权威性 | 产品范围与发布门槛以本文为准；完成/待办勾选与近期进度以 [`docs/MASTER_EXECUTION_PLAN.md`](docs/MASTER_EXECUTION_PLAN.md) 为执行事实来源 |

## 0. 执行摘要

FormatWright 是一个开源、本地优先的通用文件转换平台。它不以“支持多少种扩展名”作为核心卖点，而以以下能力形成差异：

1. 超大文件稳定转换，内存不会随输入文件大小线性增长。
2. 文件夹级批处理、暂停、恢复、跳过已完成项和崩溃恢复。
3. 转换前解释将采用的路径，并优先选择重封装、无损或最少损失路径。
4. 转换后验证输出，明确报告内容、元数据、轨道、字体或结构是否发生变化。
5. GUI、CLI、REST API 和 MCP 共用一个 Rust 核心。
6. 默认完全离线、无遥测，并能用测试证明转换期间没有网络流量。
7. 通过可审计的引擎适配层扩展格式，而不是把不可信命令直接拼进 Shell。

### 0.1 战略判断

- 做“开源版 HowToConvert 克隆”：No-Go。
- 做“本地优先的文件转换基础设施与质量平台”：Go。
- 首发入口：桌面端 + CLI。
- 后续入口：单机 REST API、自托管服务、MCP、浏览器轻量转换。
- 首发护城河：转换路径规划、质量验证、超大文件稳定性、可恢复批处理和测试语料库。

### 0.2 一句话定位

**FormatWright 是一个会解释转换路径、验证转换结果，并能可靠处理超大文件和批量任务的开源本地文件转换平台。**

### 0.3 v0.1 成功定义

只有同时满足以下条件，v0.1 才允许标记为 Public Beta：

- 12 条黄金工作流全部通过三平台测试。
- 单个 10GB 媒体文件转换不导致主程序 OOM。
- 10,000 个图片文件的批处理可暂停、恢复并保留目录结构。
- 强制退出后重启，未完成任务能够恢复或明确标记为可重试。
- 转换路径可预览，能重封装时不进行无意义重编码。
- 每个成功任务生成机器可读和人类可读的验证报告。
- 转换过程默认零外联网络请求。
- 不静默覆盖任何已有文件。
- Windows、macOS、Linux CI 通过。
- GUI 与 CLI 使用同一核心，不复制业务规则。

## 1. 研究结论与市场证据

本节汇总截至 2026-08-10 已完成的产品、论坛、市场、竞品和开源项目研究。星标和价格是研究快照，会随时间变化。

### 1.1 竞品格局

| 产品 | 当前优势 | 已发现的结构性缺口 | 对 FormatWright 的启示 |
|---|---|---|---|
| HowToConvert | 本地处理、浏览器演示、桌面端、约 5,438 条转换路由、一次性约 29 美元 | 闭源；核心是多个现成引擎的产品化封装；缺少开放 API、可审计插件体系和深入质量报告 | 不能只拼格式数量；必须在可靠性、解释性和自动化上明显领先 |
| VERT | AGPL；浏览器 WASM；约 250+ 格式；约 15.3k GitHub Stars | 大视频可能依赖自托管 daemon；曾出现约 800MB 视频转换后下载失败 | 浏览器适合轻任务，超大文件应优先由本地原生执行 |
| ConvertX | AGPL；Docker 自托管；约 1,000+ 格式；约 18.4k Stars | 大文件上传可能进入内存并崩溃；官方 API 需求被关闭为不计划实现 | 流式文件路径和正式 API 是明确差异点 |
| FileConverter | Windows Explorer 右键菜单体验成熟；约 14.9k Stars | Windows 为主；Windows 11 新右键菜单集成受限；预设导入导出仍有需求 | 系统集成很有价值，但需要跨平台且配置可迁移 |
| Stirling-PDF | PDF 领域强势；约 88k Stars；部署成熟 | Office 转 PDF、PDF 转 Word 等场景有字体、版式和 RTL 保真问题 | “输出了文件”不等于“转换正确”，验证报告必须是一等能力 |
| Transmute | API-first、自托管、界面现代 | 项目较新，成熟度、引擎覆盖和长期兼容性尚未充分验证 | API-first 需求真实，但不能跳过稳定核心和测试语料 |
| ConvX | 提出 Desktop + CLI + MCP 的方向 | 当前更接近早期 POC，提交和采用度极低 | 方向正确不代表执行完成；可用性和可靠性是机会 |

关键项目：

- [HowToConvert](https://howtoconvert.co/)
- [VERT](https://github.com/VERT-sh/VERT)
- [ConvertX](https://github.com/C4illin/ConvertX)
- [FileConverter](https://github.com/Tichau/FileConverter)
- [Stirling-PDF](https://github.com/Stirling-Tools/Stirling-PDF)
- [ConvX](https://github.com/JSBtechnologies/convx)

### 1.2 用户需求证据

| 需求主题 | 已观察到的真实问题 | 产品要求 |
|---|---|---|
| 隐私与信任 | 用户不知道在线转换站是否上传、保存或分析文件；企业用户会绕过内部工具；FBI 曾警告恶意在线转换器传播恶意软件 | 默认本地；零网络证明；开源；引擎来源和哈希可见 |
| 一次性便捷与高频本地使用 | 浏览器适合偶发小任务；大文件、高频任务和敏感资料更适合桌面端；部分企业设备不允许随意安装软件 | 桌面端优先，同时保留便携版和后续浏览器轻量模式 |
| 超大文件 | ConvertX 有 7.3GB 文件导致内存问题的案例；VERT 有大视频下载失败案例 | 引擎直接读写文件；主程序不缓存整个输入或输出；断点和临时文件策略明确 |
| 大批量与可恢复 | 用户需要处理数千 WebP、保留目录结构和元数据；超大队列会带来高内存和长时间不可恢复问题 | 持久化队列、分页、并发限制、暂停恢复、跳过完成项、失败重试 |
| API、CLI 与自动化 | 自托管社区持续询问 CloudConvert 替代品及 REST API；ConvertX 的 API 请求未进入计划 | CLI 必须首发；REST API 使用同一核心；后续加入 webhook 和 MCP |
| 质量与保真 | Office 到 PDF 可能出现字体替换、页数变化；PDF 到 Word 可能破坏 RTL；媒体常被无意义重编码 | 转换前规划、重封装优先、多引擎回退、输出验证、清楚标记损失 |
| 操作系统集成 | Windows 用户喜欢右键即转；Windows 11 菜单变化带来兼容问题；用户需要预设迁移 | 右键菜单、Finder Quick Action、Linux 文件管理器动作、预设导入导出 |
| 企业与离线环境 | 公司用户要求视频等内容绝不外发，且需要完整 local-only 配置 | 全局网络 Kill Switch、引擎白名单、离线引擎包、审计日志和策略文件 |
| 冷门格式和扩展性 | 用户需要 3D、CAD、字体、字幕、科研和档案格式 | 插件/格式包体系；核心不承诺一次覆盖所有格式 |

关键证据：

- [在线转换器隐私讨论](https://www.reddit.com/r/privacy/comments/1r73kbd/psa_most_online_file_converters_upload_your_files/)
- [浏览器与本地转换的使用场景讨论](https://news.ycombinator.com/item?id=43663865)
- [ConvertX 7.3GB / 900MB 内存问题](https://github.com/C4illin/ConvertX/issues/364)
- [ConvertX 超过 100MB 上传问题](https://github.com/C4illin/ConvertX/issues/447)
- [VERT 大视频下载失败](https://github.com/VERT-sh/VERT/issues/216)
- [数千 WebP 批量转换需求](https://www.reddit.com/r/DataHoarder/comments/ha1jrh/converting_1000s_of_webp_images_to_jpg/)
- [HandBrake 大队列性能问题](https://github.com/HandBrake/HandBrake/issues/3117)
- [自托管转换 API 需求](https://www.reddit.com/r/selfhosted/comments/1so6oi5/self_hosted_file_conversion/)
- [ConvertX API 功能请求](https://github.com/C4illin/ConvertX/issues/247)
- [开源 Web UI + REST 需求](https://www.reddit.com/r/selfhosted/comments/1pdy9nq/anybody_interested_in_a_open_source_self_hosted/)
- [Stirling-PDF Office 保真问题](https://github.com/Stirling-Tools/Stirling-PDF/issues/4137)
- [Stirling-PDF RTL 转换问题](https://github.com/Stirling-Tools/Stirling-PDF/issues/420)
- [Windows 右键转换需求](https://www.reddit.com/r/software/comments/178sila/file_convert_from_context_menu_right_click/)
- [Windows 11 现代右键菜单限制](https://github.com/Tichau/FileConverter/discussions/663)
- [FileConverter 预设导入导出需求](https://github.com/Tichau/FileConverter/discussions/785)
- [VERT 完全本地配置需求](https://github.com/VERT-sh/VERT/issues/140)

### 1.3 市场与商业信号

- CloudConvert 免费额度约为每日 10 credits，并通过付费 credits 与企业能力变现。
- Convertio 免费层约 100MB、每日 10 次，订阅价格约 11.99 至 44.99 美元/月，API 页面宣称累计处理数亿文件。
- FreeConvert 订阅约 12.99 至 29.99 美元/月，最大文件约 1.5GB 至 5GB。
- HowToConvert 采用约 29 美元一次性购买，说明“隐私 + 本地 + 简单”具有付费意愿。
- 云服务的限制集中在文件大小、每日次数、隐私与订阅；开源本地产品可绕开这些结构性限制。
- [FBI 2025 年在线文件转换器诈骗警告](https://www.fbi.gov/contact-us/field-offices/denver/news/fbi-denver-warns-of-online-file-converter-scam)强化了“本地、可审计、可信分发”的价值。

价格来源：

- [CloudConvert Pricing](https://cloudconvert.com/pricing)
- [Convertio Pricing](https://convertio.co/pricing/)
- [Convertio Free Tier](https://support.convertio.co/hc/en-us/articles/360004386774-Free-tier-limit-for-file-conversions)
- [FreeConvert Pricing](https://www.freeconvert.com/pricing)

### 1.4 最终机会判断

FormatWright 不应在首页宣传一个不断膨胀但质量未知的“转换组合数”。首发应围绕四个可被用户立即验证的承诺：

1. **大文件不会拖垮主程序。**
2. **批处理可以停、可以恢复。**
3. **转换前知道会发生什么。**
4. **转换后知道有没有丢东西。**

## 2. 产品定义

### 2.1 愿景

成为文件转换领域可信的开源基础设施：普通用户能够放心点击，专业用户能够审查路径，开发者能够自动化，企业能够离线部署。

### 2.2 核心承诺

FormatWright 不只告诉用户“转换完成”，还要回答：

- 输入文件实际上是什么，而不只看扩展名。
- 为什么选择这条转换路径。
- 是否发生了解码、重编码或有损转换。
- 哪些内容、轨道、字体、元数据或结构被保留。
- 哪些变化是用户要求的，哪些是不可避免的。
- 输出是否能够重新打开并通过验证。
- 使用了哪个引擎、版本、构建参数和许可证。

### 2.3 目标用户优先级

#### P0：创作者与重度桌面用户

- 视频、音频、图片高频转换。
- 文件很大或批量很多。
- 需要预设、队列、右键菜单和可恢复能力。

#### P0：开发者与自动化用户

- 需要 CLI、JSON 输出、稳定退出码和可重复计划。
- 需要在脚本、CI、本地 Agent 或后续 MCP 中调用。

#### P0：数据保管者与档案用户

- 重视元数据、色彩配置、字幕、章节、文件结构和可追溯性。
- 需要批量迁移、校验和报告。

#### P1：隐私敏感的普通用户

- 只想拖入文件并获得安全、清楚的结果。
- 不应被编码器、像素格式和复杂参数淹没。

#### P2：企业与隔离网络

- 需要引擎白名单、离线包、策略控制、审计、签名和支持。

### 2.4 产品原则

1. Local-first：默认本地，不要求账户。
2. Plan-first：执行前先生成可解释计划。
3. Preserve-first：能重封装就不重编码；能无损就不有损。
4. Verify-always：成功必须包含输出验证，不以进程退出码代替质量判断。
5. Crash-safe：任务状态持久化，临时产物可识别、可清理、可恢复。
6. One-core：GUI、CLI、API、MCP 共用领域模型和执行器。
7. Honest-loss：无法保留的内容必须明确告知。
8. Open-by-design：能力、引擎来源、许可证和报告均可审查。
9. Safe-by-default：不调用 Shell，不静默覆盖，不默认联网。
10. Progressive complexity：普通模式三步完成，专家模式暴露完整控制。

### 2.5 v0.1 明确不做

- 不追逐所有格式和数千条路由。
- 不做完整视频编辑器、图片编辑器或 Office 编辑器。
- 不做云账户、团队协作、支付和配额系统。
- 不用 AI 自动修改用户内容。
- 不同时首发 Desktop、Web、Server、MCP 四套入口。
- 不承诺任意两个格式之间都能高质量互转。
- 不在未经法律和许可证审查时捆绑 Ghostscript 或 nonfree FFmpeg。
- 不把文件内容、完整路径或使用数据发送到遥测服务。

## 3. 版本范围

### 3.1 v0.1 黄金工作流

| ID | 工作流 | 默认策略 | 必须验证的内容 |
|---|---|---|---|
| GW-01 | HEIC/HEIF → JPG/PNG | 自动方向；保留 ICC；元数据策略可选 | 尺寸、方向、ICC、EXIF、透明度变化 |
| GW-02 | PNG/JPG → WebP/AVIF | 按视觉质量或目标大小；透明度保护 | 尺寸、Alpha、ICC、输出可解码 |
| GW-03 | 图片文件夹批量缩放/压缩 | 递归；保留目录；命名模板；并发受控 | 文件数量、目录映射、尺寸、失败清单 |
| GW-04 | MOV/MKV/AVI/WebM → MP4 | 编解码兼容时优先 remux，否则转码 | 时长、分辨率、帧率、轨道、字幕、章节 |
| GW-05 | 视频 → MP3/M4A/WAV | 选择主音轨；可保留全部音轨 | 时长、声道、采样率、封面、标签 |
| GW-06 | 视频 → GIF | 时间段、裁剪、缩放、帧率、目标大小 | 帧数、尺寸、时长、输出大小 |
| GW-07 | 音频格式互转 | 默认保留标签、封面和声道布局 | 时长、声道、采样率、标签、封面 |
| GW-08 | DOCX/PPTX/XLSX → PDF | LibreOffice Headless；独立用户配置目录 | 页数、页面尺寸、字体警告、视觉漂移 |
| GW-09 | PDF → PNG/JPG | 每页独立；DPI 和颜色设置可控 | 页数、尺寸、渲染成功、透明度 |
| GW-10 | Markdown/HTML → PDF/DOCX | Pandoc；PDF 引擎可选；资源显式解析 | 标题结构、资源、页数、字体与链接 |
| GW-11 | CSV/JSON/YAML/XML 互转 | Rust 原生；显式映射嵌套和空值策略 | 记录数、字段、类型、空值、解析错误 |
| GW-12 | 文件检查与元数据清理 | Inspect 与 Clean 分离；清理前展示计划 | 格式识别、被删除字段、内容哈希不误变 |

### 3.2 v0.1 必须具备的产品能力

- 文件与文件夹拖放。
- 根据文件内容探测真实格式。
- 推荐目标格式与预设。
- 转换计划预览。
- 任务队列、并发控制、暂停、恢复、取消、重试。
- 保留文件夹结构。
- 输出命名模板和冲突处理。
- 临时文件与原子提交。
- 转换验证和质量报告。
- 历史记录和预设导入导出。
- 引擎检测、版本显示和健康检查。
- CLI 与 JSON 输出。
- Windows、macOS、Linux 桌面应用。
- Windows 右键菜单；macOS Finder Quick Action 与 Linux 文件管理器动作作为 Beta 目标。

### 3.3 v0.2 候选范围

- 监听文件夹。
- 字幕格式包。
- 压缩包格式包。
- 字体与电子书格式包。
- 多引擎选择与自动回退。
- PDF 页面编辑类操作。
- 可共享的转换配方。
- 更完整的系统分享菜单和托盘能力。
- 轻量本地 REST API。

### 3.4 v1.x 候选范围

- 自托管服务器与 Docker Worker。
- REST API、SSE 进度、Webhook 和 SDK。
- MCP Server。
- 分布式任务与 Postgres 存储适配器。
- 企业离线包、策略、SSO、审计和 SLA。
- 浏览器 WASM 轻量工作流。
- CAD、3D、科研与档案格式插件包。

## 4. 功能规格

### 4.1 检查与格式识别

**FW-FR-001：内容探测**

- 不仅依赖扩展名。
- 输出容器、编码、尺寸、时长、轨道、页面、元数据、色彩和文件哈希等可用信息。
- 扩展名与内容不一致时警告，不能自动篡改输入。

**FW-FR-002：机器可读结果**

- Inspect 结果支持稳定的 JSON Schema。
- Schema 带版本号。
- 未知字段允许向前兼容。

### 4.2 转换路径规划

**FW-FR-010：能力图**

- 每个引擎声明输入格式、输出格式、约束、损失等级、可保留属性和可验证属性。
- 同一格式对可存在多条边和多个引擎。

**FW-FR-011：硬约束**

- 用户可以指定目标格式、最大尺寸、分辨率、质量、编码器、元数据、轨道和网络策略。
- 不满足硬约束的路径不得被选择。

**FW-FR-012：路径排序**

排序优先级：

1. 满足所有硬约束。
2. 避免内容解码与重编码。
3. 降低不可逆损失。
4. 提高可验证程度和引擎可信度。
5. 减少步骤数、时间和临时磁盘需求。

**FW-FR-013：计划解释**

计划至少展示：

- 每一步引擎。
- 是否重封装、无损或有损。
- 预计变化。
- 预计保留项。
- 无法保证的内容。
- 所需临时空间。
- 缺失引擎和可安装来源。

### 4.3 执行

**FW-FR-020：流式与直接文件 I/O**

- 主程序不得为方便而把整个输入或输出读入内存。
- FFmpeg、libvips 等引擎直接读取输入路径并写入隔离的临时路径。
- 只有确有必要时才使用 OS Pipe。

**FW-FR-021：安全进程调用**

- 不经过 Shell。
- 可执行文件和参数数组分离。
- 环境变量显式允许。
- 工作目录为单任务隔离目录。

**FW-FR-022：临时输出**

- 输出先写为可识别的 partial 文件。
- 通过引擎退出、文件存在、格式重探测和验证后才原子移动到目标位置。
- 失败、取消和崩溃后保留策略可配置，默认安全清理。

**FW-FR-023：取消**

- Windows 使用 Job Object 或等价机制终止进程树。
- Unix 使用进程组和信号升级策略。
- 取消不得留下被误认为成功的目标文件。

### 4.4 批处理与恢复

**FW-FR-030：持久化队列**

- 任务在运行前写入 SQLite。
- 状态变化使用事务。
- UI 列表分页，不一次加载 10,000 个完整对象。

**FW-FR-031：状态机**

- queued
- inspecting
- planned
- blocked
- running
- validating
- completed
- warning
- failed
- cancelled
- interrupted

**FW-FR-032：恢复**

- 启动时扫描 running 和 validating 状态。
- 检查子进程是否仍存在、partial 文件和验证信息。
- 能安全继续则恢复；否则转 interrupted 并提供重试。

**FW-FR-033：幂等与跳过**

- 任务具有输入身份、计划摘要和目标身份。
- 输出存在且验证摘要匹配时可跳过。
- 不允许仅凭同名文件判断完成。

**FW-FR-034：冲突策略**

- ask
- skip
- rename
- overwrite
- overwrite 必须由用户或明确 CLI 参数授权。

### 4.5 验证与报告

**FW-FR-040：结果重探测**

- 所有成功输出必须重新打开并识别。
- 仅进程退出码为 0 不能视为成功。

**FW-FR-041：验证结果等级**

- Pass：满足要求且未发现非预期变化。
- Warning：输出可用，但存在损失、无法验证项或可接受差异。
- Fail：输出不可打开、关键内容缺失或违反硬约束。
- Unknown：缺少验证器，不得伪装为 Pass。

**FW-FR-042：质量报告**

报告包含：

- 输入与输出摘要及哈希。
- 转换计划和实际执行步骤。
- 引擎版本、构建信息和来源。
- 用户要求的变化。
- 检测到的非预期变化。
- 每项检查的状态、证据和建议。
- 可复现命令或配方。

### 4.6 引擎管理

**FW-FR-050：Doctor**

- 检查所有可用引擎、版本、路径、编解码能力、许可证元数据和健康状态。
- 区分系统引擎、FormatWright 认证引擎包和用户自定义引擎。
- 生成带 engine identity 的 capability snapshot，供 Doctor、Planner、UI 和执行后端共同使用。

**FW-FR-051：引擎包**

每个引擎包必须包含：

- 名称和版本。
- 平台与架构。
- 下载来源。
- SHA-256。
- 签名信息。
- 许可证和源代码地址。
- 构建参数。
- 是否可再分发。
- 专利或地区风险说明。

**FW-FR-052：安装行为**

- 网络下载必须由用户触发；安装介质可携带同版本、已审查的离线 Starter pack。
- 支持离线导入引擎包。
- 下载失败不能影响已有转换能力。
- 引擎安装、激活、升级和回滚必须原子化；半安装或校验失败不得替换当前可用版本。

**FW-FR-053：生产定位与能力门控**

- Release 只执行 Engine Registry 中已激活、hash/manifest 已验证的精确路径；不得依赖 ambient `PATH`、开发缓存、`.cmd` 或 `.bat` 包装器。
- 环境变量和 PATH 发现仅用于显式开发模式或“导入候选”，候选完成身份登记前不进入生产 capability snapshot。
- UI 推荐格式、目标选择器和 Planner 只能展示当前 snapshot 可执行的路线；执行后端必须再次硬校验，防止绕过 UI。
- 缺少、损坏、撤销或不兼容引擎时，返回稳定错误码与明确修复入口，而不是展示一个看似可执行的转换按钮。

### 4.7 CLI 合约

首发命令：

~~~text
formatwright inspect INPUT [--json]
formatwright plan INPUT --to FORMAT [OPTIONS] [--json]
formatwright convert INPUT --to FORMAT [OPTIONS]
formatwright batch INPUT_DIR --to FORMAT [OPTIONS]
formatwright jobs list|show|retry|cancel|resume
formatwright doctor [--json]
formatwright engines list|inspect|install
~~~

稳定退出码：

| 退出码 | 含义 |
|---:|---|
| 0 | 成功或全部任务成功 |
| 1 | 未分类内部错误 |
| 2 | 参数或用户输入错误 |
| 3 | 不支持的格式或路径 |
| 4 | 引擎缺失或不可用 |
| 5 | 转换进程失败 |
| 6 | 输出验证失败 |
| 7 | 部分批处理任务失败 |
| 8 | 策略或安全限制阻止执行 |
| 130 | 用户取消 |

CLI 要求：

- stdout 可作为结构化输出，日志进入 stderr。
- JSON 模式不混入彩色文本。
- 相同输入、引擎能力和约束生成相同计划。
- 支持 dry-run。
- 专家参数必须通过受控模型映射，默认不开放任意 Shell 参数。

## 5. 系统架构

### 5.1 总体架构

~~~mermaid
flowchart LR
    Desktop["Tauri Desktop"] --> Core["Rust Core"]
    CLI["CLI"] --> Core
    API["Axum REST API（后续）"] --> Core
    MCP["MCP Adapter（后续）"] --> CLI
    Core --> Inspect["Inspector"]
    Core --> Planner["Conversion Planner"]
    Core --> Queue["Job Queue"]
    Core --> Validate["Validators"]
    Planner --> Registry["Capability Registry"]
    Queue --> Runner["Engine Runner"]
    Runner --> FFmpeg["FFmpeg / ffprobe"]
    Runner --> Vips["libvips"]
    Runner --> Office["LibreOffice"]
    Runner --> Pandoc["Pandoc"]
    Runner --> PDF["PDF engines"]
    Core --> SQLite["SQLite"]
    Registry --> Manifests["Signed Engine Manifests"]
~~~

### 5.2 控制面与数据面

- Rust Core 是控制面：探测、规划、排队、进度、取消、验证、记录。
- 转换引擎是数据面：直接处理文件数据。
- 不让 React、API Handler 或 SQLite 承担大文件字节转发。
- 不为统一接口而复制几十 GB 数据。

### 5.3 核心领域模型

| 模型 | 作用 |
|---|---|
| Artifact | 输入、输出或中间文件及其身份信息 |
| Format | 规范化格式、容器和变体 |
| Probe | 对 Artifact 的可验证观察 |
| Capability | 引擎能够完成的转换边及限制 |
| Constraint | 用户、策略或平台提出的硬/软约束 |
| Plan | 确定的转换步骤、损失和资源估计 |
| Job | 可持久化、可恢复的执行实例 |
| Event | 状态、进度、日志和警告 |
| ValidationReport | 预期与实际输出的比较 |
| EngineManifest | 引擎身份、能力、许可证、来源和哈希 |

### 5.4 路径规划器

转换能力表示为有向多重图：

- 节点是规范化格式或媒体状态。
- 边是某引擎提供的转换能力。
- 同一输入输出可以有重封装、无损转码、有损转码和多步转换等多条边。
- 边声明损失类型、属性保留能力、验证器、资源需求和平台可用性。

规划采用确定性约束搜索。第一版可使用带词典序成本的 Dijkstra：

1. loss_rank
2. validation_confidence
3. reencode_count
4. engine_trust_rank
5. step_count
6. estimated_time
7. temporary_disk

任何硬约束失败的边直接排除，不通过调低权重蒙混过关。

### 5.5 引擎适配与插件协议

第一方适配器：

- 编译进 Rust Core 或独立 Rust crate。
- 负责将领域选项映射为安全参数。
- 解析结构化进度、警告和探测结果。

第三方插件：

- 使用 JSON/NDJSON over stdio。
- 插件进程与主程序生命周期隔离。
- 协议版本化，支持 capability、inspect、plan、execute、cancel 和 validate。
- v0.1 不加载任意原生动态库，避免 ABI、崩溃和供应链风险。

### 5.6 任务状态机

~~~mermaid
stateDiagram-v2
    [*] --> queued
    queued --> inspecting
    inspecting --> planned
    inspecting --> blocked
    planned --> running
    blocked --> queued
    running --> validating
    running --> cancelled
    running --> failed
    running --> interrupted
    validating --> completed
    validating --> warning
    validating --> failed
    interrupted --> queued
    failed --> queued
    completed --> [*]
    warning --> [*]
    cancelled --> [*]
~~~

### 5.7 建议仓库结构

~~~text
FormatWright/
  apps/
    desktop/              Tauri 2 + React
  crates/
    core/                 领域模型、规划、执行、验证、队列
    engine-sdk/           引擎能力和插件协议
    cli/                  formatwright CLI
    server/               Axum，后续启用
  packages/
    ui/                   可复用 React 组件
  engines/
    manifests/            引擎能力与许可证元数据
    adapters/             第一方适配器
  test-corpus/
    manifests/            测试文件清单、来源、许可证、预期结果
  docs/
    adr/                  架构决策记录
    specs/                拆分后的详细规格
    security/             威胁模型与供应链说明
  scripts/
  .github/
  Cargo.toml
  pnpm-workspace.yaml
  LICENSE
  SECURITY.md
  CONTRIBUTING.md
  SPEC_PLAN.md
~~~

## 6. 技术栈决策

### 6.1 应用栈

| 层 | 选型 | 决策原因 |
|---|---|---|
| 核心语言 | Rust stable，Edition 2024 | 内存安全、跨平台、低运行时开销、适合长期运行的本地核心 |
| 异步控制面 | Tokio | 子进程 I/O、并发任务、取消、事件流；不用于替代引擎的 CPU 编码 |
| 序列化 | Serde | JSON、配置和协议的稳定基础 |
| CLI | Clap | 类型化命令和帮助生成 |
| 日志 | tracing | 结构化上下文、任务关联和可选 JSON 日志 |
| 本地存储 | SQLite WAL + rusqlite | 单文件、事务、无需服务、适合断电恢复 |
| 桌面壳 | Tauri 2 | Rust 后端、跨平台原生 WebView、安装体积较小 |
| 前端 | React + TypeScript + Vite | 成熟组件生态、多人协作和后续 Web 复用 |
| UI | Tailwind CSS + Radix primitives | 快速构建一致、可访问的桌面界面 |
| UI 状态 | Zustand | 仅管理界面状态；任务真相保留在 Rust/SQLite |
| 本地 API | Axum + SSE | 后续自托管与进度流；暂不为 v0.1 阻塞 |
| 包管理 | Cargo workspace + pnpm workspace | Rust 与前端 Monorepo |
| 测试 | cargo-nextest、proptest、insta、Vitest、Playwright | 单元、属性、快照、UI 和端到端 |
| CI/CD | GitHub Actions | Windows、macOS、Linux 构建测试与发布 |

依赖版本策略：

- 使用当前稳定大版本。
- Cargo.lock 与 pnpm lockfile 必须提交。
- 引擎版本通过认证包固定，不直接依赖用户 PATH 中的未知版本。
- 安全更新与功能升级分开评估。

### 6.2 转换引擎

| 类别 | 主引擎 | 辅助/回退 | 首发说明 |
|---|---|---|---|
| 视频/音频 | FFmpeg + ffprobe | 平台硬件编码器 | 主力；优先重封装；解析构建配置 |
| 图片 | libvips | ImageMagick | libvips 负责低内存批处理；冷门格式回退 |
| Office | LibreOffice Headless | 后续可插拔商业引擎 | 每任务独立用户配置目录 |
| 标记文档 | Pandoc | Typst/WeasyPrint 等可选 PDF 引擎 | 网络资源默认禁用或显式允许 |
| PDF | qpdf + PDFium | Poppler 可选 | PDF 渲染与结构操作分离 |
| 元数据 | ExifTool | Rust 原生解析器 | 清理操作必须先显示字段差异 |
| 结构化数据 | Rust 原生 | 后续 DuckDB 插件 | 严格处理类型、嵌套、空值与编码 |

### 6.3 明确不采用的首发架构

- 不让 Electron/Node 承担转换核心。
- 不把纯浏览器 WASM 作为超大文件首发执行环境。
- 不直接把 FFmpeg C API、LibreOffice UNO 和所有图像库同时 FFI 链接进主进程。
- 不用 Redis、Postgres、Kubernetes 或分布式队列解决本地 v0.1 问题。
- 不开放任意 Shell 模板作为插件系统。

## 7. 桌面 UX 规格

### 7.1 主流程

1. **添加**：拖放文件或文件夹，也可由右键菜单进入。
2. **理解**：自动 Inspect，显示真实格式和关键属性。
3. **选择**：推荐目标或选择预设。
4. **确认**：展示转换计划、损失、预计空间和警告。
5. **执行**：显示队列、逐项进度、速度、剩余时间和控制按钮。
6. **验证**：完成后展示 Pass、Warning 或 Fail，并可打开报告和输出目录。

### 7.2 普通模式

- 默认只呈现目标、质量、尺寸、元数据和保存位置。
- 推荐选项用人类语言解释。
- 最常见任务三次操作内开始。
- 不显示不必要的 codec、pixel format 和 muxer 名称。

### 7.3 专家模式

- 显示容器、编码器、码率、CRF、采样率、像素格式、色彩、轨道、字幕和章节。
- 显示实际引擎命令预览，但不可直接通过 Shell 执行。
- 允许固定引擎、禁止有损路径、限制临时空间和保存配方。

### 7.4 任务与历史

- 按状态筛选。
- 批量重试失败项。
- 只重新验证，不重新转换。
- 显示实际输出路径和冲突处理结果。
- 可导出任务配方与验证报告。
- 默认历史不记录文件内容；路径显示可选择脱敏。

### 7.5 错误体验

错误必须回答：

- 哪一步失败。
- 引擎返回了什么。
- 输入是否可能损坏。
- 是否缺少编码器、字体、权限或磁盘空间。
- 可以安全重试还是需要改变设置。
- partial 文件是否保留以及如何清理。

### 7.6 国际化与可访问性

- 首发简体中文和英文。
- UI 文本不硬编码在组件中。
- 键盘可完成完整工作流。
- 支持屏幕阅读器、焦点状态、高对比度和减少动画。
- 颜色不是唯一状态标识。
- RTL 文件内容和文件名进入测试语料。

## 8. 质量验证规格

### 8.1 按类型验证

| 类型 | 必检项 | 重要警告 |
|---|---|---|
| 图片 | 可解码、尺寸、方向、Alpha、ICC、位深 | EXIF 丢失、色彩空间改变、透明度丢失、动画变静态 |
| 视频 | 可解码、时长、分辨率、帧率、视频轨道 | HDR/色彩变化、帧率变化、章节丢失、旋转信息改变 |
| 音频 | 可解码、时长、声道、采样率、音轨数 | 标签、封面、声道布局或无缝信息丢失 |
| 字幕 | 条目数、开始结束时间、编码 | 样式、位置、字体和 RTL 变化 |
| PDF | 可打开、页数、页面尺寸、字体、文本可提取性 | 页面增减、字体替换、透明度或表单丢失 |
| Office | 输出 PDF 可打开、页数、页面渲染 | 字体缺失、页数显著变化、版式视觉漂移 |
| 结构化数据 | 可解析、记录数、字段、类型、空值 | 嵌套展开歧义、数值精度、日期与编码变化 |

### 8.2 视觉比较

Office 到 PDF 的 v0.1 验证流程：

1. 使用引擎生成 PDF。
2. 渲染页面缩略图。
3. 检查页数、页面尺寸和字体警告。
4. 若输入可由 LibreOffice 预览，生成参考渲染并计算感知差异。
5. 差异超过阈值时标记 Warning，不伪装为完全保真。

视觉分数只能作为证据之一，不能取代结构检查。

### 8.3 报告可复现性

报告必须携带：

- FormatWright 版本。
- 操作系统和架构。
- 引擎版本与构建配置。
- 计划摘要哈希。
- 输入与输出哈希。
- 时间和资源摘要。
- 隐私策略状态。

默认报告不嵌入原始文件内容。

## 9. 安全、隐私与供应链

### 9.1 威胁模型

需要防御：

- 伪装扩展名和畸形文件。
- 解析器或编解码器漏洞。
- 命令注入。
- 路径遍历、符号链接逃逸和输出覆盖。
- 超大尺寸、压缩炸弹和资源耗尽。
- 恶意插件和被替换的引擎二进制。
- 文档转换时的外部资源请求。
- 自动更新或遥测泄露路径及文件信息。

### 9.2 强制控制

- 使用进程参数数组，不使用 Shell 拼接。
- 子进程使用最小环境变量。
- 每个任务隔离工作目录。
- 对 URL、协议和外部资源使用白名单；默认只允许本地文件与必要 Pipe。
- 所有下载校验哈希和签名。
- 引擎包记录 SBOM、许可证和来源。
- 输出路径规范化并检查是否越出授权目录。
- MCP 和 Server 模式只允许访问显式 allowlist 目录。
- 转换进程设置超时、内存、CPU 和磁盘策略。
- 崩溃文件与日志不包含文件内容，路径可脱敏。

### 9.3 可证明的零网络

v0.1 必须包含自动化测试：

- 转换测试期间启动网络监视或网络 canary。
- 所有黄金工作流在断网环境通过。
- 默认 Content Security Policy 禁止前端任意外联。
- 引擎协议配置禁止网络输入。
- 更新检查和引擎下载只有用户启用后才能访问网络。
- 发布说明明确列出哪些动作可能联网。

### 9.4 代码签名与发布完整性

- Windows 安装包签名。
- macOS 签名与公证。
- Linux 发布校验和与签名。
- 每个发布生成 SBOM。
- 发布 SHA-256 和签名文件。
- 认证引擎包与主程序独立版本化。
- Alpha 可在无证书情况下发布测试构建；Public Beta/Stable 必须满足签名门槛。

## 10. 数据与本地存储

### 10.1 SQLite 使用范围

保存：

- Jobs。
- Job Steps。
- Events 索引。
- Presets。
- Engine Inventory。
- Validation Summaries。
- Schema Migrations。

不保存：

- 用户文件内容。
- 未经同意的使用分析。
- 密钥和令牌明文。

### 10.2 文件存储

- 大型日志、完整报告和中间文件存放在任务目录，不写入 SQLite Blob。
- partial 文件名包含任务 ID，但不暴露到最终输出。
- 任务结束后按策略清理。
- 提供一键清除历史与缓存，并预览将删除的内容。

### 10.3 迁移与兼容

- 数据库 Schema 有单向迁移。
- 升级前创建轻量备份。
- 迁移失败时不破坏原数据库。
- 配方和报告使用独立版本化 JSON Schema。

## 11. 性能与可靠性指标

### 11.1 v0.1 硬指标

| ID | 指标 | 目标 |
|---|---|---|
| FW-NFR-001 | 10GB 单文件 | 主程序不 OOM；控制面内存不随输入大小线性增长 |
| FW-NFR-002 | 10,000 文件队列 | 可创建、分页、暂停、恢复、重试 |
| FW-NFR-003 | 控制面内存 | 参考环境中目标 RSS 不超过 250MB，不含转换引擎 |
| FW-NFR-004 | 取消响应 | 发出取消后 3 秒内进入终止流程并阻止提交输出 |
| FW-NFR-005 | 状态持久化 | 每个状态转换事务化；崩溃后无“假成功” |
| FW-NFR-006 | 离线 | 12 条黄金工作流在断网环境全部可运行 |
| FW-NFR-007 | 确定性 | 相同能力清单、输入 Probe 和约束产生相同 Plan |
| FW-NFR-008 | 覆盖安全 | 默认不覆盖；所有覆盖均有明确授权记录 |
| FW-NFR-009 | 支持语料成功率 | 支持范围内至少 95% 成功，剩余均给出明确失败原因 |
| FW-NFR-010 | 主程序稳定性 | Public Beta 目标 99.5% 无崩溃会话 |

### 11.2 测量规则

- 指标必须在固定参考硬件和操作系统镜像中复现。
- 引擎 CPU 和内存与主程序控制面分开记录。
- 10GB 测试文件由可复现脚本生成或从合法测试包获取，不提交巨型二进制到 Git。
- 每次发布保留性能基线，显著回退阻止发布。

## 12. 测试计划

### 12.1 测试层级

1. 单元测试：领域模型、格式归一化、命名和状态机。
2. 属性测试：规划器终止、硬约束、确定性、路径安全。
3. 合约测试：每个引擎适配器的 capability、进度、取消和错误映射。
4. 集成测试：真实引擎和黄金文件。
5. 恢复测试：强制终止主程序、引擎或操作系统会话。
6. 端到端测试：CLI 与桌面主流程。
7. 模糊测试：Probe、Manifest、插件协议和结构化数据解析。
8. 安全测试：命令注入、路径遍历、符号链接、外联和恶意文件。
9. 性能测试：10GB、10,000 文件、长队列和并发限制。

### 12.2 测试语料分类

- 正常小文件。
- 超大文件。
- 零字节与截断文件。
- 扩展名错误。
- Unicode、超长路径、空格、特殊字符。
- RTL 文本和文件名。
- 多音轨、多字幕、多章节。
- HDR、ICC、Alpha、EXIF。
- 缺失字体和复杂 Office 布局。
- 损坏 PDF、加密 PDF、表单 PDF。
- 嵌套数据、空值、超大数值和混合编码。

每个样本必须记录：

- 来源和许可证。
- 输入哈希。
- 预期 Probe。
- 允许与禁止的变化。
- 支持的平台与引擎版本。

### 12.3 CI 矩阵

- Windows 11 x64。
- macOS 当前与前一主版本，Apple Silicon 为主，x64 通过补充构建验证。
- Ubuntu LTS x64。
- Rust fmt、clippy、test、audit、deny。
- 前端 lint、typecheck、unit、accessibility smoke。
- 安装包构建和最小启动测试。

## 13. 开源、许可证与治理

### 13.1 第一方许可证

- Rust Core、CLI、Desktop 和插件 SDK：Apache-2.0。
- 官方自托管服务：AGPL-3.0。
- 文档：CC BY 4.0。
- 测试文件逐项记录原始许可证，不默认与代码同许可。

目的：

- 核心和 SDK 易于集成。
- 托管服务的修改保持开放。
- 所有核心功能继续开源，不制造“社区版阉割”。

### 13.2 第三方引擎原则

- FFmpeg 主要为 LGPL，但可选组件可能触发 GPL；不启用 nonfree 发布配置。
- [FFmpeg Legal](https://ffmpeg.org/legal.html)要求每个认证构建记录配置和许可证。
- Ghostscript 具有 AGPL/商业双许可，v0.1 不默认捆绑。
- [Ghostscript Licensing FAQ](https://ghostscript.com/faq/index.html)
- Pandoc、Poppler、ExifTool 等按独立程序和引擎包管理，并完成再分发合规审查。
- 本节是工程策略，不替代正式法律意见。

### 13.3 社区治理

- 使用 DCO，不在早期引入复杂 CLA。
- 采用 Conventional Commits 和 Semantic Versioning。
- 重要架构变更必须写 ADR。
- 大功能先 RFC，再实现。
- 发布公开 Roadmap、SECURITY.md、CONTRIBUTING.md 和 Code of Conduct。
- 安全问题提供私密报告渠道。
- 插件注册表显示维护状态、签名、权限和兼容版本。

## 14. 发布与分发

### 14.1 桌面发行物

- Windows：MSI/NSIS。
- macOS：DMG/App Bundle，签名与公证。
- Linux：AppImage，后续补充 deb/rpm/Flatpak。
- Portable：无安装便携包。
- Offline Bundle：主程序 + 认证引擎包 + 许可证 + 校验和。

### 14.2 引擎分发

Release 默认顺序：

1. 检测并验证 FormatWright Engine Registry 中已激活的认证包。
2. Offline Bundle/安装介质提供 Windows Starter pack；可选能力由用户主动下载或离线导入。
3. 缺失能力时禁用对应路线并展示安装/导入动作；已有可用能力不受影响。
4. 未知系统 PATH 只在显式开发模式中可作为候选发现；生产 Release 不执行它，即使专家模式也不能绕过身份验证。

### 14.3 更新通道

- nightly：开发者测试。
- beta：功能冻结后的真实用户验证。
- stable：通过签名、语料和回归门槛。
- 默认不静默更新；用户可选择自动下载但安装前确认。

## 15. 商业与可持续性

所有产品核心能力保持开源。可持续收入候选：

- 官方托管 API。
- 企业支持与 SLA。
- 认证、签名和长期维护的离线引擎包。
- 特殊行业或商业格式引擎适配。
- 企业策略、审计和部署服务。
- 培训、迁移和定制集成。
- 赞助与捐赠。

明确不做：

- 出售文件或使用数据。
- 用隐私换免费额度。
- 把恢复、验证和安全能力放入付费墙。
- 用虚高格式数字诱导付费。

## 16. 交付路线图

以下工期按一名主要开发者全职投入、AI 辅助、设计和测试并行估算；它是规划基线，不是对发布日期的承诺。

### Phase 0 — 项目定义与仓库基础（2–3 天）

交付：

- 确认 FormatWright 名称和发布前复核清单。
- 固化本 SPEC_PLAN。
- 建立 Monorepo、许可证和基础文档。
- 建立 ADR 模板和首批架构决策。
- 把 12 条黄金工作流拆成测试清单。

退出条件：

- 范围、非目标、许可证和技术栈有明确记录。
- 每条黄金工作流都有输入样本计划和验收项。

### Phase 1 — 架构风险验证（1 周）

必须完成的 Spike：

1. Rust 启动 FFmpeg，解析 ffprobe JSON 和进度。
2. 10GB 文件路径执行，验证控制面内存非线性增长。
3. Windows/macOS/Linux 子进程取消与进程树清理。
4. partial 写入、验证和原子提交。
5. SQLite 状态写入，强制终止后恢复。
6. Tauri 事件桥接 10,000 个任务时不阻塞 UI。
7. 引擎能力 Manifest 和确定性 Plan 示例。
8. FFmpeg、libvips、LibreOffice、Pandoc、PDF 引擎的分发与许可证清单。

退出条件：

- 上述 Spike 全部有可运行代码、测试和 ADR。
- 若 10GB、恢复或跨平台进程控制失败，停止 UI 扩展并解决核心风险。

### Phase 2 — Core + CLI Alpha（2 周）

交付：

- Artifact、Format、Probe、Capability、Constraint、Plan、Job 模型。
- Inspect、Plan、Convert、Batch、Doctor CLI。
- FFmpeg/ffprobe、libvips、Rust 数据转换适配器。
- 基础验证器和 JSON 报告。
- GW-01 至 GW-07、GW-11 的 Alpha。

退出条件：

- CLI 能完成至少 8 条黄金工作流。
- 相同输入产生确定计划。
- 转换失败不产生假成功输出。

### Phase 3 — 批处理、恢复与质量（2 周）

交付：

- SQLite 队列和完整状态机。
- 递归目录、保留结构、命名模板和冲突策略。
- 暂停、恢复、取消、崩溃恢复、重试和跳过完成项。
- 类型化验证报告。
- LibreOffice、Pandoc、PDF 适配器。
- 完成全部 12 条黄金工作流。

退出条件：

- 10,000 文件批处理通过。
- 强制退出恢复测试通过。
- 12 条工作流均有验证器，不出现 Unknown 被当作 Pass。

### Phase 4 — Desktop Beta（2 周）

交付：

- Tauri 2 + React 桌面端。
- 普通/专家模式。
- 拖放、推荐目标、计划预览、队列、历史、报告、预设。
- 引擎 Doctor 和安装/导入流程。
- Windows 右键菜单；macOS/Linux 集成 Beta。
- 中英文与基础可访问性。

退出条件：

- 新用户三分钟内可以完成首次转换。
- 关键流程键盘可操作。
- UI 关闭与重启不丢任务。

### Phase 5 — 安全加固与 Public Beta（2 周）

交付：

- 完整测试语料、性能基线、模糊测试和安全测试。
- 零网络验证。
- 安装包、签名、公证、SBOM、校验和。
- SECURITY、CONTRIBUTING、隐私说明和故障排查文档。
- Beta 反馈和崩溃恢复机制。

退出条件：

- 通过 0.3 节全部 Public Beta 门槛。
- 没有 P0/P1 已知缺陷。
- 许可证清单完整。

### Phase 6 — API、自托管与 MCP（3–4 周，Beta 反馈后）

交付：

- Axum REST API。
- SSE 进度、Webhook、OpenAPI。
- Docker 单机 Worker。
- 目录 allowlist、资源限制和审计。
- MCP Adapter：先 plan 后 execute，覆盖需确认，不允许任意 Shell 参数。

退出条件：

- API 和 CLI 对同一输入生成等价 Plan。
- 默认安全策略能阻止越权目录和未授权覆盖。

### Phase 7 — 浏览器与企业能力（按需求排序）

- WASM 适合的小型图片、数据和文档工作流。
- 大媒体和复杂 Office 通过本地 Worker。
- 企业全局网络禁用、引擎白名单、策略签名、SSO、审计和离线升级。
- 分布式任务仅在单机吞吐成为真实瓶颈后引入。

### 16.1 总工期基线

- CLI Alpha：约 3–4 周。
- Desktop Beta：约 7 周。
- Public Beta：约 9–10 周。
- 自托管/API/MCP：Public Beta 后约 3–4 周。

## 17. 工作分解与 Epic

| Epic | 名称 | 主要输出 | 依赖 |
|---|---|---|---|
| E00 | Repo/Foundation | Monorepo、CI、许可、ADR、开发环境 | 无 |
| E01 | Domain/Formats | 核心模型、格式注册表、Schema | E00 |
| E02 | Inspect | 文件探测、归一化 Probe | E01 |
| E03 | Planner | 能力图、约束、路径解释 | E01、E02 |
| E04 | Engine SDK | Manifest、协议、进程执行、安全参数 | E01 |
| E05 | Media/Image Engines | FFmpeg、ffprobe、libvips | E02、E04 |
| E06 | Document/Data Engines | LibreOffice、Pandoc、PDF、Rust Data | E02、E04 |
| E07 | Queue/Recovery | SQLite、状态机、暂停恢复、幂等 | E03、E04 |
| E08 | Validators | 图片、媒体、PDF、Office、数据报告 | E02、E05、E06 |
| E09 | CLI | Commands、JSON、退出码、配方 | E03、E07、E08 |
| E10 | Desktop | Tauri、React、队列、报告、系统集成 | E07、E08 |
| E11 | Security/Packaging | 签名、SBOM、零网络、引擎包 | E04、E10 |
| E12 | Docs/Community | 用户文档、贡献指南、测试语料政策 | E00 起持续 |
| E13 | Server/MCP | Axum、SSE、Docker、MCP | Public Beta 后 |

### 17.1 第一批实现 Issues

1. 初始化 Cargo/pnpm workspace。
2. ADR-001：一个核心、多入口。
3. ADR-002：引擎子进程优先于 FFI。
4. ADR-003：SQLite 任务状态与恢复。
5. ADR-004：引擎包和许可证隔离。
6. 定义 Format、Artifact 和 Probe Schema v1。
7. 实现 ffprobe Inspect。
8. 定义 Capability Manifest v1。
9. 实现重封装优先的最小 Planner。
10. 实现安全 Process Runner。
11. 实现 partial + atomic commit。
12. 实现 Job 状态机和 SQLite migrations。
13. 实现 CLI inspect、plan、convert、doctor。
14. 建立首批媒体与图片黄金文件。
15. 建立 10GB 与 10,000 文件性能脚本。

## 18. 成功指标

### 18.1 北极星指标

**每周完成并通过验证的本地转换任务数。**

该指标只在用户明确选择匿名遥测时统计；项目不能依赖默认收集。默认情况下通过可选本地统计页和用户调研获取。

### 18.2 产品指标

| 指标 | Public Beta 目标 |
|---|---:|
| 首次成功时间 | 80% 新用户在 3 分钟内 |
| 支持语料成功率 | 至少 95% |
| 验证覆盖率 | 100% 成功任务有报告 |
| 假成功 | 0 个已知案例 |
| 崩溃恢复 | 100% 测试场景状态一致 |
| 默认转换外联 | 0 |
| 错误可操作性 | 至少 90% 测试错误给出下一步 |
| 无崩溃会话 | 99.5% 目标 |

### 18.3 社区指标

- 外部贡献者能够在 30 分钟内跑通开发环境。
- 首个插件示例不需要修改 Core。
- Issue 模板能收集样本哈希、引擎版本、Plan 和报告。
- 主要架构决策公开且可追溯。

## 19. 风险、缓解与停止条件

| 风险 | 影响 | 缓解 | 停止/转向条件 |
|---|---|---|---|
| 格式数量诱发范围膨胀 | 延期、低质量 | 固定 12 条黄金工作流；新格式走 RFC | 核心可靠性未过门槛时不加格式 |
| Office 保真不可控 | 用户不信任 | 多引擎、字体检查、视觉比较、明确 Warning | 无法测量时不宣称高保真 |
| 引擎许可证/专利 | 无法安全分发 | 模块化引擎包、构建配置、法律复核 | 许可不清的引擎不进入认证包 |
| 大文件仍经应用内存 | OOM、数据丢失 | 数据面直读写、10GB 回归门槛 | Spike 不通过则暂停 UI |
| 跨平台进程控制差异 | 无法取消、残留进程 | Job Object、进程组、平台集成测试 | 任一平台无法安全取消则降级为实验支持 |
| 恶意输入利用引擎 | 安全事件 | 隔离、资源限制、更新、模糊测试 | 高危未修复时暂停相关引擎 |
| 引擎包过大 | 安装和更新困难 | 按需包、离线 bundle、差分下载后续 | 主安装包不强制塞入全部引擎 |
| 插件系统变成命令执行器 | 注入与供应链风险 | 版本化协议、签名、能力声明、无 Shell | 无法约束权限则不开放公共注册表 |
| 开源可持续性不足 | 维护中断 | 托管、支持、认证包、赞助 | 不以关闭核心功能换短期收入 |
| 名称或商标冲突 | 品牌返工 | 发布前商标、域名、GitHub 和包名检索 | 冲突则在首个公开 Release 前改名 |

## 20. 已定决策与待确认项

### 20.1 已定

- 项目工作名称：FormatWright。
- 产品类别：开源、本地优先的文件转换平台。
- 差异化：大文件、恢复、路径规划、质量验证、自动化。
- 核心：Rust。
- 桌面：Tauri 2 + React/TypeScript。
- 本地存储：SQLite。
- 引擎：独立子进程优先。
- 首发：Desktop + CLI。
- 后续：Axum API、自托管、MCP、浏览器轻任务。
- 默认隐私：零遥测、零转换外联。
- v0.1 范围：12 条黄金工作流。

### 20.2 Phase 1 前必须确认

- FormatWright 的商标、域名、GitHub 组织、crates.io 和 npm 名称。
- Windows/macOS 代码签名预算与账户。
- v0.1 最低支持的操作系统版本。
- 官方引擎包采用“默认下载”还是“安装时选择”。
- PDF 默认渲染引擎最终选择。
- 测试语料的许可证和托管方式。
- 自托管服务 AGPL 与核心 Apache 的仓库边界。

## 21. v0.1 Definition of Done

- [ ] 在无相关系统引擎、无开发缓存、断网的干净机器上，安装后可完成 Starter Core/PDF/Media 真实转换。
- [ ] Release 只解析已验证 pack 的精确路径；污染 PATH 和 `.cmd`/`.bat` 负向测试通过。
- [ ] UI 推荐/可选路线与 Planner/Backend capability snapshot 一致，不可执行能力不伪装为可用。
- [ ] 12 条黄金工作流全部实现。
- [ ] 所有工作流均有 Inspect、Plan、Execute、Validate。
- [ ] 10GB 文件测试通过。
- [ ] 10,000 文件队列测试通过。
- [ ] 暂停、恢复、取消、重试通过。
- [ ] 强制退出恢复通过。
- [ ] 重封装优先策略通过。
- [ ] 所有成功任务都有验证报告。
- [ ] 不存在静默覆盖。
- [ ] 默认零网络测试通过。
- [ ] Windows、macOS、Linux CI 通过。
- [ ] GUI 与 CLI 共用 Core。
- [ ] 安装包、SBOM、哈希和许可证清单齐全。
- [ ] SECURITY、PRIVACY、CONTRIBUTING 和用户文档齐全。
- [ ] 没有 P0/P1 已知缺陷。

## 22. 启动后的前 72 小时

### Day 1

- 初始化仓库和许可证。
- 创建 ADR-001 至 ADR-004。
- 建立 Rust workspace 与最小 CLI。
- 固定 Format、Artifact、Probe 数据结构。

### Day 2

- 接入 ffprobe Inspect。
- 创建 Capability Manifest。
- 实现最小路径规划器。
- 加入 MOV/MKV → MP4 的 remux 计划测试。

### Day 3

- 实现安全 Process Runner。
- 实现 partial 输出和原子提交。
- 创建 SQLite Job 状态机。
- 完成首个端到端命令：inspect → plan → convert → validate → report。

首个垂直切片建议：

**MKV/MOV → MP4：兼容时 remux，不兼容时解释为何转码，并验证时长、轨道和章节。**

它能最早验证 FormatWright 最重要的产品主张：智能路径、超大文件、进度、取消和质量报告。

## 23. 计划维护规则

- 每个 GitHub Epic 必须链接本文件中的 Requirement ID。
- 需求变化先更新 SPEC 或写 ADR，再合并实现。
- 每个里程碑结束更新指标、风险和实际工期。
- 研究数据每季度复核；价格、Stars 和竞品能力标记快照日期。
- 公开发布后，产品承诺与测试门槛的变更必须进入 Changelog。

---

## 附录 A：项目口号候选

- Convert locally. Know what changed.
- File conversion you can verify.
- 把文件转换正确。
- 本地转换，结果有据可查。

## 附录 B：核心参考

- [Tauri 2](https://v2.tauri.app/start/)
- [Tokio](https://tokio.rs/tokio/tutorial)
- [Axum](https://docs.rs/axum/latest/axum/)
- [FFmpeg Documentation](https://www.ffmpeg.org/documentation.html)
- [ffprobe Documentation](https://ffmpeg.org/ffprobe.html)
- [libvips](https://www.libvips.org/)
- [SQLite](https://www.sqlite.org/about.html)
- [Pandoc](https://pandoc.org/)
- [FFmpeg Legal](https://ffmpeg.org/legal.html)
- [Ghostscript FAQ](https://ghostscript.com/faq/index.html)

## 附录 C：版本优先级

| 优先级 | 含义 |
|---|---|
| P0 | 缺失则不能发布 |
| P1 | 应在当前里程碑完成，必要时可明确降级 |
| P2 | 有证据后进入后续版本 |
| P3 | 探索项，不作承诺 |

## 附录 D：执行配套文档与当前证据

本文件保留产品范围、需求与发布门槛；可执行细节由以下受版本控制的配套文档维护：

- `docs/specs/GOLDEN_WORKFLOWS.md`：12 条黄金工作流的逐项验收合同。
- `docs/specs/CORE_SCHEMAS.md` 与 `schemas/`：Probe、Plan、JobEvent、ValidationReport、EngineManifest 的版本化边界。
- `docs/specs/JOB_RECOVERY.md`：任务、输入身份、partial、提交与恢复语义。
- `docs/specs/VALIDATION_RULES.md`：Pass、Warning、Fail、Unknown 及各文件族规则。
- `docs/specs/RESOURCE_SCHEDULER.md`：并发、磁盘、内存和 10GB/10,000 文件门槛。
- `docs/security/THREAT_MODEL.md` 与 `docs/security/ENGINE_SUPPLY_CHAIN.md`：攻击面及引擎认证。
- `docs/specs/TRACEABILITY.md`：Requirement → 实现 → 直接证据的活追踪矩阵。
- `docs/MASTER_EXECUTION_PLAN.md`：基于当前实现证据维护的完成清单、待完成清单、目标架构、模块设计与顺序执行 Gate。
- `docs/testing/SANDBOX_TESTS.md`：隔离沙箱程序和证据解释规则。
- `docs/testing/LARGE_FILE.md`：1 GiB/10 GiB 稀疏文件架构门槛与内存证据。
- `docs/testing/QUEUE_BRIDGE.md`：10,000 任务 Rust → WebView 有界批次与真实窗口证据。
- `docs/testing/DURABLE_QUEUE.md`：10,000 条磁盘 SQLite 任务、分页、输出预约与队列动作证据。
- `docs/testing/STRUCTURED_SANDBOX.md`：GW-11 严格结构化映射、语义摘要、显式有损策略与 Windows 沙箱证据。
- `docs/testing/IMAGE_SANDBOX.md`：GW-02 图像编码、缩放、透明度约束与独立探测的 Windows 沙箱证据。
- `docs/testing/HEIC_SANDBOX.md`：GW-01 真实 HEVC HEIC、libheif 开发回退、独立像素验证，以及 libvips Windows HEVC 能力预检边界。
- `docs/testing/METADATA_SANDBOX.md`：GW-12 媒体元数据分类、值脱敏、流复制与保留 Unknown 的 Windows 沙箱证据。
- `docs/testing/BATCH_SANDBOX.md`：GW-03 递归枚举、目录链接拒绝、输出预约、暂停/恢复与输入变更的 Windows 沙箱证据。
- `docs/testing/MIXED_SCHEDULER.md`：结构化、图像、视频混合队列的确定性资源准入、真实进程并发、RSS 与 SQLite/WAL Windows 观测证据。
- `docs/testing/PRESET_SANDBOX.md`：版本化命名预设、失败原子化导入合并、JSON Schema 与中断写入恢复的 Windows 沙箱证据。
- `docs/testing/DOCUMENT_SANDBOX.md`：GW-10 离线 Pandoc、DOCX/OPC 结构与文本语义摘要，以及经隔离 LibreOffice/Poppler 的全页 PDF 验证、取消恢复的 Windows 沙箱证据。
- `docs/testing/PDF_SANDBOX.md`：GW-09 内容优先 PDF 探测、全页 Poppler 渲染、逐页像素验证与目录原子提交的 Windows 沙箱证据。
- `docs/testing/OFFICE_SANDBOX.md`：GW-08 有界 OOXML 探测、隔离 LibreOffice 转换、全页 PDF 验证、取消恢复与代表页视觉检查的 Windows 沙箱证据。
- `docs/testing/DESKTOP_MVP.md`：Phase 4 共享核心 Tauri 命令、双语普通/专家界面、持久任务/报告、原生启动与像素检查证据。
- `docs/testing/DESKTOP_ACCESSIBILITY.md`：真实 Tauri/WebView2 自动可访问性树、首 Tab 跳转、200% 物理等效视口、RTL 路径、对比度/forced-colors/reduced-motion 与双语语义证据边界。
- `docs/testing/WINDOWS_EXPLORER_INTEGRATION.md`：Windows 经典 Explorer 文件/目录入口、严格路径参数、单实例转发、安装/卸载键归属和隔离安装验收边界。
- `docs/testing/DESKTOP_RELEASE_CONVERSION.md`：真实 Release UI 的 PDF→PNG/JPG CDP 转换证据，每格式独立进程与隔离应用状态、逐项 Pass 报告与确定性页命名边界。
- `docs/testing/TEN_THOUSAND_CONVERSIONS.md`：10,000 个不同结构化输入的真实规划、原子入队、数据库重启、有界窗口执行、语义验证与提交证据。
- `docs/security/FUZZING.md` 与 `fuzz/`：引擎 manifest、JSON/YAML/CSV/XML 解析边界的 libFuzzer/AddressSanitizer harness、定时 CI 与 Windows 有界 campaign 证据。
- `docs/release/SBOM.md` 与 `scripts/generate_sbom.py`：锁定 Cargo/pnpm 依赖的 SPDX 2.3 应用 SBOM 生成与自校验证据；引擎包 SBOM 仍独立管理。
- `docs/security/DEPENDENCY_AUDIT.md` 与 `scripts/audit_dependencies.py`：主应用、fuzz 与生产前端锁文件的零已知漏洞门禁、RustSec/pnpm 失败闭合策略与 MSRV 1.88 证据。
- `docs/release/WINDOWS_PACKAGING.md`、`scripts/generate_checksums.py` 与手动 release-candidate workflow：嵌入离线 WebView2 的 NSIS 构建、显式产物哈希、沙箱安装/启动/卸载与未签名 Alpha 边界证据。
- `PRIVACY.md`、`docs/USER_GUIDE.md`、`docs/TROUBLESHOOTING.md`：本地数据、网络边界、首次转换与类型化恢复操作的用户文档。
- `docs/testing/ZERO_NETWORK.md`：Plan/Runner/协议白名单/UNC 路径硬约束，以及真实 FFmpeg 进程树的 Windows TCP/UDP 观测门禁与局限。

截至 2026-08-13，Phase 0 仓库基础已建立；GW-04 的首个 Windows 开发环境垂直切片已验证 Inspect → Plan → remux/transcode → Validate → atomic same-volume commit，并验证输出冲突、取消和强退恢复。Phase 1 已通过 Windows 10 GiB 稀疏文件控制面门槛，以及真实 Tauri 窗口中的 10,000 任务有界投影门槛。Phase 3 已通过同质与结构化/图片/媒体 10,000 项真实转换、公平性/队列延迟/RSS/WAL、四进程原子认领与真实强退恢复的 Windows 开发门槛；高分辨率/PDF/Office、长时掉电与跨平台认证仍待完成。GW-01 至 GW-12 均只有 Windows 开发环境实验路径，其中 GW-01 使用明确标注的 libheif 开发回退且 libvips HEVC 认证仍待完成。Phase 4 已有共享核心的 Tauri 转换、文件/文件夹预览与批量入队、持久任务/报告/维护/恢复、真实阶段/调度等待、双语普通/专家界面、可编辑预设、Windows 经典 Explorer 实际安装冷/热入口与单实例转发，以及真实 WebView 自动可访问性/200%/RTL/对比度基线；Windows 11 现代顶层菜单、macOS/Linux 集成、现场读屏/物理高 DPI 和正式可用性测试仍待完成。Phase 5 已有隐私/用户/故障文档、SPDX 应用 SBOM、Windows 有界 ASAN fuzz、零已知锁定依赖漏洞门禁、真实进程树零套接字观测，以及内嵌 PDF/Media Starter 的离线 NSIS 本机构建与真实 Release 转换证据；干净离线虚拟机、OS 强制网络隔离、可信签名钥匙环、引擎包 SBOM/许可证义务、升级回滚、正式签名及跨平台 campaign 仍待完成。完成状态以 `docs/specs/TRACEABILITY.md` 和 `docs/MASTER_EXECUTION_PLAN.md` 的逐项审计为准。
