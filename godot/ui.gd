extends Control
## 薄胶水：把按钮事件转发给 Rust 侧 KpiBridge，并渲染返回的 JSON。
## 业务逻辑一律在 kpi-engine / kpi_tool(Rust)，本文件不做计算。

var bridge: Node
var _frame := 0
var _auto_validated := false
var _auto_dry := false

func _ready() -> void:
	bridge = KpiBridge.new()
	add_child(bridge)

	%ValidateBtn.pressed.connect(_on_validate)
	%DryRunBtn.pressed.connect(_on_run.bind(true))
	%RunBtn.pressed.connect(_on_run.bind(false))

	_log("就绪：方案/月份路径已填默认值，可点按钮试跑。")

func _process(_delta: float) -> void:
	# 开发自检：设置环境变量 KPI_SHOT=1 时自动跑一遍并渲染截图退出。
	if OS.get_environment("KPI_SHOT") == "1":
		_frame += 1
		if _frame == 30 and not _auto_validated:
			_auto_validated = true
			_on_validate()
		if _frame == 60 and not _auto_dry:
			_auto_dry = true
			_on_run(true)
		if _frame == 150:
			var img := get_viewport().get_texture().get_image()
			var out := ProjectSettings.globalize_path("res://../target/ui-shot.png")
			var err := img.save_png(out)
			print("KPI_SHOT saved=", out, " err=", err)
			get_tree().quit()

func _global(path: String) -> String:
	return ProjectSettings.globalize_path(path)

func _on_validate() -> void:
	_log("→ 校验 " + %SchemePath.text)
	var res = bridge.validate_scheme(_global(%SchemePath.text))
	_render(res)

func _on_run(dry: bool) -> void:
	_log("→ " + ("Dry-run 预览" if dry else "实际运行") + "："
		+ %SchemePath.text + "  @  " + %MonthDir.text)
	var res = bridge.run_scheme(
		_global(%SchemePath.text), _global(%MonthDir.text), dry)
	_render(res)

func _render(raw: String) -> void:
	var parsed = JSON.parse_string(raw)
	if parsed is Dictionary and parsed.has("ok") and not parsed["ok"]:
		_log("❌ " + str(parsed.get("error", "未知错误")))
		return
	_log(JSON.stringify(parsed, "  "))

func _log(line: String) -> void:
	print(line)
	%Log.append_text(line + "\n")
