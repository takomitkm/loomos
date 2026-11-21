#!/usr/bin/env python
"""vision 真引擎 Python 侧验证（cv2 dnn 驱动，绕开本机 ORT dll 环境问题）。

真跑契约（models 来自 opencv_zoo，与 face-video 板同源）：
- 真 YuNet：合影检出大量人脸（bbox+5 点+score）；
- 真 SFace：128 维嵌入，同人 cos=1.0、不同人显著低（区分度证据）。

用法：python scripts/vision_python_validate.py [zoo根目录] [合影图]
输出：单行 JSON 证据（人脸数/嵌入维度/cos 值），非 0 退出=失败。
依赖：pip install opencv-python（本机 cv2 5.1 验证通过）。
"""
import json
import sys

import cv2


def main():
    zoo = sys.argv[1] if len(sys.argv) > 1 else r"D:/yzd/models-ort/zoo.git"
    img_path = (
        sys.argv[2]
        if len(sys.argv) > 2
        else zoo + r"/models/face_detection_yunet/example_outputs/largest_selfie.jpg"
    )
    img = cv2.imread(img_path)
    if img is None:
        print(f"读图失败: {img_path}", file=sys.stderr)
        sys.exit(1)

    det = cv2.FaceDetectorYN.create(
        zoo + r"/models/face_detection_yunet/face_detection_yunet_2023mar.onnx",
        "",
        (img.shape[1], img.shape[0]),
        0.6,
        0.3,
    )
    det.setInputSize((img.shape[1], img.shape[0]))
    ok, faces = det.detect(img)
    n = 0 if faces is None else len(faces)
    if n < 10:
        print(f"人脸数不足: {n}", file=sys.stderr)
        sys.exit(1)

    rec = cv2.FaceRecognizerSF.create(
        zoo + r"/models/face_recognition_sface/face_recognition_sface_2021dec.onnx", ""
    )
    top = sorted(faces, key=lambda f: -float(f[14]))[:3]
    feats = [rec.feature(rec.alignCrop(img, f)) for f in top]
    cos01 = float(rec.match(feats[0], feats[1], 0))
    cos12 = float(rec.match(feats[1], feats[2], 0))
    cos00 = float(rec.match(feats[0], feats[0], 0))
    dim = int(feats[0].shape[1])

    out = {
        "engine": "cv2 dnn (OpenCV %s)" % cv2.__version__,
        "image": img_path,
        "image_shape": list(img.shape),
        "faces_detected": n,
        "top_score": round(float(top[0][14]), 3),
        "sface_dim": dim,
        "cos_same": round(cos00, 4),
        "cos_cross_01": round(cos01, 4),
        "cos_cross_12": round(cos12, 4),
        "threshold_zoo": 0.363,
        "verdict": (
            "PASS" if cos00 >= 0.9999 and max(cos01, cos12) < 0.363 else "INCONCLUSIVE"
        ),
    }
    print(json.dumps(out, ensure_ascii=False))
    if out["verdict"] != "PASS":
        sys.exit(1)


if __name__ == "__main__":
    main()
