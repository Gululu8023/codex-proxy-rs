#!/bin/sh
# 固定上游源码与本地补丁，不创建提交、不覆盖不匹配的工作副本。
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
destination="$root/.deps/veil"
revision=82f9290022144c82609aaad149d81693c4a563ec
patch="$root/scripts/patches/veil-library.patch"
if [ ! -e "$destination" ]; then
    mkdir -p "$destination"
    git -C "$destination" init --quiet
    git -C "$destination" remote add origin https://github.com/Mouseww/veil.git
    git -C "$destination" fetch --depth 1 origin "$revision"
    git -C "$destination" checkout --detach FETCH_HEAD
fi
if [ "$(git -C "$destination" rev-parse HEAD)" != "$revision" ]; then
    printf '%s\n' 'Existing veil checkout does not match the pinned revision; it has been preserved' >&2
    exit 1
fi
if ! git -C "$destination" apply --reverse --check "$patch" 2>/dev/null; then
    git -C "$destination" apply --check "$patch"
    git -C "$destination" apply "$patch"
fi
printf '%s\n' "Veil ready: $revision + scripts/patches/veil-library.patch"
