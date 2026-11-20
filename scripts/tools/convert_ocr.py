"""把本地 PP-OCRv5 Paddle 推理权重转换为 ONNX，并导出字符字典。

使用 PoC 虚拟环境中的 paddle2onnx（不下载任何文件）：
  D:\\android_proj\\autop\\poc\\visual-parsing\\.venv\\Scripts\\python.exe scripts\\tools\\convert_ocr.py
产物输出到仓库外的 D:\\vf-models\\ocr（模型不进入 git）。
"""
import pathlib
import subprocess
import sys

import yaml

SRC = pathlib.Path(r"D:\android_proj\autop\poc\visual-parsing\weights")
OUT = pathlib.Path(r"D:\vf-models\ocr")
OUT.mkdir(parents=True, exist_ok=True)

# 1. 字典：识别模型的 inference.yml 中 PostProcess.character_dict
rec_cfg = yaml.safe_load((SRC / "PP-OCRv5_mobile_rec_infer" / "inference.yml").read_text(encoding="utf-8"))
chars = rec_cfg["PostProcess"]["character_dict"]
dict_path = OUT / "ppocr_keys_v5.txt"
dict_path.write_text("\n".join(chars) + "\n", encoding="utf-8")
print(f"dict: {len(chars)} entries -> {dict_path}")

# 2. 检测与识别模型转 ONNX
for name, folder in [("det", "PP-OCRv5_mobile_det_infer"), ("rec", "PP-OCRv5_mobile_rec_infer")]:
    model_dir = SRC / folder
    save = OUT / f"{name}.onnx"
    # paddle2onnx 是控制台脚本（Scripts\paddle2onnx.exe），没有 __main__ 模块
    exe = pathlib.Path(sys.executable).parent / "paddle2onnx.exe"
    cmd = [
        str(exe),
        "--model_dir", str(model_dir),
        "--model_filename", "inference.json",
        "--params_filename", "inference.pdiparams",
        "--save_file", str(save),
        "--opset_version", "11",
        "--enable_onnx_checker", "True",
    ]
    print("converting", name, "->", save)
    subprocess.run(cmd, check=True)
    print("ok", save)
