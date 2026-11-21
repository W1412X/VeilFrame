"""从 PP-OCRv5 识别 ONNX 的 metadata 中导出字符表，与模型保持一致。

用法：
    python scripts/tools/extract_ppocr_v5_dict.py [rec.onnx] [输出.txt]

不传参数时使用本机 PoC 环境的默认路径。
"""
import pathlib
import sys

import onnx

REC = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else pathlib.Path(r"D:\vf-models\ocr\rec_v5.onnx")
OUT = pathlib.Path(sys.argv[2]) if len(sys.argv) > 2 else pathlib.Path(r"D:\vf-models\ocr\ppocr_keys_v5_onnx.txt")

model = onnx.load(str(REC))
meta = {p.key: p.value for p in model.metadata_props}
print("metadata keys:", sorted(meta.keys()))
chars = meta.get("character")
if not chars:
    raise SystemExit("rec 模型 metadata 中没有 character 字段，需要改用 Paddle 字典 ppocr_keys_v5.txt")
lines = chars.split("\n")
OUT.write_text("\n".join(lines) + "\n", encoding="utf-8")
print("entries:", len(lines), "->", OUT)
