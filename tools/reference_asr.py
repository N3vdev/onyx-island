"""Reference Parakeet TDT v3 decode used to validate the Rust port.

This mirrors istupakov/onnx-asr's NeMo TDT greedy loop and runs the same model
files. It is a development aid, not shipped code. Requires numpy + onnxruntime.
"""

from __future__ import annotations

import argparse
import json
import re
import time
import wave
from pathlib import Path

import numpy as np
import onnxruntime as rt

SAMPLE_RATE = 16_000
BLANK_ID = 8192
VOCAB_SIZE = 8193
MAX_TOKENS_PER_STEP = 10


def read_wav(path: Path) -> np.ndarray:
    with wave.open(str(path), "rb") as wav:
        assert wav.getnchannels() == 1, "mono only"
        assert wav.getsampwidth() == 2, "16-bit only"
        rate = wav.getframerate()
        samples = np.frombuffer(wav.readframes(wav.getnframes()), dtype="<i2")
    audio = samples.astype(np.float32) / 32768.0
    if rate != SAMPLE_RATE:
        # Linear resample; good enough for the SAPI fixtures.
        duration = audio.shape[0] / rate
        target_len = int(round(duration * SAMPLE_RATE))
        source_x = np.linspace(0.0, duration, num=audio.shape[0], endpoint=False)
        target_x = np.linspace(0.0, duration, num=target_len, endpoint=False)
        audio = np.interp(target_x, source_x, audio).astype(np.float32)
    return audio


def load_vocab(path: Path) -> list[str]:
    vocab = [""] * VOCAB_SIZE
    with path.open("rt", encoding="utf-8") as handle:
        for line in handle:
            token, index = line.rstrip("\n").rsplit(" ", 1)
            vocab[int(index)] = token.replace("\u2581", " ")
    return vocab


def normalise(text: str) -> str:
    text = text.lower()
    text = re.sub(r"[^a-z0-9' ]+", " ", text)
    return " ".join(text.split())


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--models", type=Path, default=Path.home() / "AppData/Local/Islet/models/parakeet-tdt-0.6b-v3-int8")
    parser.add_argument("--wav", type=Path, required=True)
    parser.add_argument("--expected", default="")
    args = parser.parse_args()

    started = time.perf_counter()
    load_started = time.perf_counter()
    pre = rt.InferenceSession(str(args.models / "nemo128.onnx"), providers=["CPUExecutionProvider"])
    encoder = rt.InferenceSession(str(args.models / "encoder-model.int8.onnx"), providers=["CPUExecutionProvider"])
    decoder = rt.InferenceSession(str(args.models / "decoder_joint-model.int8.onnx"), providers=["CPUExecutionProvider"])
    load_seconds = time.perf_counter() - load_started

    vocab = load_vocab(args.models / "vocab.txt")
    audio = read_wav(args.wav)

    infer_started = time.perf_counter()
    features, feature_lengths = pre.run(
        ["features", "features_lens"],
        {
            "waveforms": audio[None, :],
            "waveforms_lens": np.array([audio.shape[0]], dtype=np.int64),
        },
    )
    enc, enc_lengths = encoder.run(
        ["outputs", "encoded_lengths"],
        {
            "audio_signal": features,
            "length": feature_lengths,
        },
    )
    # Encoder exports [B, hidden, frames]; the TDT joint consumes one frame
    # column at a time as [B, hidden, 1].

    state = (
        np.zeros((2, 1, 640), dtype=np.float32),
        np.zeros((2, 1, 640), dtype=np.float32),
    )
    tokens: list[int] = []
    timestamps: list[int] = []
    t = 0
    emitted = 0
    enc_len = int(enc_lengths[0])
    while t < enc_len:
        logits, _, next_state_1, next_state_2 = decoder.run(
            ["outputs", "prednet_lengths", "output_states_1", "output_states_2"],
            {
                "encoder_outputs": enc[:, :, t : t + 1],
                "targets": np.array([[tokens[-1] if tokens else BLANK_ID]], dtype=np.int32),
                "target_length": np.array([1], dtype=np.int32),
                "input_states_1": state[0],
                "input_states_2": state[1],
            },
        )
        logits = logits.squeeze()
        token = int(np.argmax(logits[:VOCAB_SIZE]))
        step = int(np.argmax(logits[VOCAB_SIZE:]))
        if token != BLANK_ID:
            state = (next_state_1, next_state_2)
            tokens.append(token)
            timestamps.append(t)
            emitted += 1
        if step > 0:
            t += step
            emitted = 0
        elif token == BLANK_ID or emitted == MAX_TOKENS_PER_STEP:
            t += 1
            emitted = 0

    text = "".join(vocab[token] for token in tokens).strip()
    text = re.sub(r"\s+", " ", text)
    seconds = audio.shape[0] / SAMPLE_RATE
    inference_seconds = time.perf_counter() - infer_started
    total_seconds = time.perf_counter() - started

    print(json.dumps({
        "text": text,
        "audio_seconds": round(seconds, 3),
        "load_seconds": round(load_seconds, 3),
        "inference_seconds": round(inference_seconds, 3),
        "total_seconds": round(total_seconds, 3),
        "rtf": round(seconds / inference_seconds, 2),
        "frames": enc_len,
        "tokens": len(tokens),
        "timestamps": timestamps[:20],
        "expected": args.expected,
    }, indent=2))


if __name__ == "__main__":
    main()
