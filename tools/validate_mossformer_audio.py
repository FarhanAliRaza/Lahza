#!/usr/bin/env python3
"""Independent torchaudio/ONNX reference for Lahza's native MossFormer2 frontend.

Requires numpy, torch, torchaudio, onnxruntime, scipy and soundfile. Supply a
directory containing *-noisy.wav and corresponding *-reference.wav fixtures
prepared by validate_neural_audio.py. No audio is downloaded or uploaded.

Normal validation writes reference outputs and metrics, never the tracked Kaldi
golden. Regenerating that golden requires the explicit --golden-only flag.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import time

ROOT = Path(__file__).resolve().parents[1]
RATE = 48_000
WINDOW = RATE * 4
OVERLAP = RATE
STRIDE = WINDOW - OVERLAP
FRAME = 1920
HOP = 384


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def features(audio):
    import torch
    import torchaudio

    fbanks = torchaudio.compliance.kaldi.fbank(
        torch.from_numpy(audio).unsqueeze(0),
        dither=0.0,
        frame_length=40.0,
        frame_shift=8.0,
        num_mel_bins=60,
        sample_frequency=float(RATE),
        window_type="hamming",
    )
    first = torchaudio.functional.compute_deltas(fbanks.T).T
    second = torchaudio.functional.compute_deltas(first.T).T
    return torch.cat([fbanks, first, second], dim=1).numpy()


def generate_golden(destination):
    import numpy as np

    state = 1
    audio = np.empty(WINDOW, dtype=np.float32)
    for index in range(WINDOW):
        state = (state * 1664525 + 1013904223) & 0xFFFFFFFF
        audio[index] = np.float32(np.float32((state >> 8) / 16777216.0) - 0.5) * np.float32(4000.0)
    selected = features(audio)[[0, 1, 247, 494, 495]]
    destination.parent.mkdir(parents=True, exist_ok=True)
    selected.astype("<f4").tofile(destination)
    print(json.dumps({"golden": str(destination), "rows": 5, "columns": 180,
                      "sha256": sha256(destination)}, indent=2))


def process(audio, session, window):
    import numpy as np
    import torch

    power = audio.astype(np.float64) ** 2
    above = power[power > power.mean()]
    rms = np.sqrt(above.mean() if len(above) else power.mean())
    gain = np.float32(np.clip(10 ** (-25 / 20) / (rms + 1e-6), 0.001, 1000.0))
    scale = np.float32(gain * np.float32(32768.0))
    result = np.zeros(len(audio), np.float32)
    tail = None
    for start in range(0, len(audio), STRIDE):
        count = min(WINDOW, len(audio) - start)
        chunk = np.zeros(WINDOW, np.float32)
        chunk[:count] = audio[start:start + count]
        chunk *= scale
        if np.all(np.abs(audio[start:start + count]) < 1e-9):
            enhanced = np.zeros(count, np.float32)
        else:
            mask = session.run(None, {"fbanks": features(chunk)[None]})[0][0]
            spectrum = torch.stft(torch.from_numpy(chunk), FRAME, HOP, FRAME,
                                  window, center=False, return_complex=True)
            enhanced = torch.istft(spectrum * torch.from_numpy(mask).T, FRAME, HOP,
                                   FRAME, window, center=False, length=WINDOW).numpy() / scale
            enhanced = enhanced[:count]
        if tail is not None:
            overlap = min(OVERLAP, count)
            blend = np.arange(overlap, dtype=np.float32) / np.float32(OVERLAP)
            enhanced[:overlap] = tail[:overlap] * (1 - blend) + enhanced[:overlap] * blend
        last = start + count == len(audio)
        emit = count if last else STRIDE
        result[start:start + emit] = enhanced[:emit]
        if last:
            break
        tail = enhanced[STRIDE:].copy()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture_directory", nargs="?", type=Path)
    parser.add_argument("--output-directory", "--output-dir", type=Path,
                        help="default: the supplied fixture directory")
    parser.add_argument("--model", type=Path,
                        default=ROOT / "assets/audio/mossformer2-se-48k-int8.onnx")
    parser.add_argument("--threads", type=int, default=2)
    parser.add_argument("--golden-only", action="store_true",
                        help="explicitly regenerate the Kaldi feature golden; do not load a model")
    parser.add_argument("--golden-output", type=Path,
                        help="golden destination (requires --golden-only; default: assets/audio/fbank-reference.f32)")
    args = parser.parse_args()
    if args.threads < 1:
        parser.error("--threads must be positive")
    if args.golden_output is not None and not args.golden_only:
        parser.error("--golden-output requires --golden-only")
    if not args.golden_only and args.fixture_directory is None:
        parser.error("fixture_directory is required unless --golden-only is used")

    # Set numerical-library limits before importing them. The tool is a
    # developer validation job and must not start a full-machine thread pool.
    for variable in ("OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS"):
        os.environ[variable] = str(args.threads)
    import numpy as np
    import torch
    import torchaudio

    torch.set_num_threads(args.threads)
    torch.set_num_interop_threads(1)
    if args.golden_only:
        generate_golden(args.golden_output or ROOT / "assets/audio/fbank-reference.f32")
        return

    import onnxruntime as ort
    import soundfile as sf
    from validate_neural_audio import evaluate

    fixtures = sorted(args.fixture_directory.glob("*-noisy.wav"))
    if not fixtures:
        parser.error("fixture_directory contains no *-noisy.wav fixtures")
    destination = args.output_directory or args.fixture_directory
    destination.mkdir(parents=True, exist_ok=True)
    options = ort.SessionOptions()
    options.intra_op_num_threads = args.threads
    options.inter_op_num_threads = 1
    began = time.monotonic()
    session = ort.InferenceSession(str(args.model), options, providers=["CPUExecutionProvider"])
    load_seconds = time.monotonic() - began
    window = torch.hamming_window(FRAME, periodic=False)
    rows = []
    with torch.inference_mode():
        for fixture in fixtures:
            stem = fixture.name.removesuffix("-noisy.wav")
            reference_path = args.fixture_directory / f"{stem}-reference.wav"
            noisy, rate = sf.read(fixture, dtype="float32")
            clean, clean_rate = sf.read(reference_path, dtype="float32")
            if rate != RATE or clean_rate != RATE or noisy.ndim != 1 or clean.ndim != 1:
                raise ValueError(f"{stem}: both fixtures must be mono 48 kHz audio")
            if not len(noisy) or len(noisy) != len(clean):
                raise ValueError(f"{stem}: fixtures must have equal nonzero lengths")
            if not np.all(np.isfinite(noisy)) or not np.all(np.isfinite(clean)):
                raise ValueError(f"{stem}: fixtures contain non-finite samples")
            began = time.monotonic()
            enhanced = process(noisy, session, window)
            elapsed = time.monotonic() - began
            sf.write(destination / f"{stem}-moss-python.wav", enhanced, RATE, subtype="FLOAT")
            noisy.astype("<f4").tofile(destination / f"{stem}-noisy.f32")
            enhanced.astype("<f4").tofile(destination / f"{stem}-moss-python.f32")
            row = evaluate(clean, noisy, enhanced, RATE)
            row.update(sample=stem, seconds=elapsed, rtf=elapsed / (len(noisy) / RATE),
                       noisy_sha256=sha256(fixture), reference_sha256=sha256(reference_path))
            rows.append(row)
            print(json.dumps(row), flush=True)
    (destination / "moss-python-metrics.json").write_text(json.dumps(rows, indent=2) + "\n")
    metadata = {
        "model": str(args.model), "model_sha256": sha256(args.model),
        "model_load_seconds": load_seconds, "threads": args.threads,
        "torch": torch.__version__, "torchaudio": torchaudio.__version__,
        "onnxruntime": ort.__version__, "numpy": np.__version__,
        "sample_rate": RATE, "window_samples": WINDOW, "overlap_samples": OVERLAP,
        "dither": 0.0, "golden_regenerated": False,
    }
    (destination / "moss-python-run.json").write_text(json.dumps(metadata, indent=2) + "\n")


if __name__ == "__main__":
    main()
