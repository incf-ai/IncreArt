#!/usr/bin/env bash
# Makes the promotional image, docs/readme/promo.png (1280×640, GitHub's
# social preview size) and docs/readme/promo-2x.png (2560×1280): the logo,
# the tagline and four frames. Generate is moonwalk-styles' style sweep, its
# blanks with prompt enhance's suggestions, in a 1920×1080 window where they
# lay out in lines (the tour's P1 scene); Review the same sweep's contact
# sheet (G09); A/B compare and the Output library the README's shots (R2,
# R4). Each is taken at twice the pixels on a fresh copy of the example
# library, on a private display (scripts/headless.sh).
#
#   scripts/make-promo.sh
#
# The copy of example-library/ goes where a release zip would unpack it, as
# for scripts/make-readme-gallery.sh, and is removed afterwards. Build first:
# cargo build --release -p increart-gui
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/crates/increart-gui/Cargo.toml")
unpack=$HOME/Downloads/increart-v$version-linux-x86_64
out=$root/docs/readme
fonts=$root/crates/increart-gui/assets/fonts
regular=$fonts/NotoSansCJKsc-Regular.otf
bold=DejaVu-Sans-Bold

[[ -e $unpack ]] && { echo "$unpack is in the way" >&2; exit 1; }
work=$(mktemp -d)
trap 'rm -rf "$work" "$unpack"' EXIT
mkdir -p "$unpack"

# twice the pixels: the 1400×900 window at a scale factor of 2; each run on a
# fresh copy of the library (PROMO_SHOTS=<dir> uses shots already taken, to
# try another layout)
# shoot <name> <window> <scenes>
shoot() {
    rm -rf "$unpack/Example library"
    cp -R "$root/example-library" "$unpack/Example library"
    rm -rf "$unpack/Example library/"{cache,thumbs,library.lock}
    local w=${2%x*} h=${2#*x}
    HEADLESS_SCREEN=$((w * 2 + 60))x$((h * 2 + 60))x24 "$root/scripts/headless.sh" env WINIT_X11_SCALE_FACTOR=2 \
        INCREART_TOUR_WINDOW="$2" INCREART_CONFIG="$work/$1/config.rsn" XDG_STATE_HOME="$work/$1/state" \
        INCREART_TOUR="$work/shots" INCREART_TOUR_LIBRARY="$unpack/Example library" \
        INCREART_TOUR_SCENES="$3" \
        "$root/target/release/increart" > "$work/$1.log" 2>&1 || { cat "$work/$1.log" >&2; exit 1; }
}
if [[ -n ${PROMO_SHOTS:-} ]]; then
    cp -R "$PROMO_SHOTS" "$work/shots"
else
    # R2 goes on from R1
    shoot readme 1400x900 02-server-dialog,03-idea-inbox-grid,R1,R2,R4
    shoot generate 1920x1080 02-server-dialog,03-idea-inbox-grid,P1
    # G09's contact sheet comes after the gallery's scenes before it
    shoot gallery 1400x900 02-server-dialog,G0
fi

# The 2560×1280 image; promo.png is it halved.
W=2560 H=1280 M=56 GAP=36 TOP=250
FW=$(( (W - 2 * M - GAP) / 2 ))
FH=$(( (H - TOP - M - GAP) / 2 ))

# shot | crop in its pixels (twice the window's) | title | pitch | solid band
frames=(
    "P1-generate-moonwalk|2280x916+690+770|Generate|Sweep styles, with prompt enhance's suggestions|solid"
    "G09-contact-sheet|2100x844+40+216|Review|A row per style, a column per seed"
    "R2-review-compare|2300x924+480+460|A/B compare|Two images side by side, zoomed in together"
    "R4-output-lineage|2290x920+500+240|Library|Every image keeps its recipe and where it came from"
)

i=0
for f in "${frames[@]}"; do
    IFS='|' read -r shot crop title pitch solid <<< "$f"
    # over images a shade, over text a solid band (text showing through is noise)
    if [[ -n $solid ]]; then
        band=(-size "${FW}x80" gradient:'rgba(10,12,16,0)-rgba(10,12,16,1)' -size "${FW}x116" xc:'rgb(10,12,16)' -append)
    else
        band=(-size "${FW}x190" gradient:'rgba(10,12,16,0)-rgba(10,12,16,0.94)')
    fi
    # the crop filling the frame, a band at its foot for the caption, fading in: the
    # title, then the pitch after it on the same line
    magick -background none -fill white -font "$bold" -pointsize 44 label:"$title" "$work/title.png"
    tw=$(magick identify -format %w "$work/title.png")
    magick "$work/shots/$shot.png" -crop "$crop" +repage -resize "${FW}x${FH}^" \
        -gravity center -extent "${FW}x${FH}" \
        \( "${band[@]}" \) \
        -gravity south -composite \
        "$work/title.png" -gravity southwest -geometry +34+26 -composite \
        -fill '#c9ced8' -font "$regular" -pointsize 32 -annotate +$((34 + tw + 22))+32 "$pitch" \
        +gravity +geometry \
        -fill none -stroke 'rgba(255,255,255,0.16)' -strokewidth 2 \
        -draw "roundrectangle 1,1,$((FW - 2)),$((FH - 2)),22,22" +stroke \
        \( -size "${FW}x${FH}" xc:black -fill white -draw "roundrectangle 0,0,$((FW - 1)),$((FH - 1)),22,22" \) \
        -alpha off -compose CopyOpacity -composite \
        "$work/frame$i.png"
    # its shadow
    magick "$work/frame$i.png" \( +clone -background black -shadow 70x18+0+10 \) +swap \
        -background none -layers merge +repage "$work/framesh$i.png"
    i=$((i + 1))
done

# the background: dark, lighter towards the top left where the logo is
magick -size "${W}x${H}" radial-gradient:'#2b3140-#12141a' \
    -distort SRT "0,0 1.6 0 $((W / 5)),$((H / 6))" "$work/bg.png"

# the logo at the top left, the tagline beside it (slanted: no italic font here)
magick -background none -fill white -font "$regular" -pointsize 64 label:"Gotta sketch 'em all!" \
    -shear 12x0 -trim +repage "$work/tagline.png"
magick -background none -fill '#aeb4c0' -font "$regular" -pointsize 36 \
    label:"Ideas to finished images on your own GPUs: generate, review, compare, keep." \
    -trim +repage "$work/pitch.png"

logo_h=200
magick "$work/bg.png" \
    \( "$root/crates/increart-gui/assets/logo.png" -resize "x$logo_h" \) -geometry "+$((M - 22))+$((M - 34))" -composite \
    "$work/tagline.png" -geometry "+$((M + 640))+$((M + 22))" -composite \
    "$work/pitch.png" -geometry "+$((M + 644))+$((M + 112))" -composite \
    "$work/framesh0.png" -geometry "+$((M - 36))+$((TOP - 28))" -composite \
    "$work/framesh1.png" -geometry "+$((M + FW + GAP - 36))+$((TOP - 28))" -composite \
    "$work/framesh2.png" -geometry "+$((M - 36))+$((TOP + FH + GAP - 28))" -composite \
    "$work/framesh3.png" -geometry "+$((M + FW + GAP - 36))+$((TOP + FH + GAP - 28))" -composite \
    -background '#12141a' -alpha remove -alpha off "$work/promo-2x.png"

magick "$work/promo-2x.png" -strip PNG24:"$out/promo-2x.png"
magick "$work/promo-2x.png" -filter Lanczos -resize 1280x640 -strip PNG24:"$out/promo.png"
ls -la "$out/promo.png" "$out/promo-2x.png"
