#!/usr/bin/env bash

set -euo pipefail

readonly ARCHIVES="fixtures/archives"
export COPYFILE_DISABLE=1

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

readonly STAMP=202601010000

tar_owner=()
if tar --uid 0 --gid 0 -cf /dev/null -T /dev/null 2>/dev/null; then
  tar_owner=(--uid 0 --gid 0 --uname "" --gname "")
elif tar --owner=0 --group=0 -cf /dev/null -T /dev/null 2>/dev/null; then
  tar_owner=(--owner=0 --group=0)
fi

mkdir -p "$ARCHIVES"

write() {
  mkdir -p "$(dirname "$1")"
  printf '%s' "$2" >"$1"
}

targz() {
  local out=$1 root=$2 base=${3:-$work}
  find "$base/$root" -exec touch -t "$STAMP" {} +
  (cd "$base" && tar "${tar_owner[@]}" -cf - "$root") | gzip -n -9 >"${ARCHIVES}/${out}"
}

zip_up() {
  local out=$1 dir=$2 prefix=${3:-}
  find "$dir" -exec touch -t "$STAMP" {} +
  rm -f "${ARCHIVES:?}/${out}"
  local root=$dir target=.
  if [[ -n "$prefix" ]]; then
    root=$(dirname "$dir")
    target=$prefix
  fi
  (cd "$root" && zip -q -X -r "$OLDPWD/${ARCHIVES}/${out}" "$target")
}

npm=$work/package
write "$npm/package.json" '{
  "name": "@types/node",
  "version": "20.1.0"
}
'
write "$npm/index.d.ts" 'declare const version: string;
'
targz "node-20.1.0.tgz" "package"

sdist=$work/requests-2.31.0
write "$sdist/setup.py" 'from setuptools import setup

setup(name="requests", version="2.31.0")
'
write "$sdist/requests/__init__.py" '__version__ = "2.31.0"
'
targz "requests-2.31.0.tar.gz" "requests-2.31.0"

wheel=$work/wheel
write "$wheel/requests/__init__.py" '__version__ = "2.31.0"
'
write "$wheel/requests-2.31.0.dist-info/METADATA" 'Metadata-Version: 2.1
Name: requests
Version: 2.31.0
'
zip_up "requests-2.31.0-py3-none-any.whl" "$wheel"

wheel_only=$work/wheel-only
write "$wheel_only/tensorflow/__init__.py" 'VERSION = "2.16.1"
'
write "$wheel_only/tensorflow-2.16.1.dist-info/METADATA" 'Metadata-Version: 2.1
Name: tensorflow
Version: 2.16.1
'
zip_up "tensorflow-2.16.1-py3-none-any.whl" "$wheel_only"

krate=$work/serde-1.0.0
write "$krate/Cargo.toml" '[package]
name = "serde"
version = "1.0.0"
'
write "$krate/src/lib.rs" 'pub fn serialize() {}
'
targz "serde-1.0.0.crate" "serde-1.0.0"

zipsdist=$work/numpy-1.9.0
write "$zipsdist/setup.py" 'from distutils.core import setup

setup(name="numpy", version="1.9.0")
'
write "$zipsdist/numpy/__init__.py" '__version__ = "1.9.0"
'
zip_up "numpy-1.9.0.zip" "$work/numpy-1.9.0" "numpy-1.9.0"

readonly MANY=2500
many=$work/many
mkdir -p "$many/package"
for i in $(seq 1 "$MANY"); do
  printf -v padded '%04d' "$i"
  write "${many}/package/src/${padded:0:2}/module-${padded}.js" "export const n = ${i};
"
done
targz "many-files-1.0.0.tgz" "package" "$many"

readonly ENORMOUS=6000
enormous=$work/enormous
mkdir -p "$enormous/package"
for i in $(seq 1 "$ENORMOUS"); do
  printf -v padded '%05d' "$i"
  client=$((10#${padded} / 600))
  schema=$((10#${padded} / 60))
  printf -v client '%02d' "$client"
  printf -v schema '%03d' "$schema"
  write "${enormous}/package/packages/generated-client-for-the-platform-api-${client}/src/resources/schema-definitions-and-validators-${schema}/resource-descriptor-with-a-deliberately-long-generated-name-${padded}.generated.ts" "export const n = ${i};
"
done
targz "enormous-1.0.0.tgz" "package" "$enormous"

enormous_next=$work/enormous-next
descriptor="packages/generated-client-for-the-platform-api-00/src/resources"
descriptor="${descriptor}/schema-definitions-and-validators-000"
descriptor="${descriptor}/resource-descriptor-with-a-deliberately-long-generated-name"
write "${enormous_next}/package/${descriptor}-00001.generated.ts" "export const n = 1;
"
write "${enormous_next}/package/${descriptor}-00002.generated.ts" "export const n = 2;
export const alsoChanged = true;
"
write "${enormous_next}/package/README.md" "Generated clients for the platform API.

Nothing in this file resembles a resource descriptor, so no removal pairs with
it and the statuses below stay the fixture's rather than the threshold's.
"
targz "enormous-2.0.0.tgz" "package" "$enormous_next"

odd=$work/odd
mkdir -p "$odd/package"
printf '\211PNG\r\n\032\n\377\376' >"$odd/package/logo.png"
awk 'BEGIN { for (i = 0; i < 100000; i++) print "export const n = 0;" }' \
  >"$odd/package/big.txt"
targz "odd-files-1.0.0.tgz" "package" "$odd"

empty=$work/empty-file
mkdir -p "$empty/package"
: >"$empty/package/empty.txt"
write "$empty/package/src/index.js" 'export {};
'
targz "empty-file-1.0.0.tgz" "package" "$empty"

shape=$work/shape
for version in 1.0.0 2.0.0; do
  write "$shape/$version/package/package.json" '{
  "name": "shape"
}
'
done
write "$shape/1.0.0/package/lib" 'A plain file, where 2.0.0 has a directory.
'
write "$shape/2.0.0/package/lib/index.js" 'export const shape = "a directory now";
'
targz "shape-1.0.0.tgz" "package" "$shape/1.0.0"
targz "shape-2.0.0.tgz" "package" "$shape/2.0.0"

sneaky=$work/sneaky-1.0.0
write "$sneaky/setup.py" 'from setuptools import setup

setup(name="sneaky", version="1.0.0")
'
targz "sneaky-1.0.0.tar.gz" "sneaky-1.0.0"

echo "ok: rebuilt the archives in ${ARCHIVES}/"
