#!/usr/bin/env python
"""稳健下载 sherpa 模型：Range 续传 + Content-Range 总长校验 + 完整性验证 + 自动解包。

用法：python scripts/dl_sherpa_models.py <工作目录>
完成后目录结构：<dir>/asr/*.onnx|tokens.txt、<dir>/tts/*、<dir>/silero_vad.onnx
"""
import os
import sys
import time
import urllib.request
import tarfile

JOBS = [
    # (输出名, URL, 解包到子目录, 是否 tar.bz2)
    ("asr.tar.bz2",
     "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-small-bilingual-zh-en-2023-02-16.tar.bz2",
     "asr", True),
    ("tts.tar.bz2",
     "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-melo-tts-zh_en.tar.bz2",
     "tts", True),
    ("silero_vad.onnx",
     "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx",
     None, False),
]


def total_size(url: str) -> int:
    # Range 0-0 → Content-Range 给出总长（绕过 HEAD 不可靠）
    req = urllib.request.Request(url, headers={"Range": "bytes=0-0"})
    with urllib.request.urlopen(req, timeout=60) as r:
        cr = r.headers.get("Content-Range", "")
        if "/" in cr:
            return int(cr.rsplit("/", 1)[1])
        return int(r.headers.get("Content-Length", 0))


def download(url: str, path: str, label: str) -> None:
    total = total_size(url)
    print(f"[{label}] 总长 {total} 字节", flush=True)
    for attempt in range(60):
        have = os.path.getsize(path) if os.path.exists(path) else 0
        if have >= total and total > 0:
            break
        req = urllib.request.Request(url, headers={"Range": f"bytes={have}-{total - 1}"})
        try:
            with urllib.request.urlopen(req, timeout=60) as r, open(path, "ab") as f:
                while True:
                    chunk = r.read(1024 * 256)
                    if not chunk:
                        break
                    f.write(chunk)
        except Exception as e:  # noqa: BLE001 —— 网络抖动：续传重试
            print(f"[{label}] 中断于 {os.path.getsize(path)}: {e}，2s 后续传", flush=True)
            time.sleep(2)
    got = os.path.getsize(path)
    if got != total:
        print(f"[{label}] 失败：{got}/{total}", flush=True)
        sys.exit(1)
    print(f"[{label}] 下载完成 {got} 字节", flush=True)


def main() -> None:
    root = sys.argv[1]
    os.makedirs(root, exist_ok=True)
    for out_name, url, extract_dir, is_tar in JOBS:
        path = os.path.join(root, out_name)
        if os.path.exists(path) and (
            (not is_tar) or tarfile_is_complete(path)
        ):
            print(f"[{out_name}] 已存在且完整，跳过", flush=True)
        else:
            download(url, path, out_name)
        if is_tar and extract_dir and tarfile_is_complete(path):
            dest = os.path.join(root, extract_dir)
            os.makedirs(dest, exist_ok=True)
            with tarfile.open(path, "r:bz2") as tf:
                members = tf.getmembers()
                # 统一 strip-components=1：按顶层目录名前缀裁剪
                top = {m.name.split("/")[0] for m in members if "/" in m.name}
                for m in members:
                    if m.name.split("/")[0] in top and "/" in m.name:
                        m.name = m.name.split("/", 1)[1]
                tf.extractall(dest, filter="data")
            print(f"[{out_name}] 解包 → {dest}", flush=True)
    print("ALL-DONE", flush=True)


def tarfile_is_complete(path: str) -> bool:
    try:
        with tarfile.open(path, "r:bz2") as tf:
            for m in tf:
                pass
        return True
    except Exception:  # noqa: BLE001
        return False


if __name__ == "__main__":
    main()
