#!/usr/bin/env bash
# CPU vs NPU comparison on a real video clip: latency, throughput, CPU time per
# frame, whole-system power (battery) and package energy (RAPL, root only).
#
# Needs the `npu` build of vinoopeneffects-daemon, an onnxruntime-openvino (ovfetch), and
# the models in the per-user model dir. Run on battery for the power numbers.
#
#   scripts/bench-video.sh [CLIP] [SECONDS]
#
# Env: OED (binary), ORT_DYLIB_PATH, FPS (paced rate, default 30).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
oed="${OED:-$repo_root/target/release/vinoopeneffects-daemon}"
export ORT_DYLIB_PATH="${ORT_DYLIB_PATH:-$HOME/.local/lib/vinoopeneffects-ort/libonnxruntime.so}"
secs="${2:-45}"
fps="${FPS:-30}"
cache="${XDG_CACHE_HOME:-$HOME/.cache}/vinoopeneffects-bench"
mkdir -p "$cache"

# Public-domain interview (Wikimedia Commons), 1280x720 VP8. Title cards sit
# between answers, so cut a stretch that is a person talking to camera.
src_url="https://upload.wikimedia.org/wikipedia/commons/7/7b/Science_Speaks_-_Talking_to_Women_in_Science_%28Liz%2C_Epidemiologist%29.webm"
src_sha="097151f59acf2c76fca34fdc84b75e91d16cd7f167b464ada7bb926b13f07017"
clip="${1:-$cache/clip.webm}"
if [ ! -f "$clip" ]; then
    full="$cache/liz.webm"
    if [ ! -f "$full" ]; then
        echo "downloading stock clip to $full"
        curl -fsSL -A "vinoopeneffects-bench" "$src_url" -o "$full.tmp"
        [ "$(sha256sum "$full.tmp" | cut -d' ' -f1)" = "$src_sha" ] \
            || { echo "sha256 mismatch for stock clip" >&2; rm -f "$full.tmp"; exit 1; }
        mv "$full.tmp" "$full"
    fi
    ffmpeg -v error -y -ss 112 -t 28 -i "$full" -an -c:v copy "$clip"
fi

bat=/sys/class/power_supply/BAT0
rapl=/sys/class/powercap/intel-rapl:0/energy_uj
npu_busy=/sys/class/accel/accel0/device/npu_busy_time_us
log="$(mktemp)"
trap 'rm -f "$log"; systemctl --user start vinoopeneffects-daemon 2>/dev/null || true' EXIT
systemctl --user stop vinoopeneffects-daemon 2>/dev/null || true

on_battery=0
[ "$(cat $bat/status 2>/dev/null)" = "Discharging" ] && on_battery=1
[ $on_battery = 1 ] || echo "warning: not discharging, battery power will read 0 (unplug for power numbers)" >&2
rapl_ok=0
[ -r "$rapl" ] && rapl_ok=1
[ $rapl_ok = 1 ] || echo "note: $rapl not readable; re-run with sudo for package energy" >&2

# Average battery draw in watts over N seconds, sampled at 1 Hz.
sample_watts() {
    local n="$1" sum=0 c=0 v
    for _ in $(seq "$n"); do
        v=$(cat $bat/power_now 2>/dev/null || echo 0)
        sum=$((sum + v)); c=$((c + 1)); sleep 1
    done
    python3 -c "print(round($sum / $c / 1e6, 2))"
}

idle() { # label
    local w; w=$(sample_watts 20)
    echo "idle $1: ${w} W (battery)" >&2
    echo "$w"
}

# run_one EP MODE(flat|paced) -> prints a summary line plus power fields
run_one() {
    local ep="$1" mode="$2" args="" w="" j="" nb0 nb1 r0 r1 t0 t1
    if [ "$mode" = paced ]; then args="--fps $fps --seconds $secs"; else args="--seconds 20"; fi
    : > "$log"
    nb0=$(cat $npu_busy 2>/dev/null || echo 0)
    VINOOPENEFFECTS_EP="$ep" "$oed" --bench-video "$clip" $args > "$log" 2>&1 &
    local pid=$!
    until grep -q '^# measuring' "$log"; do
        kill -0 $pid 2>/dev/null || { cat "$log" >&2; return 1; }
        sleep 0.2
    done
    if [ "$mode" = paced ]; then
        [ $rapl_ok = 1 ] && r0=$(cat $rapl); t0=$(date +%s.%N)
        w=$(sample_watts $((secs - 2)))
        [ $rapl_ok = 1 ] && r1=$(cat $rapl); t1=$(date +%s.%N)
        if [ $rapl_ok = 1 ]; then
            j=$(python3 -c "print(round(($r1 - $r0) / 1e6 / ($t1 - $t0), 2))")
        fi
    fi
    wait $pid
    nb1=$(cat $npu_busy 2>/dev/null || echo 0)
    echo "$(grep -h '^ep=' "$log") battery_w=${w:-na} pkg_w=${j:-na} npu_busy_ms=$(( (nb1 - nb0) / 1000 ))"
}

echo "clip: $clip   binary: $oed   paced fps: $fps for ${secs}s" >&2
echo "== flat out (throughput) =="
for ep in cpu npu; do run_one $ep flat; done

echo "== paced at ${fps} fps (power) =="
idle0=$(idle before)
for ep in cpu npu npu cpu; do run_one $ep paced; done
idle1=$(idle after)
echo "idle_before_w=$idle0 idle_after_w=$idle1"
