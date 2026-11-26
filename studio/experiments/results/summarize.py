import json,sys
for f in sys.argv[1:]:
    d=json.load(open(f))
    if "error" in d: print(f, d); continue
    print(f.split('/')[-1], d["surface"].get("user_agent","native")[-60:], "| tile ms", round(d["mean_tile_compute_ms"],1))
    g=lambda p,k,q: (p.get(k) or {}).get(q)
    for p in d["phases"]:
        o=p.get("gpu_offscreen_ms_per_frame")
        print(f'  {p["phase"]:26} n={p["frames"]:4} int p50/p95/p99/max={g(p,"frame_interval_ms","p50")}/{g(p,"frame_interval_ms","p95")}/{g(p,"frame_interval_ms","p99")}/{g(p,"frame_interval_ms","max")} >20={p["frames_over_20_ms"]} enc p50/p99/max={g(p,"encode_ms","p50")}/{g(p,"encode_ms","p99")}/{g(p,"encode_ms","max")} gpuOff={o and round(o,2)} coarseF={p["frames_with_coarse_tiles"]} maxC={p["coarse_tiles_max"]} blank={p["blank_tiles_max"]} up={p["tiles_uploaded"]} sharp={p.get("sharp_after_ms") is not None and round(p["sharp_after_ms"])} cache={p["cache_mb"]}')
