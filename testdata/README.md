# testdata —— 对拍基线数据

本目录存放从参考仓库 `school-kpi/example_data` 复制来的示例数据，以及（后续生成的）golden 输出，
用于引擎 M1 与老程序做**逐格对拍回归**。

## 目录结构

```
testdata/
  2023-09/            # 某个月份的“工作目录”（与老程序 -d 一致）
    *.xlsx            # 直接从 school-kpi/example_data 复制（含 绩效.xlsx 模板等）
  2023-09-golden/     # 【待生成】老程序(Go kpi.exe)跑出的结果 = 对拍基线
    bukeshi-golden/   #   补课时.xlsx
    kpi-golden/       #   写回后的 绩效.xlsx
```

## 如何生成 golden（一次性，需要 Go）

1. 在 `school-kpi` 仓库构建老程序（或使用已发布的 `kpi-x86_64-pc-windows-msvc.exe`）；
2. 复制一份 `2023-09` 目录到临时目录，在其中运行老程序并保留产物：
   ```sh
   # 旧目录 → 先备份原 绩效.xlsx / 补课时.xlsx，再执行：
   kpi gen bukeshi -d <目录>
   kpi gen kpi     -d <目录>
   ```
3. 把生成的 `补课时.xlsx` 与写回后的 `绩效.xlsx` 拷入 `2023-09-golden/` 对应子目录；
4. 记录当时的输入文件指纹与程序版本到 `golden-README.md`（含参数：老参数取自 配置.xlsx）。

> golden 是“老程序真实行为”的冻结快照（含其疑似 bug，如 `min()` 语义），
> 引擎对拍目标 = 与 golden 逐格一致；语义是否需要修正另列清单（见设计稿 R3），不在对拍阶段擅自“纠正”。

## 本目录不是运行目录

引擎/CLI 运行时应把输入**复制**到临时运行目录（或只读打开后写副本），
避免 golden 与输入被破坏；`apply=inplace` 场景另按 3.8 节先 `.bak`。
