#!/usr/bin/env python3
"""Measure TF-Restormer weights and native parity on an explicit public fixture.

Run separately from application builds. Needs numpy/scipy/soundfile/onnxruntime.
No model downloads, recording access or network requests are performed.
"""
from __future__ import annotations

import argparse
import gc
import json
from pathlib import Path
import time

import numpy as np
import onnxruntime as ort
import soundfile as sf

from validate_neural_audio import evaluate

RATE = 48000
FRAME = 1920
HOP = 960
WINDOW = RATE
OVERLAP = RATE // 5
STRIDE = WINDOW - OVERLAP
WEIGHTS = np.sqrt(0.5 - 0.5 * np.cos(np.arange(FRAME) * 2 * np.pi / FRAME))


def analyze(audio):
    padded = np.pad(audio, (HOP, HOP + (-len(audio)) % HOP))
    frames = np.lib.stride_tricks.sliding_window_view(padded, FRAME)[::HOP]
    spectrum = (np.fft.rfft(frames * WEIGHTS, axis=1) / np.sqrt(HOP)).T
    profile = np.mean(abs(spectrum), axis=1)
    active = np.flatnonzero(profile > np.max(profile) * 0.001)
    frequencies = min(960, max(161, int(active[-1] + 1) if len(active) else 161))
    return np.stack([spectrum[:frequencies].real, spectrum[:frequencies].imag], axis=-1)[None].astype(np.float32)


def synthesize(result, length):
    spectrum = result[0, :, :, 0] + 1j * result[0, :, :, 1]
    spectrum[[0, -1]] *= 2
    frames = np.fft.irfft(spectrum.T, n=FRAME, axis=1) * (FRAME / (2 * np.sqrt(HOP))) * WEIGHTS
    output = np.zeros((len(frames) - 1) * HOP + FRAME)
    for index, frame in enumerate(frames):
        output[index * HOP:index * HOP + FRAME] += frame
    return output[HOP:HOP + length].astype(np.float32)


def process(model, audio, threads, window=WINDOW, overlap=OVERLAP, no_arena=False):
    stride = window - overlap
    options = ort.SessionOptions()
    options.intra_op_num_threads = threads
    options.inter_op_num_threads = 1
    if no_arena:
        options.enable_cpu_mem_arena = False
        options.enable_mem_pattern = False
    started = time.perf_counter()
    session = ort.InferenceSession(str(model), sess_options=options, providers=["CPUExecutionProvider"])
    load_seconds = time.perf_counter() - started
    started = time.perf_counter()
    result = np.zeros(len(audio), dtype=np.float32)
    tail = None
    shapes = []
    for start in range(0, len(audio), stride):
        chunk = audio[start:start + window]
        if np.all(np.abs(chunk) < 1e-9):
            restored = np.zeros_like(chunk)
        else:
            features = analyze(chunk)
            shapes.append(list(features.shape))
            spectrum = session.run(None, {"spectrum": features})[0]
            expected_shape = (1, 961, features.shape[2], 2)
            if spectrum.shape != expected_shape or not np.isfinite(spectrum).all():
                raise ValueError(f"Invalid model output {spectrum.shape}; expected {expected_shape}")
            restored = synthesize(spectrum, len(chunk))
        if tail is not None:
            count = min(overlap, len(chunk))
            blend = np.arange(count, dtype=np.float32) / overlap
            restored[:count] = tail[:count] * (1 - blend) + restored[:count] * blend
        last = start + len(chunk) == len(audio)
        emit = len(chunk) if last else stride
        result[start:start + emit] = restored[:emit]
        if last:
            break
        tail = restored[stride:].copy()
    elapsed = time.perf_counter() - started
    del session
    gc.collect()
    return result, {"load_seconds": load_seconds, "inference_seconds": elapsed,
                    "realtime_factor": elapsed / (len(audio) / RATE), "input_shapes": shapes}


def difference(reference, actual):
    if len(reference) != len(actual):
        raise ValueError(f"Sample count mismatch: {len(reference)} vs {len(actual)}")
    error = actual.astype(np.float64) - reference
    rms = float(np.sqrt(np.mean(reference.astype(np.float64) ** 2)))
    rmse = float(np.sqrt(np.mean(error ** 2)))
    return {"max_abs_difference": float(np.max(np.abs(error))), "rmse": rmse,
            "error_to_signal_db": float(20 * np.log10(max(rmse, 1e-20) / max(rms, 1e-20)))}


def high_band_ratio(audio):
    frequencies = np.fft.rfftfreq(len(audio), 1 / RATE)
    power = abs(np.fft.rfft(audio)) ** 2
    high = power[(frequencies >= 8000) & (frequencies < 20000)].sum()
    voice = power[(frequencies >= 300) & (frequencies < 3400)].sum()
    return float(10 * np.log10(max(high, 1e-20) / max(voice, 1e-20)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--reference", type=Path, required=True)
    parser.add_argument("--model", type=Path, default=Path("assets/audio/tf-restormer-weights-f16.onnx"))
    parser.add_argument("--fp32-model", type=Path)
    parser.add_argument("--native-output", type=Path, help="Mono f32le from restoration_audio_file_probe")
    parser.add_argument("--compare-only", action="store_true", help="Reuse this directory's prior Python output; no inference")
    parser.add_argument("--output", type=Path, required=True, help="Directory for WAV/PCM and JSON evidence")
    parser.add_argument("--threads", type=int, default=2)
    parser.add_argument("--limit-seconds", type=float)
    parser.add_argument("--window-seconds", type=float, default=1.0)
    parser.add_argument("--overlap-seconds", type=float, default=0.2)
    parser.add_argument("--no-arena", action="store_true", help="Disable ORT arena and memory pattern for memory measurements")
    args = parser.parse_args()
    audio, rate = sf.read(args.input, dtype="float32")
    reference, reference_rate = sf.read(args.reference, dtype="float32")
    if rate != RATE or reference_rate != RATE or audio.ndim != 1 or reference.ndim != 1:
        raise ValueError("Use mono 48kHz fixtures; resample explicitly before measurement")
    if len(audio) != len(reference):
        raise ValueError("Reference and input must have equal sample counts")
    if args.limit_seconds is not None:
        count = max(1, int(args.limit_seconds * RATE))
        audio, reference = audio[:count], reference[:count]
    window = int(args.window_seconds * RATE)
    overlap = int(args.overlap_seconds * RATE)
    if window < FRAME or not 0 < overlap < window:
        parser.error("Window must be at least 40ms and overlap strictly between zero and window")
    if not len(audio) or not np.isfinite(audio).all() or not np.isfinite(reference).all():
        raise ValueError("Fixtures must contain finite, nonempty PCM")
    args.output.mkdir(parents=True, exist_ok=True)
    if args.compare_only:
        if args.native_output is None:
            parser.error("--compare-only requires --native-output")
        saved_input = np.fromfile(args.output / "input.f32", dtype="<f4")
        if not np.array_equal(saved_input, audio):
            raise ValueError("Saved reference run used a different input or duration")
        restored = np.fromfile(args.output / "restored-python.f32", dtype="<f4")
        native = np.fromfile(args.native_output, dtype="<f4")
        report = json.loads((args.output / "metrics.json").read_text())
        report["native_vs_python"] = difference(restored, native)
        report["native_metrics"] = evaluate(reference, audio, native, RATE)
        serialized = json.dumps(report, indent=2) + "\n"
        (args.output / "metrics.json").write_text(serialized)
        print(serialized, end="", flush=True)
        return
    audio.astype("<f4").tofile(args.output / "input.f32")
    full = None
    report = {"input": str(args.input), "reference": str(args.reference), "samples": len(audio),
              "window_samples": window, "overlap_samples": overlap, "cpu_arena": not args.no_arena}
    if args.fp32_model:
        full, timing = process(args.fp32_model, audio, max(1, args.threads), window, overlap, args.no_arena)
        report["fp32"] = timing
    restored, timing = process(args.model, audio, max(1, args.threads), window, overlap, args.no_arena)
    report["packed_weights"] = timing
    report["metrics"] = evaluate(reference, audio, restored, RATE)
    report["high_to_voice_band_db"] = {"input": high_band_ratio(audio),
                                       "reference": high_band_ratio(reference),
                                       "restored": high_band_ratio(restored)}
    if full is not None:
        report["packed_vs_fp32"] = difference(full, restored)
    if args.native_output:
        native = np.fromfile(args.native_output, dtype="<f4")
        report["native_vs_python"] = difference(restored, native)
        report["native_metrics"] = evaluate(reference, audio, native, RATE)
    restored.astype("<f4").tofile(args.output / "restored-python.f32")
    sf.write(args.output / "restored-python.wav", restored, RATE, subtype="FLOAT")
    serialized = json.dumps(report, indent=2) + "\n"
    (args.output / "metrics.json").write_text(serialized)
    print(serialized, end="", flush=True)


if __name__ == "__main__":
    main()
