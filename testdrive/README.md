# Anole 内测成品（未入库）

双击运行：

`E:\Users\Administrator\Desktop\Anole\target\release\formatwright-desktop.exe`

关窗即退出。Explorer 右键 Convert to … 已指到这份 exe（当前用户 HKCU）。Win11 请走「显示更多选项」。

取消右键：

```
pwsh -NoProfile -File scripts\register_dev_explorer_convert.ps1 -Remove
```

本目录已有 CLI 试转结果：`sample.json` → `sample.converted.yaml`。
