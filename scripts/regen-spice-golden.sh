#!/usr/bin/env bash
# Regenerates ngspice golden data for the bb-spice reference suite (SPEC SPICE-6).
#
#   scripts/regen-spice-golden.sh            # all circuits
#   scripts/regen-spice-golden.sh rc_tran    # one circuit
#
# For every crates/bb-spice/tests/reference/<name>.cir this runs ngspice in batch mode and
# stores <name>.raw (ASCII rawfile, values rounded to 10 significant digits) and, when the
# circuit has .meas statements, <name>.meas (ngspice's printed measurement results).
#
# * If <name>.ngspice.cir exists it is simulated instead (an ngspice-equivalent netlist for
#   Breadbox extensions ngspice lacks, e.g. a logarithmic .dc over a .param).
# * If the netlist contains a .control block, that block must run the analyses and
#   `write golden.raw` (with `set filetype=ascii` and `set appendwrite`) in the same order as
#   Breadbox runs them (.step points outermost, analyses in netlist order). Breadbox ignores
#   .control blocks.
# * Otherwise ngspice runs the dot analyses itself (-r golden.raw).
# * ngspice runs with `.options reltol=1e-5` unless the netlist sets reltol, so the golden data
#   is closer to the exact solution than the 0.1 % tolerance it is compared at.
#
# The golden files are checked in, so `cargo test -p bb-spice` never needs ngspice.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REF="$ROOT/crates/bb-spice/tests/reference"

if ! command -v ngspice >/dev/null 2>&1; then
    echo "ngspice not found — install it with: brew install ngspice" >&2
    exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
ONLY="${1:-}"
FAILED=0

compact_raw() {
    # Rounds every value to 10 significant digits, blanks the Date line, and decimates plots
    # with more than MAXP points (the comparison interpolates Breadbox results at the golden
    # time points, so every kept ngspice point is still exact).
    awk -v MAXP=2500 '
        function fmt(v,   c) {
            if (v ~ /,/) { split(v, c, ","); return sprintf("%.10e,%.10e", c[1], c[2]) }
            return sprintf("%.10e", v)
        }
        function flush(   stride, kept, p, j, k, i, l) {
            if (nhdr == 0) return
            stride = 1
            if (npts > MAXP) stride = int((npts + MAXP - 1) / MAXP)
            kept = 0
            for (p = 0; p < npts; p++) if (p % stride == 0 || p == npts - 1) kept++
            for (i = 1; i <= nhdr; i++) {
                l = hdr[i]
                if (l ~ /^No\. Points:/) l = "No. Points: " kept
                print l
            }
            k = 0
            for (p = 0; p < npts; p++) {
                if (p % stride == 0 || p == npts - 1) {
                    for (j = 0; j < nvars; j++) {
                        if (j == 0) print k "\t\t" fmt(val[p, j]); else print "\t" fmt(val[p, j])
                    }
                    k++
                }
            }
            nhdr = 0; invals = 0; ntok = 0
            delete val
        }
        /^Title:/ { flush() }
        {
            if (!invals) {
                line = $0
                if (line ~ /^Date:/) line = "Date: (golden)"
                hdr[++nhdr] = line
                if (line ~ /^No\. Variables:/) { nvars = $3 + 0 }
                if (line ~ /^No\. Points:/) { npts = $3 + 0 }
                if (line ~ /^Values:/) { invals = 1; ntok = 0 }
                next
            }
            for (i = 1; i <= NF; i++) {
                # each point is: index, then nvars values
                pos = ntok % (nvars + 1)
                p = int(ntok / (nvars + 1))
                if (pos > 0) val[p, pos - 1] = $i
                ntok++
            }
        }
        END { flush() }' "$1"
}

for cir in "$REF"/*.cir; do
    base="$(basename "$cir" .cir)"
    case "$base" in *.ngspice) continue ;; esac
    if [ -n "$ONLY" ] && [ "$ONLY" != "$base" ]; then continue; fi
    src="$cir"
    if [ -f "$REF/$base.ngspice.cir" ]; then src="$REF/$base.ngspice.cir"; fi
    work="$TMP/$base"
    mkdir -p "$work"
    # .step/.mc are Breadbox (LTspice-style) extensions ngspice rejects; the .control block
    # emulates them, so drop the cards from ngspice's copy.
    sed -E '2,$ s/^([[:space:]]*\.(step|mc)[[:space:]])/* \1/I' "$src" >"$work/in.cir"
    # Tighter Newton tolerance for the reference: ngspice's DC sweeps otherwise stop within its
    # default 0.1 % reltol, which is the very tolerance we compare at. (Breadbox runs with the
    # netlist's own options.) Circuits that set reltol themselves keep their value.
    if ! grep -qi 'reltol' "$work/in.cir"; then
        awk 'NR == 1 { print; print ".options reltol=1e-5"; next } { print }' "$work/in.cir" >"$work/in2.cir"
        mv "$work/in2.cir" "$work/in.cir"
    fi
    (
        cd "$work"
        if grep -qi '^\.control' in.cir; then
            ngspice -b in.cir >log.txt 2>&1 || true
        else
            SPICE_ASCIIRAWFILE=1 ngspice -b -r golden.raw in.cir >log.txt 2>&1 || true
        fi
    )
    if [ ! -s "$work/golden.raw" ]; then
        echo "FAILED: $base" >&2
        sed 's/^/    /' "$work/log.txt" >&2
        FAILED=1
        continue
    fi
    compact_raw "$work/golden.raw" >"$REF/$base.raw"
    if grep -qiE '^\.meas' "$cir"; then
        grep -E '^[A-Za-z][A-Za-z0-9_]*[[:space:]]+=[[:space:]]' "$work/log.txt" >"$REF/$base.meas" || true
    fi
    echo "ok: $base"
done

ngspice --version 2>/dev/null | grep -o 'ngspice-[0-9][0-9.]*' | head -1 >"$REF/NGSPICE_VERSION"
exit $FAILED
