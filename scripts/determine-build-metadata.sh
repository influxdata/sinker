#!/usr/bin/env bash

# Determine build metadata for container images using git tags and an optional prefix.

set -euo pipefail

usage() {
  cat <<'EOF'
Usage: determine-build-metadata.sh --tag-match-pattern PATTERN [--strip-prefix PREFIX] [--output PATH]

Options:
  --tag-match-pattern PATTERN  Glob pattern (e.g. module-v*) used with git describe to locate tags.
  --strip-prefix PREFIX        Prefix removed from the resolved tag when emitting image_tag.
  --output PATH                File to append output key/value pairs to. Defaults to $GITHUB_OUTPUT if set, or stdout.
  -h, --help                   Show this help message and exit.
EOF
}

TAG_MATCH_PATTERN=""
STRIP_PREFIX=""
OUTPUT_PATH="${GITHUB_OUTPUT:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag-match-pattern)
      TAG_MATCH_PATTERN="$2"
      shift 2
      ;;
    --strip-prefix)
      STRIP_PREFIX="$2"
      shift 2
      ;;
    --output)
      OUTPUT_PATH="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "$TAG_MATCH_PATTERN" ]]; then
  echo "--tag-match-pattern is required" >&2
  exit 1
fi

if [[ -z "$OUTPUT_PATH" ]]; then
  OUTPUT_PATH="/dev/stdout"
fi

git fetch --tags --force >/dev/null 2>&1 || true

REF_TYPE="${GITHUB_REF_TYPE:-}"
REF_NAME="${GITHUB_REF_NAME:-}"
REF="${GITHUB_REF:-}"

RAW_IMAGE_TAG=""

if [[ "$REF_TYPE" == "tag" ]]; then
  if [[ -n "$REF_NAME" && "$REF_NAME" == $TAG_MATCH_PATTERN ]]; then
    BASE_TAG="$REF_NAME"
  else
    if ! BASE_TAG="$(git describe --tags --abbrev=0 --match "$TAG_MATCH_PATTERN" --exact-match)"; then
      echo "The current ref ${REF_NAME:-"<unknown>"} does not match the expected $TAG_MATCH_PATTERN pattern." >&2
      exit 1
    fi
  fi
  RAW_IMAGE_TAG="$BASE_TAG"
else
  if ! BASE_TAG="$(git describe --tags --abbrev=0 --match "$TAG_MATCH_PATTERN")"; then
    echo "Failed to determine the latest $TAG_MATCH_PATTERN tag for non-tag ref ${REF:-"<unknown>"}." >&2
    exit 1
  fi
  TIME_STAMP_ISO="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  TIME_STAMP="${TIME_STAMP_ISO//:/-}"
  RAW_IMAGE_TAG="${BASE_TAG}-${TIME_STAMP}"
fi

if [[ -n "$STRIP_PREFIX" ]]; then
  IMAGE_TAG="${RAW_IMAGE_TAG#${STRIP_PREFIX}}"
else
  IMAGE_TAG="$RAW_IMAGE_TAG"
fi

SHORT_SHA="$(git rev-parse --short HEAD)"

{
  echo "base_tag=${BASE_TAG}"
  echo "raw_image_tag=${RAW_IMAGE_TAG}"
  echo "image_tag=${IMAGE_TAG}"
  echo "short_sha=${SHORT_SHA}"
} >>"$OUTPUT_PATH"
