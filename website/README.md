# Anole website

Landing page for the Anole project（对应 `docs/COMPETITIVE_GAP_ROADMAP.md` 的 G-05 条目）。这是一个**纯静态、单文件**站点：`index.html` 内联了全部 CSS 与 JS，无构建步骤、无外部字体、无 CDN 依赖，断网状态下可完整打开。

## 本地预览

直接双击打开 `index.html` 即可（`file://` 协议下语言偏好存储会被部分浏览器禁用，页面本身功能不受影响）。也可以起一个静态服务器：

```bash
cd website
python -m http.server 8000
# 或 npx serve .
```

然后访问 `http://localhost:8000`。

## 部署到 GitHub Pages

推荐用 Actions 工作流把 `website/` 目录发布为 Pages 内容（Pages 的 "Deploy from a branch" 只支持仓库根目录或 `/docs`，不支持任意子目录）。在 `.github/workflows/website.yml` 放入：

```yaml
name: Deploy website
on:
  push:
    branches: [main]
    paths: ["website/**"]
permissions:
  contents: read
  pages: write
  id-token: write
jobs:
  deploy:
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - uses: actions/checkout@v4
      - uses: actions/configure-pages@v5
      - uses: actions/upload-pages-artifact@v3
        with:
          path: website
      - uses: actions/deploy-pages@v4
        id: deployment
```

然后在仓库 **Settings → Pages → Source** 选择 **GitHub Actions**。备选方案：把 `website/index.html` 单独推到 `gh-pages` 分支根目录，Pages 指向该分支。

## 上线前需要替换的占位

- `index.html` 底部脚本里的 `REPO_URL = "#"`：改成仓库地址（例如 `https://github.com/owner/FormatWright`），页内所有带 `data-gh` 的 GitHub 链接（含 README `#engines` 锚点）会自动解析。
- 英雄区「下载 v0.1」按钮的 `href="#"`：发布后指向实际安装包地址。

## 设计与实现说明

- **Meadowlark 设计系统**：`:root` 色板与字体栈镜像 `apps/desktop/src/styles.css`（米白纸面 `#f8f4ed`、深墨 `#1f3a46`、朱砂主色 `#e8674a`；Marcellus/Urbanist 本地字体栈，未安装时回退系统衬线/无衬线，中文回退宋体/雅黑）。纸感来自内联 SVG 噪点、回执卡纸叠层与断缘虚线，无任何图片资源。
- **双语**：默认英文，右上角 EN/中文 原生 JS 切换；文案以 `lang="en"` / `lang="zh-Hans"` 成对内联，CSS 按 `html[data-lang]` 隐藏非当前语言，偏好写入 `localStorage`。JS 禁用时回退为纯英文。
- **无障碍**：跳转链接、语义化地标、`aria-pressed` 语言开关、键盘可聚焦的横向滚动表格、可见焦点环、`prefers-reduced-motion` 降级；正文对比度按 WCAG AA 校验（正文用深墨/深青，朱砂仅用于大字号与装饰）。
- **响应式**：≤920px 英雄区与卡片转单列，≤560px 收紧内边距；表格始终可横向滚动。
