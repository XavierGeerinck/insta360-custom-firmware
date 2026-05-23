#!/usr/bin/env bash
# F2 §3.4 elevation bias measurement. Runs WAVED_DOA_DEBUG, records
# measured vs truth el at 4 elevations × 4 azimuths, output to
# /tmp/elevation-measurements.csv on device + pulled to host.
#
# Operator-driven (placement of source). Outcome → AUDIO_HARDWARE.md
# update in Task 19; no auto-PASS/FAIL gate.

set -euo pipefail
DEVICE="${DEVICE:-PSP_0000000000000000}"
adb_run() { adb -s "$DEVICE" shell "$@"; }

CSV=/tmp/elevation-measurements.csv
adb_run "echo 'truth_az,truth_el,measured_az,measured_el,sharp_0_2,sharp_1_3,sharp_0_4,sharp_2_6' > $CSV"

for az in 0 90 180 270; do
    for el in -30 0 30 60; do
        echo
        echo "Position loudspeaker at az=${az}°, el=${el}° relative to top-ring plane."
        read -r -p "press ENTER when ready: " _
        # 5 s WAVED_DOA_DEBUG burst → grep latest line, extract numbers.
        adb_run "WAVED_DOA_DEBUG=100 timeout 5 /data/waved/waved 2>&1 | grep doa_debug | tail -5"
        # Manually transcribe into CSV — easier than parsing the log format.
        read -r -p "measured az: " m_az
        read -r -p "measured el: " m_el
        read -r -p "sh values comma-separated (sh_02,sh_13,sh_04,sh_26): " shs
        adb_run "echo '${az},${el},${m_az},${m_el},${shs}' >> $CSV"
    done
done

adb pull "$CSV" ./elevation-measurements-$(date +%Y%m%d).csv
echo
echo "Done. Analyse the CSV; update AUDIO_HARDWARE.md with findings (Task 19)."
