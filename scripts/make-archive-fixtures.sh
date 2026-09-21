#!/usr/bin/env bash
#
# Rebuild the archives `fixtures/archives/` holds.
#
# The fixture adapter in `src/archive/` reads these instead of reaching a
# registry, which is what lets the suite assert what a version's files are
# without a network — and what lets #24's conformance run in CI. They are
# real archives made by `tar`, `gzip` and `zip` rather than by this crate, so
# a test that reads one back is not agreeing with our own extractor by
# construction.
#
# Every archive is tiny and its content is made up. What has to be real is the
# *shape*: the top-level directory npm and crates.io wrap a package in, the
# two top-level directories a wheel has and does not wrap, and the file
# extensions each registry serves.
#
# A rebuild has to produce the same bytes, or every regeneration is a diff
# nobody can review. Three things would otherwise vary: gzip stamps the time
# (`gzip -n`), tar and zip stamp each file's mtime (fixed below before either
# runs), and tar records who built it (zeroed, through whichever spelling of
# the flags this tar has). `COPYFILE_DISABLE` is macOS, which would otherwise
# slip `._` resource forks into a tar.
#
# Run from the repository root:
#
#   ./scripts/make-archive-fixtures.sh
#
# The generated files are checked in, and so is `index.json` beside them,
# which maps the URL a registry serves each archive from to the file here that
# stands in for it. That file is written by hand: it is the part a test is
# asserting about — that a fetch asks for the URL `registry` builds — so it
# should not be generated from the same place the fetch reads it.
#
# Rebuilding should leave `git status` clean unless the layouts below changed.

set -euo pipefail

readonly ARCHIVES="fixtures/archives"
export COPYFILE_DISABLE=1

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The same instant for every entry in every archive. Which instant does not
# matter; that it is not "now" does.
readonly STAMP=202601010000

# bsdtar and GNU tar spell "do not record who I am" differently, and both are
# in use. A tar that has neither still builds, and its archives differ between
# machines rather than being wrong.
tar_owner=()
if tar --uid 0 --gid 0 -cf /dev/null -T /dev/null 2>/dev/null; then
  tar_owner=(--uid 0 --gid 0 --uname "" --gname "")
elif tar --owner=0 --group=0 -cf /dev/null -T /dev/null 2>/dev/null; then
  tar_owner=(--owner=0 --group=0)
fi

mkdir -p "$ARCHIVES"

# Write $2 to $1, creating the directories above it.
write() {
  mkdir -p "$(dirname "$1")"
  printf '%s' "$2" >"$1"
}

# A gzip'd tar of $2 (a directory under $3, or under $work) at $ARCHIVES/$1.
#
# The base is a parameter because two archives here wrap their contents in the
# same directory name, and building both under one root would put the first
# one's files inside the second.
targz() {
  local out=$1 root=$2 base=${3:-$work}
  find "$base/$root" -exec touch -t "$STAMP" {} +
  (cd "$base" && tar "${tar_owner[@]}" -cf - "$root") | gzip -n -9 >"${ARCHIVES}/${out}"
}

# A zip of $2's contents (a directory), at $ARCHIVES/$1, or of $2 under the
# wrapper directory $3 when one is given. `-X` drops the platform's extra
# attributes.
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

# --- npm ------------------------------------------------------------------
#
# A scoped package, because the scope is the npm rule worth holding: the path
# keeps it and the filename drops it. npm wraps every package in `package/`.
npm=$work/package
write "$npm/package.json" '{
  "name": "@types/node",
  "version": "20.1.0"
}
'
write "$npm/index.d.ts" 'declare const version: string;
'
targz "node-20.1.0.tgz" "package"

# --- PyPI -----------------------------------------------------------------
#
# A version with both a source distribution and a wheel. The two carry
# different files on purpose: `setup.py` is only in the sdist and
# `.dist-info/` only in the wheel, so which one a fetch chose is visible in
# the file map rather than only in a URL.
#
# The sdist wraps everything in `{name}-{version}/`, the way `setuptools`
# builds one. The wheel does not: it has two top-level directories, which is
# why a wheel's paths keep their prefixes where an sdist's do not.
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

# A project that publishes no source distribution. TensorFlow is the real
# case — every release is wheels — and a wheel is then the only thing there is
# to diff.
wheel_only=$work/wheel-only
write "$wheel_only/tensorflow/__init__.py" 'VERSION = "2.16.1"
'
write "$wheel_only/tensorflow-2.16.1.dist-info/METADATA" 'Metadata-Version: 2.1
Name: tensorflow
Version: 2.16.1
'
zip_up "tensorflow-2.16.1-py3-none-any.whl" "$wheel_only"

# --- crates.io ------------------------------------------------------------
#
# A `.crate` is a gzip'd tar wrapping everything in `{name}-{version}/`, which
# is a different extension and a different wrapper than npm's and the same
# file map out the other side.
krate=$work/serde-1.0.0
write "$krate/Cargo.toml" '[package]
name = "serde"
version = "1.0.0"
'
write "$krate/src/lib.rs" 'pub fn serialize() {}
'
targz "serde-1.0.0.crate" "serde-1.0.0"

# An older PyPI release, whose source distribution is a zip rather than a
# tarball. numpy shipped one for years, and it is the shape that would break
# an extractor assuming gzip.
zipsdist=$work/numpy-1.9.0
write "$zipsdist/setup.py" 'from distutils.core import setup

setup(name="numpy", version="1.9.0")
'
write "$zipsdist/numpy/__init__.py" '__version__ = "1.9.0"
'
zip_up "numpy-1.9.0.zip" "$work/numpy-1.9.0" "numpy-1.9.0"

# --- a package with thousands of files ------------------------------------
#
# Every other archive here is a handful of files, chosen for a shape. This one
# is chosen for a *count*: it is what proves a listing tool answers with a page
# rather than with everything, which is a property no small package can fail.
#
# The files are empty and the paths are ordinary. What is being generated is
# the number of entries, not their content, so an archive of a few kilobytes
# stands in for a package nobody wants to check in.
readonly MANY=2500
many=$work/many
mkdir -p "$many/package"
for i in $(seq 1 "$MANY"); do
  # Spread over directories the way a real package is, so the listing has
  # directory entries between its files rather than one flat run of names.
  printf -v padded '%04d' "$i"
  write "${many}/package/src/${padded:0:2}/module-${padded}.js" "export const n = ${i};
"
done
targz "many-files-1.0.0.tgz" "package" "$many"

# --- the archive this server will not fetch -------------------------------
#
# A real archive at a host no registry names, referenced by a listing that
# names it. It exists so the allowlist test fails when the guard is missing:
# without the check the fetch succeeds and returns these files, rather than
# failing because a fixture was absent.
sneaky=$work/sneaky-1.0.0
write "$sneaky/setup.py" 'from setuptools import setup

setup(name="sneaky", version="1.0.0")
'
targz "sneaky-1.0.0.tar.gz" "sneaky-1.0.0"

echo "ok: rebuilt the archives in ${ARCHIVES}/"
