#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
SCRIPT="$ROOT/scripts/init_bear_repo.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT HUP INT TERM
REPO="$TMP/test-bear.git"

sh "$SCRIPT" test-bear "$REPO" >/dev/null

for branch in talk pair curate work watch; do
  git --git-dir="$REPO" show-ref --verify --quiet "refs/heads/$branch"
done

talk_work="$TMP/talk"
git clone --branch talk "$REPO" "$talk_work" >/dev/null 2>&1
mkdir -p "$talk_work/core"
echo nope > "$talk_work/core/nope.txt"
git -C "$talk_work" add .
git -C "$talk_work" config user.name test
git -C "$talk_work" config user.email test@example.test
git -C "$talk_work" commit -m bad >/dev/null
if git -C "$talk_work" push origin HEAD:talk >/tmp/talk-push.out 2>&1; then
  echo "expected talk branch push to core/ to fail" >&2
  cat /tmp/talk-push.out >&2
  exit 1
fi
if ! grep -q "branch 'talk' attempted to write to 'core/nope.txt'" /tmp/talk-push.out; then
  echo "expected clear pre-receive error" >&2
  cat /tmp/talk-push.out >&2
  exit 1
fi

curate_work="$TMP/curate"
git clone --branch curate "$REPO" "$curate_work" >/dev/null 2>&1
mkdir -p "$curate_work/core"
echo yep > "$curate_work/core/yep.txt"
git -C "$curate_work" add .
git -C "$curate_work" config user.name test
git -C "$curate_work" config user.email test@example.test
git -C "$curate_work" commit -m good >/dev/null
git -C "$curate_work" push origin HEAD:curate >/dev/null

echo "init_bear_repo.sh ok"
