#!/usr/bin/env bash
# Acquire local Gemma weights once, then build and launch the metalOS UI.
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$root"

if [[ "${1:-}" == "--non-interactive" ]]; then
    shift
elif [[ -t 0 && -t 1 && "${1:-}" != "--help" && "${1:-}" != "-h" ]]; then
    exec python3 "$root/scripts/setup_run.py" "$@"
fi

auth_args=()
run_args=()
while (( $# )); do
    case "$1" in
        --auth-env|--auth-file)
            if (( $# < 2 )) || [[ -z "$2" || "$2" == --* ]]; then
                echo "Missing value for $1" >&2
                exit 2
            fi
            auth_args+=("$1" "$2")
            shift 2
            ;;
        -h|--help)
            cat <<'HELP'
Usage: ./run.sh [--non-interactive] [--auth-env VARIABLE | --auth-file PATH] [UI launch options]

In a terminal, opens a step-by-step setup guide for Gemma and optional cloud
API credentials. Use --non-interactive as the first option to skip the guide.
Cloud setup is skipped when credentials exist; use --setup-cloud to add another.

Downloads Gemma if its local model image is missing, then builds and runs
the UI in release mode with the model attached (16 GiB guest RAM by default).
Accept the Gemma license on Hugging Face before the first download.
Authentication defaults to HF_TOKEN. Pass a variable name or private token
file path, never a token value. Assets remain under ignored .local/models/.

UI launch options are forwarded to xtask run, e.g. --memory 20G or --debug-shell.
HELP
            exit 0
            ;;
        --model-image|--model-image=*)
            echo "run.sh manages the Gemma image; use xtask run for a custom model." >&2
            exit 2
            ;;
        *)
            run_args+=("$1")
            shift
            ;;
    esac
done

model_image="$root/.local/models/gemma-2-2b-it/model.img"
if [[ ! -f "$model_image" ]]; then
    echo "[run] Preparing local Gemma weights..."
    python3 "$root/scripts/download_gemma.py" ${auth_args[@]+"${auth_args[@]}"}
fi

echo "[run] Building and launching metalOS with Gemma..."
exec cargo run -p xtask -- run --release --model-image "$model_image" ${run_args[@]+"${run_args[@]}"}
