# Model Weights

Model weight files are not checked into git. Use Git LFS or the fetch script below.

## Fetch script (TODO)

```bash
./scripts/fetch-models.sh
```

## Model inventory

| File | Format | Est. size | Runtime | Used by |
|------|--------|-----------|---------|---------|
| `yolo-nano-int8.rknn` | RKNN | ~2MB | RKNN SDK (NPU) | ai-daemon — vision |
| `whisper-tiny.rknn` | RKNN | ~40MB | RKNN SDK (NPU) | ai-daemon — STT |
| `intent-classifier.onnx` | ONNX | <1MB | ONNX Runtime (CPU) | ai-daemon — intent |
| `kws-driving.rknn` | RKNN | <1MB | RKNN SDK (NPU) | ai-daemon — KWS driving |
| `kws-parked.bin` | Binary | <100KB | MCU runtime | MCU kws_task |
| `piper-tts.onnx` | ONNX | ~60MB | ONNX Runtime (CPU) | voice-daemon — TTS |

## Conversion notes

RKNN models are compiled for the RV1106 NPU target using Rockchip RKNN Toolkit 2.
Source models (PyTorch / HuggingFace) must be exported to ONNX first, then converted
with `rknn-toolkit2` targeting `rk1106`.

ONNX Runtime should be built as a minimal CPU-only build to reduce rootfs footprint.
