#!/bin/sh
# POSIX installer. Download this script from a trusted pinned tag for reproducibility.
set -eu
repo=neko233-com/ssh-client
version=latest
prefix=${HOME:?HOME is required}/.local/share/ssh-client
update_path=1
uninstall=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --version|--prefix) [ "$#" -ge 2 ] || { echo "Missing value: $1" >&2; exit 2; }; option=$1; shift; case "$option" in --version) version=$1;; --prefix) prefix=$1;; esac;;
    --no-path) update_path=0;;
    --uninstall) uninstall=1;;
    --help) echo 'Usage: install.sh [--version vX.Y.Z] [--prefix absolute-directory] [--no-path] [--uninstall]'; exit 0;;
    *) echo "Unknown option: $1" >&2; exit 2;;
  esac
  shift
done
case "$prefix" in /*) ;; *) echo 'Prefix must be absolute' >&2; exit 2;; esac
case "$prefix" in *"
"*|*"'"*|*'\'*|*'"'*|*'$'*|*'`'*) echo 'Unsupported characters in prefix' >&2; exit 2;; esac
[ "$prefix" != / ] || { echo 'Refusing root prefix' >&2; exit 2; }
case "$version" in latest) ;; v[0-9]*.[0-9]*.[0-9]*) case "$version" in *[!a-zA-Z0-9._-]*) echo 'Invalid version' >&2; exit 2;; esac;; *) echo 'Version must be latest or vX.Y.Z' >&2; exit 2;; esac
bin=$prefix/bin
marker=$prefix/.ssh-client-owned
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'; else shasum -a 256 "$1" | awk '{print $1}'; fi; }
# Never overwrite or delete a binary unless its checksum matches our receipt.
check_owned() {
  for file in ssh ssh-client; do
    if [ -e "$bin/$file" ] || [ -L "$bin/$file" ]; then
      [ ! -L "$bin/$file" ] && [ -f "$bin/$file" ] && [ -f "$marker" ] || { echo "Refusing foreign file: $bin/$file" >&2; exit 1; }
      expected=$(awk -v name="$file" '$2 == name {print $1}' "$marker")
      [ -n "$expected" ] && [ "$(sha "$bin/$file")" = "$expected" ] || { echo "Refusing modified file: $bin/$file" >&2; exit 1; }
    fi
  done
}
remove_profile_block() {
  profile=$1
  [ -f "$profile" ] || return 0
  [ ! -L "$profile" ] || { echo "Skipping symlinked profile: $profile" >&2; return 0; }
  # The marker embeds the prefix so uninstalling one installation cannot affect another.
  start="# >>> ssh-client PATH $prefix >>>"
  end="# <<< ssh-client PATH $prefix <<<"
  scratch=$(mktemp "${profile}.ssh-client.XXXXXX")
  if ! awk -v start="$start" -v end="$end" '$0 == start {if(skip) bad=1; skip=1;next} $0 == end {if(!skip) bad=1; skip=0;next} !skip {print} END {if(skip || bad) exit 1}' "$profile" > "$scratch"; then
    rm -f "$scratch"
    echo "Preserving malformed PATH block in $profile" >&2
    return 1
  fi
  cat "$scratch" > "$profile"
  rm -f "$scratch"
}
# Validate existing path components before either installing or uninstalling.
path=$prefix
while [ "$path" != / ]; do [ ! -L "$path" ] || { echo "Refusing symlink: $path" >&2; exit 1; }; path=$(dirname "$path"); done
[ ! -L "$bin" ] && [ ! -L "$marker" ] || { echo 'Refusing symlinked installation' >&2; exit 1; }
check_owned
if [ "$uninstall" = 1 ]; then
  if [ -f "$marker" ]; then
    mkdir "$prefix/.install-lock" 2>/dev/null || { echo 'Another install is running (or stale .install-lock requires manual inspection)' >&2; exit 1; }
    trap 'rmdir "$prefix/.install-lock" 2>/dev/null || :' EXIT
    check_owned
    if ! rm -f "$bin/ssh" "$bin/ssh-client"; then
      echo 'Could not remove all binaries. Receipt and PATH were retained; check permissions or running processes and retry uninstall.' >&2
      exit 1
    fi
    if [ -e "$bin/ssh" ] || [ -L "$bin/ssh" ] || [ -e "$bin/ssh-client" ] || [ -L "$bin/ssh-client" ]; then
      echo 'Binaries remain after removal; receipt and PATH were retained.' >&2
      exit 1
    fi
    rm -f "$marker"
    if [ "$update_path" = 1 ]; then for profile in "$HOME/.profile" "$HOME/.bashrc" "$HOME/.zshrc"; do remove_profile_block "$profile" || :; done; fi
    rmdir "$prefix/.install-lock" 2>/dev/null || :
    rmdir "$bin" "$prefix" 2>/dev/null || :
    echo 'Uninstalled managed binaries. Open a new shell to refresh PATH.'
  else
    echo 'Nothing installed (no ownership receipt).'
  fi
  exit 0
fi
case "$(uname -s)" in Linux) os=unknown-linux-gnu;; Darwin) os=apple-darwin;; *) echo 'Unsupported OS; Windows users should use install.ps1' >&2; exit 1;; esac
case "$(uname -m)" in x86_64|amd64) arch=x86_64;; aarch64|arm64) arch=aarch64;; *) echo 'Unsupported CPU architecture' >&2; exit 1;; esac
command -v curl >/dev/null 2>&1 || { echo 'curl is required' >&2; exit 1; }
command -v tar >/dev/null 2>&1 || { echo 'tar is required' >&2; exit 1; }
if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then echo 'sha256sum or shasum is required' >&2; exit 1; fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
fetch() { curl --fail --location --silent --show-error --proto '=https' --tlsv1.2 "$1" -o "$2"; }
if [ "$version" = latest ]; then
  fetch "https://api.github.com/repos/$repo/releases/latest" "$work/release.json"
  version=$(sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$work/release.json" | head -n 1)
  case "$version" in v[0-9]*.[0-9]*.[0-9]*) case "$version" in *[!a-zA-Z0-9._-]*) echo 'Unsafe release tag' >&2; exit 1;; esac;; *) echo 'Cannot resolve release tag' >&2; exit 1;; esac
fi
asset=ssh-client-$version-$arch-$os.tar.gz
url=https://github.com/$repo/releases/download/$version
fetch "$url/SHA256SUMS" "$work/SHA256SUMS"
fetch "$url/$asset" "$work/package.tar.gz"
expected=$(awk -v name="$asset" '$2 == name || $2 == "*"name {print $1}' "$work/SHA256SUMS")
[ ${#expected} -eq 64 ] && [ "$(sha "$work/package.tar.gz")" = "$expected" ] || { echo 'SHA256 verification failed' >&2; exit 1; }
# Reject extra paths, directories, links, duplicates and traversal before extraction.
tar -tzf "$work/package.tar.gz" > "$work/entries"
[ "$(wc -l < "$work/entries" | tr -d ' ')" = 2 ] && grep -qx ssh-client "$work/entries" && grep -qx ssh "$work/entries" || { echo 'Invalid archive entries' >&2; exit 1; }
tar -tvzf "$work/package.tar.gz" | awk 'substr($0,1,1) != "-" {bad=1} END {exit bad}' || { echo 'Archive must contain regular files' >&2; exit 1; }
mkdir "$work/extract"
tar -xzf "$work/package.tar.gz" -C "$work/extract"
# Reject symlinked install locations, including existing parent components.
path=$prefix
while [ "$path" != / ]; do [ ! -L "$path" ] || { echo "Refusing symlink: $path" >&2; exit 1; }; path=$(dirname "$path"); done
[ ! -L "$bin" ] && [ ! -L "$marker" ] || { echo 'Refusing symlinked installation' >&2; exit 1; }
umask 077
mkdir -p "$prefix"
lock=$prefix/.install-lock
mkdir "$lock" 2>/dev/null || { echo 'Another install is running (or stale .install-lock requires manual inspection)' >&2; exit 1; }
stage=
changed=
committed=0
cleanup() {
  rollback_failed=0
  if [ "$committed" = 0 ] && [ -n "$stage" ]; then
    for file in $changed; do
      if [ -f "$stage/$file.backup" ]; then cp "$stage/$file.backup" "$bin/$file" || { echo "Rollback failed: $bin/$file; backup retained at $stage" >&2; rollback_failed=1; }
      else rm -f "$bin/$file"; fi
    done
  fi
  rm -rf "$work"
  if [ -n "$stage" ] && [ "$rollback_failed" = 0 ]; then rm -rf "$stage"; fi
  rmdir "$lock" 2>/dev/null || :
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
mkdir -p "$bin"
check_owned
stage=$(mktemp -d "$prefix/.stage.XXXXXX")
for file in ssh-client ssh; do
  cp "$work/extract/$file" "$stage/$file"
  chmod 755 "$stage/$file"
  if [ -f "$bin/$file" ]; then cp -p "$bin/$file" "$stage/$file.backup"; fi
done
for file in ssh-client ssh; do
  mv -f "$stage/$file" "$bin/$file"
  changed="$changed $file"
done
{ for file in ssh-client ssh; do printf '%s  %s\n' "$(sha "$bin/$file")" "$file"; done; } > "$stage/receipt"
mv -f "$stage/receipt" "$marker"
committed=1
if [ "$update_path" = 1 ]; then
  for profile in "$HOME/.profile" "$HOME/.bashrc" "$HOME/.zshrc"; do
    if [ -L "$profile" ]; then echo "Skipping symlinked profile: $profile" >&2; continue; fi
    remove_profile_block "$profile" || continue
    printf '\n# >>> ssh-client PATH %s >>>\ncase ":$PATH:" in *":%s:"*) ;; *) export PATH="%s:$PATH" ;; esac\n# <<< ssh-client PATH %s <<<\n' "$prefix" "$bin" "$bin" "$prefix" >> "$profile"
  done
fi
printf 'Installed %s to %s\nOpen a new shell or run: export PATH="%s:$PATH"\n' "$version" "$bin" "$bin"
echo 'The dedicated PATH entry takes precedence over system ssh; system binaries are unchanged.'
