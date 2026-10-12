#!/usr/bin/env python3
"""Export the IAT low-light enhancer to ONNX and install it as the
`iat_lowlight` model (used by Studio Light's "Auto light", NPU only).

IAT (Illumination Adaptive Transformer, BMVC 2022, Apache-2.0, 91k params):
https://github.com/cuiziteng/Illumination-Adaptive-Transformer

The ONNX is generated here rather than downloaded, so the manifest in
data/models-experimental/iat_lowlight records the sha256 of the export from the
pinned commit and torch/timm versions below; a different toolchain may produce
a different hash. Run with:

  uv run --python 3.13 --with torch --with torchvision --with timm --with onnx \\
      --with onnxscript --with numpy scripts/export-iat.py

(torch/torchvision from the CPU index if the default wheels are too big.)
"""
import hashlib
import os
import shutil
import sys
import tempfile
import urllib.request
import warnings

warnings.filterwarnings("ignore")

COMMIT = "c76472265247f47cea57649af28b15018bb64cb1"
BASE = f"https://raw.githubusercontent.com/cuiziteng/Illumination-Adaptive-Transformer/{COMMIT}/IAT_enhance"
FILES = ["model/IAT_main.py", "model/blocks.py", "model/global_net.py", "best_Epoch_lol_v1.pth"]
PTH_SHA256 = "6fb32236152283d3f3633ea1b79601c7e819efbf9d87d6b4767e1db0f1f3435b"

models_dir = os.environ.get(
    "VINOOPENEFFECTS_MODELS_DEST",
    os.path.join(os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share")), "vinoopeneffects", "models"),
)
dest = os.path.join(models_dir, "iat_lowlight")
here = os.path.dirname(os.path.abspath(__file__))
manifest = os.path.join(here, "..", "data", "models-experimental", "iat_lowlight", "manifest.toml")

with tempfile.TemporaryDirectory() as work:
    os.makedirs(f"{work}/src/model")
    # `model` imports the stdlib `imp`, removed in 3.12; an empty shim is enough.
    os.makedirs(f"{work}/shim")
    open(f"{work}/shim/imp.py", "w").close()
    for f in FILES:
        urllib.request.urlretrieve(f"{BASE}/{f}", f"{work}/src/{f}")
    got = hashlib.sha256(open(f"{work}/src/best_Epoch_lol_v1.pth", "rb").read()).hexdigest()
    if got != PTH_SHA256:
        sys.exit(f"checkpoint sha256 mismatch: {got}")

    sys.path[:0] = [f"{work}/shim", f"{work}/src"]
    import torch
    from model.IAT_main import IAT

    net = IAT(type="lol")
    # weights_only: the checkpoint is a pickle, never load it as arbitrary code.
    net.load_state_dict(torch.load(f"{work}/src/best_Epoch_lol_v1.pth", map_location="cpu", weights_only=True))
    net.eval()

    class Enhance(torch.nn.Module):
        def __init__(self, m):
            super().__init__()
            self.m = m

        def forward(self, x):
            return self.m(x)[2]

    os.makedirs(dest, exist_ok=True)
    out = os.path.join(dest, "iat_lol_v1_256.onnx")
    torch.onnx.export(Enhance(net).eval(), (torch.rand(1, 3, 256, 256),), out,
                      input_names=["input"], output_names=["output"], opset_version=17, dynamo=False)

shutil.copy(manifest, os.path.join(dest, "manifest.toml"))
sha = hashlib.sha256(open(out, "rb").read()).hexdigest()
print(f"exported {out}\nsha256 {sha}")
want = [l for l in open(manifest) if l.startswith("sha256")][0].split('"')[1]
if sha != want:
    print(f"warning: manifest expects {want}; this toolchain's export differs, so the model "
          "will not resolve until the manifest sha256 is updated")
