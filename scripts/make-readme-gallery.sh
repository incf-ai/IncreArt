#!/usr/bin/env bash
# Retakes the README's gallery, docs/readme/GALLERY.md: the screenshot
# tour's G and V scenes (crates/increart-gui/src/tour.rs) on a fresh copy of the
# example library, on a private display (scripts/headless.sh), each shot
# saved as WebP in docs/readme/gallery/. The page also shows the README's
# own five shots.
#
#   scripts/make-readme-gallery.sh
#
# The copy of example-library/ is put where a release zip would unpack it,
# ~/Downloads/increart-v<version>-linux-x86_64/, like the README's shots, as
# its path shows in them; it's removed afterwards. The V scenes validate a
# preset whose sd-servers are the suite's fake. Build first:
# cargo build --release -p increart-gui -p increart-apitest --features increart-apitest/fake
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/crates/increart-gui/Cargo.toml")
unpack=$HOME/Downloads/increart-v$version-linux-x86_64
out=$root/docs/readme/gallery

[[ -e $unpack ]] && { echo "$unpack is in the way" >&2; exit 1; }
work=$(mktemp -d)
trap 'rm -rf "$work" "$unpack"' EXIT
mkdir -p "$unpack"
cp -R "$root/example-library" "$unpack/Example library"
rm -rf "$unpack/Example library/"{cache,thumbs,library.lock}

# 02 opens the library; a new config and state, so nothing of this machine's shows
"$root/scripts/headless.sh" env INCREART_CONFIG="$work/config/config.rsn" XDG_STATE_HOME="$work/state" \
    INCREART_TOUR="$work/shots" INCREART_TOUR_LIBRARY="$unpack/Example library" \
    INCREART_TOUR_SCENES=02-server-dialog,G \
    "$root/target/release/increart" > "$work/run.log" 2>&1 || { cat "$work/run.log" >&2; exit 1; }

# Validate servers: two fake sd-servers listing the default model preset's files
fake=$root/target/release/increart-fake-sd-server
[[ -x $fake ]] || { echo "no $fake: build it first (see above)" >&2; exit 1; }
files=diffusion_models/qwen_image_2.1_int8_convrot.safetensors,text_encoders/qwen3vl_8b_int8_convrot.safetensors,vae/qwen_image_2.1_vae_bf16.safetensors
mkdir -p "$work/vconfig"
cat > "$work/vconfig/config.rsn" <<RSN
Config {
    server_presets: [
        ServerPreset {
            name: "Two GPUs",
            servers: [
                SdServer { name: "gpu0", run: Launch { port: 18931, args: ["--fake-files", "$files"] } },
                SdServer { name: "gpu1", run: Launch { port: 18932, args: ["--fake-files", "$files"] } }
            ]
        }
    ],
    last_preset: "Two GPUs",
    programs: Programs { sd_server: SdServerSettings { binary: "$fake" } }
}
RSN
"$root/scripts/headless.sh" env INCREART_CONFIG="$work/vconfig/config.rsn" XDG_STATE_HOME="$work/vstate" \
    INCREART_TOUR="$work/shots" INCREART_TOUR_LIBRARY="$unpack/Example library" \
    INCREART_TOUR_SCENES=02-server-dialog,V \
    "$root/target/release/increart" > "$work/vrun.log" 2>&1 || { cat "$work/vrun.log" >&2; exit 1; }

rm -rf "$out"
mkdir -p "$out"
for png in "$work"/shots/[GV]*.png; do
    name=$(basename "$png" .png)
    # G02a-home → home
    magick "$png" -quality 85 -define webp:method=6 "$out/${name#*-}.webp"
done

# file | caption; files without a folder are the gallery's, the rest the README's
captions=(
    "server-wizard|**Set up servers.** Choose KoboldCpp and IncreArt finds your GPUs, picks a preset for each card's memory and shares the work between them: here two 24 GB cards each make images and run prompt enhance."
    "validate-servers|**Validate servers.** Before a preset is first used, a few minutes of checks that IncreArt and its servers work together: each server answers, lists its models and makes a small image, then a short run goes through all of them. What fails comes with what to try."
    "home|**Home.** Where the app opens: how ideas become finished images, step by step, each with its tab, its keys and what's waiting there. Start a new project (it asks for reference images too), write an idea, import images or open the drop folder."
    "idea-inbox|**Idea inbox.** Ideas come in as text, images or both: typed in, imported, or saved into the drop folder from a browser. Save each one to the input library, discard it or make a new project from it."
    "../input-library.png|**Input library.** Kept ideas in folders, with their source images, ready to start projects from."
    "projects-grid|**Projects.** Each project with its cover and its counts of good, neutral, bad and new images, in folders."
    "../generate.png|**Generate: a style sweep.** One prompt in several styles. Prompt enhance suggested values for its blanks, \`{cat_gear}\` and \`{sky}\`, and one of each is picked."
    "generate-refs|**Reference images.** Two vintage travel posters as \`<image1>\` and \`<image2>\` for a new one. Prompt enhance rewrote the prompt and has suggestions for its \`{foreground}\` blank."
    "setting-sweep|**Setting sweep.** The same prompt and seeds at five CFG values, with the time it will take before you queue it."
    "prompt-history|**Prompt history.** Every version of a project's prompt and settings, to compare with the current one or restore. Here the prompt before prompt enhance's rewrite."
    "crop-mark|**Crop and mark.** An output as the start image of the next run, cropped, with marks to show what to change."
    "jobs|**Jobs.** Queued runs, spread over the servers. Finish a run's current images and stop, or stop at once."
    "../review-grid.png|**Review.** New images in a grid, each rated good, neutral or bad with a key."
    "../review-compare.png|**A/B compare.** Two images side by side, to pick the better one."
    "contact-sheet|**Contact sheet.** A style sweep with a row per style and a column per seed. Rate a whole row at once, or ask for more seeds of the styles you kept."
    "tile-window|**Tile.** Picked images laid out as one image with a title and captions, to save or copy."
    "../output-lineage.png|**Output library: lineage.** Every kept image with where it came from (its idea, references and recipe) and what was made from it."
    "export|**Export.** Copy kept images out of the library, with or without their metadata."
)
{
    echo "# IncreArt gallery"
    echo
    echo "The main screens, on the [example library](../../README.md#example-library)"
    echo "that comes with each release. No servers were running: the jobs are"
    echo "simulated. Retaken by \`scripts/make-readme-gallery.sh\`; don't edit by hand."
    for c in "${captions[@]}"; do
        file=${c%%|*} caption=${c#*|}
        case $file in
            ../*) src=${file#../} ;;
            *) src=gallery/$file.webp; [[ -f $out/$file.webp ]] || { echo "no shot $file" >&2; exit 1; } ;;
        esac
        alt=$(sed 's/\*\*\([^*]*\)\*\*.*/\1/; s/\.$//' <<< "$caption")
        echo
        echo "$caption"
        echo
        echo "![$alt]($src)"
    done
} > "$root/docs/readme/GALLERY.md"

for w in "$out"/*.webp; do
    grep -q "(gallery/${w##*/})" "$root/docs/readme/GALLERY.md" || echo "no caption for ${w##*/}" >&2
done
ls "$out" | wc -l | xargs echo "shots:"
du -sh "$out"
