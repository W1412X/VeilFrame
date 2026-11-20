"""从 rapidocr 自带的 PP-OCRv4 识别模型 metadata 中导出字符表（不下载任何文件）。

运行：D:\\android_proj\\autop\\poc\\visual-parsing\\.venv\\Scripts\\python.exe scripts\\tools\\extract_ppocr_v4_dict.py
"""
import pathlib

import onnx

REC = pathlib.Path(r"D:\android_proj\autop\poc\visual-parsing\.venv\Lib\site-packages"
                   r"\rapidocr_onnxruntime\models\ch_PP-OCRv4_rec_infer.onnx")
OUT = pathlib.Path(r"D:\vf-models\ocr\ppocr_keys_v4.txt")

model = onnx.load(str(REC))
meta = {p.key: p.value for p in model.metadata_props}
print("metadata keys:", sorted(meta.keys()))
chars = meta.get("character")
if not chars:
    raise SystemExit("rec 模型 metadata 中没有 character 字段")
lines = chars.split("\n")
OUT.parent.mkdir(parents=True, exist_ok=True)
OUT.write_text("\n".join(lines) + "\n", encoding="utf-8")
print("entries:", len(lines), "->", OUT)
