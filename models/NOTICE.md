# Third-party notices

## wulu-geetest-bypass

- Source: https://github.com/wulu007/geetest-bypass
- License: MIT
- Use: GeeTest v4 wire protocol, PoW, request signing, and human-like click tracks.

## yzm_captcha

- Source: https://github.com/zscmmm/yzm_captcha
- Model repository: https://huggingface.co/zscmmm/yzm
- License: repository MIT; Hugging Face model card Apache-2.0
- `models/detect.onnx` is derived from the `icon4mi800/detect.pt` model,
exported to ONNX at 320×320, and dynamically quantized to INT8.
- `models/detect_icon3.onnx` and `models/simvgg19.onnx` are derived from the
  `icon4_3new` package published by the same model author through the
  `zscmmm/yzm` model repository. They were exported to ONNX and dynamically
  quantized to INT8.
- The exported INT8 models are numerically equivalent for the inference path used here.

## GeekedTest

- Source: https://github.com/xKiian/GeekedTest
- License: MIT
- Use: engineering reference for GeeTest v4 icon challenge handling. No GeekedTest
  model file is bundled in this directory.

The model names and hashes are not intended to imply endorsement by the upstream
authors. This project is not affiliated with GeeTest or Bilibili.
