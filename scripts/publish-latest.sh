#!/usr/bin/env bash
# Puts a build on the GitHub release "neueste" under a fixed file name, so
# the newest APK and DMG always have the same download link:
#   https://github.com/<repo>/releases/download/neueste/<name>
#
# Usage (in GitHub Actions, with GH_TOKEN set): publish-latest.sh <file> <name>
set -euo pipefail

file="$1"
name="$2"
tag="neueste"
repo="${GITHUB_REPOSITORY:?}"
# On pull requests GITHUB_SHA is a temporary merge commit; tag the branch head.
sha="${PR_HEAD_SHA:-$GITHUB_SHA}"

# Move the tag to this build's commit (create it the first time).
gh api -X PATCH "repos/$repo/git/refs/tags/$tag" -f sha="$sha" -F force=true >/dev/null 2>&1 ||
  gh api -X POST "repos/$repo/git/refs" -f ref="refs/tags/$tag" -f sha="$sha" >/dev/null

notes="Immer die neueste Version (Commit ${sha:0:7}).

- **Android:** [FadisNemotron.apk](https://github.com/$repo/releases/download/$tag/FadisNemotron.apk) – auf dem Handy öffnen und installieren (einmal „Installieren aus unbekannten Quellen“ erlauben). Neue Versionen installieren sich über die alte.
- **Mac (Intel):** [FadisNemotron-Intel.dmg](https://github.com/$repo/releases/download/$tag/FadisNemotron-Intel.dmg) – öffnen und die App in „Programme“ ziehen.

Mac-App und Android-App aus demselben Release passen zusammen."

# Both workflows may get here at the same time; one of them creates it.
gh release view "$tag" --repo "$repo" >/dev/null 2>&1 ||
  gh release create "$tag" --repo "$repo" --title "Neueste Version" --notes "$notes" --latest ||
  true

staged="$(mktemp -d)/$name"
cp "$file" "$staged"
gh release upload "$tag" "$staged" --repo "$repo" --clobber
gh release edit "$tag" --repo "$repo" --title "Neueste Version" --notes "$notes" --latest
echo "Veröffentlicht: https://github.com/$repo/releases/download/$tag/$name"
