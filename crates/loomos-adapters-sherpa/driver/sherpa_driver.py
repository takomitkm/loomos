#!/usr/bin/env python
# sherpa_driver.py — loomos-adapters-sherpa 的 Python 侧驱动（子进程被调方）。
#
# 设计（03/04 号文档的「参考 OS 语义删减」路线的镜像：引擎侧走
# sherpa-onnx 官方 Python API，平台无关——Windows / aarch64 Linux 同一套命令）：
#   asr  : 流式 zipformer（板同款 14M int8）整句解码 → {"text": ...}
#   tts  : vits-melo-tts-zh_en 合成 → 写 wav → {"audio_len","sample_rate","seconds"}
#
# 约定：单行 JSON 到 stdout；任何失败 exit 非 0 并把原因写到 stderr。

import argparse
import json
import os
import sys
import wave


def read_wav_16k_mono(path: str) -> list:
    """读 PCM16 WAV，返回 float32 标量列表（-1..1）。非 16k 线性重采样到 16k。"""
    with wave.open(path, "rb") as w:
        assert w.getnchannels() == 1, f"要求单声道，实际 {w.getnchannels()}"
        assert w.getsampwidth() == 2, f"要求 PCM16，实际 {w.getsampwidth()}"
        src_rate = w.getframerate()
        frames = w.getnframes()
        raw = w.readframes(frames)
    xs = [int.from_bytes(raw[i : i + 2], "little", signed=True) / 32768.0 for i in range(0, len(raw), 2)]
    if src_rate == 16000:
        return xs
    # linear 插值重采样（与 loomos-audio dsp 同口径；TTS 44100 → 16000）
    n_out = int(len(xs) * 16000 / src_rate)
    if n_out <= 1:
        return xs
    step = (len(xs) - 1) / (n_out - 1)
    out = []
    for i in range(n_out):
        pos = i * step
        i0 = int(pos)
        i1 = min(i0 + 1, len(xs) - 1)
        frac = pos - i0
        out.append(xs[i0] + (xs[i1] - xs[i0]) * frac)
    return out


def write_wav_pcm16(path: str, samples, rate: int) -> None:
    import struct

    ints = [max(-32768, min(32767, int(s * 32768))) for s in samples]
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(b"".join(struct.pack("<h", v) for v in ints))


def cmd_asr(args) -> None:
    import sherpa_onnx

    samples = read_wav_16k_mono(args.wav)
    def pick(*names):
        for n in names:
            if os.path.exists(f"{args.model_dir}/{n}"):
                return f"{args.model_dir}/{n}"
        raise FileNotFoundError(f"模型目录 {args.model_dir} 缺少 {names[0]}（及备选）")

    rec = sherpa_onnx.OnlineRecognizer.from_transducer(
        encoder=pick("encoder-epoch-99-avg-1.int8.onnx", "encoder-epoch-99-avg-1.onnx"),
        decoder=pick("decoder-epoch-99-avg-1.onnx"),
        joiner=pick("joiner-epoch-99-avg-1.int8.onnx", "joiner-epoch-99-avg-1.onnx"),
        tokens=pick("tokens.txt"),
        num_threads=args.num_threads,
        decoding_method="greedy_search",
        provider="cpu",
    )
    stream = rec.create_stream()
    stream.accept_waveform(16000, samples)
    stream.input_finished()
    while rec.is_ready(stream):
        rec.decode_stream(stream)
    print(json.dumps({"text": rec.get_result(stream).strip()}, ensure_ascii=False))


def discover_rule_fsts(model_dir: str) -> str:
    """自动发现文本正规化 fst（不同模型包命名不同：rule.fst / date/number/phone 等）。"""
    import os
    candidates = ["rule.fst", "date.fst", "number.fst", "phone.fst", "new_heteronym.fst"]
    found = [
        os.path.join(model_dir, c)
        for c in candidates
        if os.path.exists(os.path.join(model_dir, c))
    ]
    return ",".join(found)


def cmd_tts(args) -> None:
    import sherpa_onnx

    model_dir = args.model_dir
    vits = sherpa_onnx.OfflineTtsVitsModelConfig(
        model=f"{model_dir}/model.onnx",
        lexicon=f"{model_dir}/lexicon.txt",
        tokens=f"{model_dir}/tokens.txt",
        data_dir=f"{model_dir}/dict",
        dict_dir="",
    )
    config = sherpa_onnx.OfflineTtsConfig(
        model=sherpa_onnx.OfflineTtsModelConfig(
            vits=vits,
            num_threads=args.num_threads,
        ),
        rule_fsts=discover_rule_fsts(model_dir),
        max_num_sentences=1,
    )
    tts = sherpa_onnx.OfflineTts(config)
    audio = tts.generate(args.text, sid=0, speed=args.speed)
    write_wav_pcm16(args.out, audio.samples, audio.sample_rate)
    print(
        json.dumps(
            {
                "audio_len": len(audio.samples),
                "sample_rate": audio.sample_rate,
                "seconds": round(len(audio.samples) / audio.sample_rate, 3),
            },
            ensure_ascii=False,
        )
    )


def main() -> None:
    ap = argparse.ArgumentParser(description="loomos sherpa-onnx 适配器驱动")
    sub = ap.add_subparsers(dest="cmd", required=True)

    a = sub.add_parser("asr")
    a.add_argument("--model-dir", required=True)
    a.add_argument("--wav", required=True)
    a.add_argument("--num-threads", type=int, default=1)
    a.set_defaults(fn=cmd_asr)

    t = sub.add_parser("tts")
    t.add_argument("--model-dir", required=True)
    t.add_argument("--text", required=True)
    t.add_argument("--out", required=True)
    t.add_argument("--speed", type=float, default=1.0)
    t.add_argument("--num-threads", type=int, default=2)
    t.set_defaults(fn=cmd_tts)

    args = ap.parse_args()
    try:
        args.fn(args)
    except Exception as e:  # noqa: BLE001 —— 单行 JSON 协议：失败必须走 stderr + 非零退出
        print(f"sherpa_driver 失败: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
