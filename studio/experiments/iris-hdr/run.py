#!/usr/bin/env python3
"""HDR test case on the Iris Nebula (NGC 7023), driven end to end.

Builds a PSF Studio store from LRGB masters with Seiza's Gaia colour
calibration, opens it in Studio through psf-studio-mcp, chooses
BlurXTerminator and NoiseXTerminator for every input, makes an LRGB picture
with RGB stars (which separates the stars with StarXTerminator and runs the
tools), stretches every input with the HDR layered stretch, and checks:

- the colours were calibrated against Gaia;
- the RGB stars part shows the stars StarXTerminator took out;
- the Iris's core rises above SDR white and keeps its detail, with almost
  nothing at the picture's peak, with the stars shown and hidden.

It writes report.md, report.json and PNGs of the final picture and the core
to the output folder, and exits non-zero when a check fails. The masters are
read where they are and never changed.

    python3 run.py --masters <folder of masterLight_*_FILTER-{L,R,G,B}_*.xisf> \\
        --out <scratch folder> [--psf-guard ...] [--mcp ...]

Needs the Gaia photometry set where Seiza looks (Settings › Catalogs ›
Gaia colors, or `seiza download-data prebuilt --file
stars-gaia-photometry.bin`), RC-Astro's tools, and release builds of
psf-guard, psf-studio and psf-studio-mcp.
"""

import argparse
import base64
import glob
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPOSITORY = os.path.abspath(os.path.join(HERE, "..", "..", ".."))

# The Iris at the reference frame's pointing: right ascension and declination
# of the frame's centre in degrees, and the pixel scale in arcseconds.
POINTING = "315.2605,67.9221,1.1835"
CHANNELS = {"luminance": "L", "red": "R", "green": "G", "blue": "B"}
# The HDR layered stretch every input gets: the core compressed, the curve
# continued into the picture's headroom.
STRETCH = {
    "type": "layered-hdr",
    "target_background": 0.12,
    "aggressiveness": 0.7,
    "contrast_recovery": 0.0,
    "knee": 0.75,
    "core_compression": 0.9,
    "core_threshold": 100.0,
    "core_scale": 8.0,
    "sky_floor": 0.0,
    "headroom": 2.0,
}
HEADROOM = 2.0


class Mcp:
    """A psf-studio-mcp server over standard input and output."""

    def __init__(self, program):
        self.process = subprocess.Popen(
            [program], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
        )
        self.next = 0
        self.call("initialize", {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "iris-hdr", "version": "1"},
        })

    def call(self, method, params):
        self.next += 1
        message = {"jsonrpc": "2.0", "id": self.next, "method": method, "params": params}
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()
        return json.loads(self.process.stdout.readline())

    def tool(self, name, arguments, image=None):
        result = self.call("tools/call", {"name": name, "arguments": arguments})["result"]
        texts = [part["text"] for part in result["content"] if part["type"] == "text"]
        if result.get("isError"):
            raise RuntimeError(f"{name}: {texts}")
        for part in result["content"]:
            if part["type"] == "image" and image:
                with open(image, "wb") as file:
                    file.write(base64.b64decode(part["data"]))
        try:
            return json.loads(texts[0])
        except (IndexError, ValueError):
            return texts

    def close(self):
        self.process.stdin.close()
        self.process.wait()


def master(folder, letter):
    found = sorted(glob.glob(os.path.join(folder, f"masterLight_*_FILTER-{letter}_mono_autocrop.xisf")))
    if not found:
        found = sorted(glob.glob(os.path.join(folder, f"masterLight_*_FILTER-{letter}_mono.xisf")))
    if not found:
        sys.exit(f"no {letter} master in {folder}")
    return found[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--masters", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--psf-guard", default=os.path.join(REPOSITORY, "target/release/psf-guard"))
    parser.add_argument("--mcp", default=os.path.join(REPOSITORY, "studio/target/release/psf-studio-mcp"))
    parser.add_argument("--reuse-store", action="store_true", help="keep a store built before")
    arguments = parser.parse_args()
    os.makedirs(arguments.out, exist_ok=True)
    store = os.path.join(arguments.out, "iris.pgtiles")
    document = os.path.join(arguments.out, "iris.json")
    checks = []

    def check(name, passed, detail):
        checks.append({"check": name, "passed": bool(passed), "detail": detail})
        print(("PASS " if passed else "FAIL ") + name + ": " + json.dumps(detail))

    # 1. The store, registered to the luminance master and colour calibrated.
    if not (arguments.reuse_store and os.path.exists(store)):
        command = [arguments.psf_guard, "studio-files"]
        for channel, letter in CHANNELS.items():
            command += ["--channel", f"{channel}={master(arguments.masters, letter)}"]
        command += ["--pointing", POINTING, "--document", document, "--output", store]
        print("building the store:", " ".join(command[:2]), "…")
        subprocess.run(command, check=True)
    with open(store.replace(".pgtiles", ".prepared.json")) as file:
        calibration = json.load(file).get("calibration", {})
    check("colours calibrated against Gaia", calibration.get("status") == "done", calibration)

    # The Iris in the reference frame, from the plate solve.
    with open(store.replace(".pgtiles", ".sources") + "/reference-solve.json") as file:
        solution = json.load(file)["solution"]
    iris = next(o for o in solution["objects"] if o["name"] == "NGC 7023")
    core = {"x": int(iris["x"]) - 128, "y": int(iris["y"]) - 128, "width": 256, "height": 256}

    mcp = Mcp(arguments.mcp)
    pid = mcp.tool("studio_open", {"store": store, "document": document, "copy": False})["pid"]
    try:
        for channel in CHANNELS:
            mcp.tool("studio_set_tool", {"instance": pid, "input": channel, "category": "sharpen", "provider": "bxt"})
            mcp.tool("studio_set_tool", {"instance": pid, "input": channel, "category": "denoise", "provider": "nxt"})
        mcp.tool("studio_use_recipe", {"instance": pid, "name": "LRGB"})
        # RGB stars separate the stars, which runs every input's tools.
        mcp.tool("studio_add_part", {"instance": pid, "kind": "RGB stars"})
        waited = mcp.tool("studio_wait_idle", {"instance": pid, "timeout_seconds": 3600})
        check("the tools ran and the canvas caught up", waited.get("idle"), waited)

        state = mcp.tool("studio_state", {"instance": pid})
        check("no input's tools are left to run", not state["tools_not_run"], state["tools_not_run"])
        stars = [layer for layer in state["layers"] if layer["name"] == "RGB stars"]
        check("the picture has an RGB stars part", stars, [layer["name"] for layer in state["layers"]])
        with open(os.path.join(arguments.out, "iris.pgtiles"), "rb") as file:
            header = file.read(65536).decode("latin-1")
        separated = [f"{channel}@stars" for channel in ("red", "green", "blue")]
        check("the store holds the stars of red, green and blue", all(name in header for name in separated), separated)

        # 2. Every input's stretch the HDR layered one, and the picture HDR.
        for item in state["inputs"]:
            for step in item["steps"]:
                if step["name"] == "Stretch" and step["band"] in CHANNELS:
                    mcp.tool("studio_set_step", {"instance": pid, "step": step["row"]["input_step"]["step"], "settings": {"model": STRETCH}})
        mcp.tool("studio_set_headroom", {"instance": pid, "stops": HEADROOM})
        mcp.tool("studio_select", {"instance": pid, "row": "final"})
        mcp.tool("studio_set_split", {"instance": pid, "split": "one"})
        mcp.tool("studio_wait_idle", {"instance": pid, "timeout_seconds": 600})

        # 3. The core: above SDR white, with detail, and not at the peak.
        shown = mcp.tool("studio_sample", dict(core, instance=pid, divide=1.8), image=os.path.join(arguments.out, "core-with-stars.png"))
        peak = shown["peak"]
        brightest = max(channel["p99"] for channel in shown["channels"].values())
        at_peak = max(channel["at_peak"] for channel in shown["channels"].values())
        above = max(channel["above_sdr_white"] for channel in shown["channels"].values())
        check("the core rises above SDR white", above > 0.01, {"share_above_sdr_white": above, "p99": brightest, "peak": peak})
        check("almost none of the core is at the peak", at_peak < 0.01, {"share_at_peak": at_peak})
        mcp.tool("studio_set_layer", {"instance": pid, "layer": stars[0]["row"]["layer"], "patch": {"visible": False}})
        mcp.tool("studio_wait_idle", {"instance": pid, "timeout_seconds": 600})
        hidden = mcp.tool("studio_sample", dict(core, instance=pid, divide=1.8), image=os.path.join(arguments.out, "core-without-stars.png"))
        mcp.tool("studio_set_layer", {"instance": pid, "layer": stars[0]["row"]["layer"], "patch": {"visible": True}})
        # Screened stars brighten, and never flatten the core: its median
        # with the stars is no lower than without them.
        kept = all(
            shown["channels"][name]["median"] >= hidden["channels"][name]["median"] - 1e-3
            for name in ("red", "green", "blue")
        )
        check("the stars leave the core's detail", kept, {
            "median_with_stars": {k: v["median"] for k, v in shown["channels"].items()},
            "median_without_stars": {k: v["median"] for k, v in hidden["channels"].items()},
        })
        mcp.tool("studio_wait_idle", {"instance": pid, "timeout_seconds": 600})
        mcp.tool("studio_capture", {"instance": pid, "pane": 0, "divide": 1.0, "max_width": 1400}, image=os.path.join(arguments.out, "final.png"))
        mcp.tool("studio_capture", {"instance": pid, "pane": 0, "divide": peak, "max_width": 1400}, image=os.path.join(arguments.out, "final-divided-by-peak.png"))
    finally:
        mcp.tool("studio_close", {"instance": pid})
        mcp.close()

    passed = all(item["passed"] for item in checks)
    report = {"passed": passed, "checks": checks, "core_region": core, "calibration": calibration}
    with open(os.path.join(arguments.out, "report.json"), "w") as file:
        json.dump(report, file, indent=2)
    with open(os.path.join(arguments.out, "report.md"), "w") as file:
        file.write("# Iris HDR test case\n\n")
        for item in checks:
            file.write(f"- {'PASS' if item['passed'] else 'FAIL'} {item['check']}: `{json.dumps(item['detail'])}`\n")
        file.write("\nImages: final.png, final-divided-by-peak.png, core-with-stars.png, core-without-stars.png\n")
    print("report:", os.path.join(arguments.out, "report.md"))
    sys.exit(0 if passed else 1)


if __name__ == "__main__":
    main()
