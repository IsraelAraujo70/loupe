#!/bin/bash
# Puts the Loupe test fixture (docs/TEST_FIXTURE.md) back the way it was
# seeded: deletes the comments tests wrote (they start with the test prefix),
# drops your pending review and your viewed marks, converts a fixture pull
# request taken out of draft back to a draft, and restores the two seeded
# threads (one open, one resolved). Submitted reviews cannot be deleted on
# GitHub; their comments go, the empty review stays.
#
# Usage: scripts/loupe-fixture-reset.sh [pull request number...]
# Needs gh signed in with write access to the repository.
set -euo pipefail

REPO=IsraelAraujo70/loupe
PREFIX='🤖 Loupe test:'
if [ $# -gt 0 ]; then PRS=("$@"); else PRS=(186 187 188 189 190); fi

for n in "${PRS[@]}"; do
  title=$(gh pr view "$n" --repo "$REPO" --json title --jq .title)
  case "$title" in
    "[Loupe test fixture]"*) ;;
    *) echo "#$n is not a Loupe test fixture pull request: $title" >&2; exit 1 ;;
  esac

  gh api --paginate "repos/$REPO/issues/$n/comments" \
    --jq ".[] | select(.body | startswith(\"$PREFIX\")) | .id" |
    while read -r id; do
      gh api -X DELETE "repos/$REPO/issues/comments/$id" >/dev/null
      echo "#$n: deleted conversation comment $id"
    done
  gh api --paginate "repos/$REPO/pulls/$n/comments" \
    --jq ".[] | select(.body | startswith(\"$PREFIX\")) | .id" |
    while read -r id; do
      gh api -X DELETE "repos/$REPO/pulls/comments/$id" >/dev/null
      echo "#$n: deleted review comment $id"
    done
  gh api --paginate "repos/$REPO/pulls/$n/reviews" --jq '.[] | select(.state == "PENDING") | .id' |
    while read -r id; do
      gh api -X DELETE "repos/$REPO/pulls/$n/reviews/$id" >/dev/null
      echo "#$n: discarded pending review $id"
    done

  state=$(gh api graphql -F n="$n" -f query='
    query($n: Int!) { repository(owner: "IsraelAraujo70", name: "loupe") { pullRequest(number: $n) {
      isDraft
      files(first: 100) { nodes { path viewerViewedState } }
      reviewThreads(first: 100) { nodes { id isResolved comments(first: 1) { nodes { body } } } }
    } } }' --jq .data.repository.pullRequest)
  id=$(gh pr view "$n" --repo "$REPO" --json id --jq .id)

  if [ "$(jq -r .isDraft <<<"$state")" = false ]; then
    gh pr ready "$n" --repo "$REPO" --undo >/dev/null
    echo "#$n: back to draft"
  fi
  jq -r '.files.nodes[] | select(.viewerViewedState != "UNVIEWED") | .path' <<<"$state" |
    while read -r path; do
      gh api graphql -f id="$id" -f path="$path" \
        -f query='mutation($id: ID!, $path: String!) { unmarkFileAsViewed(input: {pullRequestId: $id, path: $path}) { clientMutationId } }' >/dev/null
      echo "#$n: unmarked $path"
    done
  jq -r '.reviewThreads.nodes[]
    | select((.comments.nodes[0].body | contains("Fixture seed (open thread)")) and .isResolved) | .id' <<<"$state" |
    while read -r thread; do
      gh api graphql -f id="$thread" -f query='mutation($id: ID!) { unresolveReviewThread(input: {threadId: $id}) { clientMutationId } }' >/dev/null
      echo "#$n: reopened the seeded open thread"
    done
  jq -r '.reviewThreads.nodes[]
    | select((.comments.nodes[0].body | contains("Fixture seed (resolved thread)")) and (.isResolved | not)) | .id' <<<"$state" |
    while read -r thread; do
      gh api graphql -f id="$thread" -f query='mutation($id: ID!) { resolveReviewThread(input: {threadId: $id}) { clientMutationId } }' >/dev/null
      echo "#$n: resolved the seeded resolved thread"
    done
done
echo "fixture reset: ${PRS[*]}"
