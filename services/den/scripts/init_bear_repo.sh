#!/usr/bin/env sh
set -eu

usage() {
  echo "usage: $0 <bear_id> [repo_path]" >&2
  echo "  repo_path defaults to MEMFS_BEAR_REPOS_BASE/<bear_id>.git" >&2
}

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  usage
  exit 64
fi

BEAR_ID="$1"
BASE="${MEMFS_BEAR_REPOS_BASE:-./var/memfs-bears}"
if [ "$#" -eq 2 ]; then
  REPO="$2"
else
  REPO="$BASE/$BEAR_ID.git"
fi

if [ -z "$(printf '%s' "$BEAR_ID" | tr -d '[:space:]')" ]; then
  echo "bear_id must not be empty" >&2
  exit 64
fi

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "required command not found: $1" >&2
    exit 69
  fi
}

require_cmd git
require_cmd mktemp

if [ -e "$REPO" ] && ! git --git-dir="$REPO" rev-parse --is-bare-repository >/dev/null 2>&1; then
  echo "path exists but is not a bare git repository: $REPO" >&2
  exit 65
fi

mkdir -p "$(dirname "$REPO")"
if [ ! -e "$REPO" ]; then
  git init --bare "$REPO" >/dev/null
  git --git-dir="$REPO" config http.receivepack true
fi

branch_exists() {
  git --git-dir="$REPO" show-ref --verify --quiet "refs/heads/$1"
}

write_keep() {
  mkdir -p "$1"
  : > "$1/.gitkeep"
}

create_branch() {
  branch="$1"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT HUP INT TERM
  work="$tmp/work"
  mkdir -p "$work"
  git init -b "$branch" "$work" >/dev/null
  git -C "$work" config user.name "BEARS Den"
  git -C "$work" config user.email "den@bears.local"

  case "$branch" in
    talk)
      write_keep "$work/talk/tasks"
      ;;
    pair)
      write_keep "$work/pair/tasks"
      ;;
    curate)
      write_keep "$work/curate"
      write_keep "$work/core/tasks"
      write_keep "$work/core/results"
      ;;
    work)
      write_keep "$work/work/results"
      ;;
    watch)
      write_keep "$work/watch/observations"
      write_keep "$work/watch/subscriptions"
      ;;
    *)
      echo "unknown branch skeleton: $branch" >&2
      exit 66
      ;;
  esac

  git -C "$work" add .
  git -C "$work" commit -m "Initialize $branch branch for bear $BEAR_ID" >/dev/null
  git -C "$work" remote add origin "$REPO"
  git -C "$work" push origin "HEAD:refs/heads/$branch" >/dev/null
  rm -rf "$tmp"
  trap - EXIT HUP INT TERM
}

for branch in talk pair curate work watch; do
  if ! branch_exists "$branch"; then
    create_branch "$branch"
  fi
done

HOOK="$REPO/hooks/pre-receive"
cat > "$HOOK" <<'HOOK_EOF'
#!/usr/bin/env sh
set -eu

zero="0000000000000000000000000000000000000000"

allowed_for_branch() {
  case "$1" in
    refs/heads/talk) printf '%s\n' "talk/" ;;
    refs/heads/pair) printf '%s\n' "pair/" ;;
    refs/heads/curate) printf '%s\n' "curate/" "core/" ;;
    refs/heads/work) printf '%s\n' "work/" ;;
    refs/heads/watch) printf '%s\n' "watch/" ;;
    *) printf '%s\n' "" ;;
  esac
}

while read old new ref; do
  case "$ref" in
    refs/heads/talk|refs/heads/pair|refs/heads/curate|refs/heads/work|refs/heads/watch)
      ;;
    *)
      continue
      ;;
  esac

  allowed="$(allowed_for_branch "$ref")"
  branch="${ref#refs/heads/}"

  if [ "$new" = "$zero" ]; then
    continue
  fi

  if [ "$old" = "$zero" ]; then
    paths="$(git diff-tree --root --no-commit-id --name-only -r "$new")"
  else
    paths="$(git diff --name-only "$old" "$new")"
  fi

  printf '%s\n' "$paths" | while IFS= read -r path; do
    [ -z "$path" ] && continue
    case "$ref:$path" in
      refs/heads/talk:talk/*) ;;
      refs/heads/pair:pair/*) ;;
      refs/heads/curate:curate/*|refs/heads/curate:core/*) ;;
      refs/heads/work:work/*) ;;
      refs/heads/watch:watch/*) ;;
      *)
        echo "branch '$branch' attempted to write to '$path'; allowed path prefixes are:" >&2
        printf '%s\n' "$allowed" | sed 's/^/  - /' >&2
        exit 1
        ;;
    esac
  done
done
HOOK_EOF
chmod +x "$HOOK"

echo "Initialized BEARS multi-agent MemFS repo: $REPO"
echo "Branches: talk pair curate work watch"
echo "Installed pre-receive path enforcement hook."
