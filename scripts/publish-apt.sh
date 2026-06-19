#!/usr/bin/env bash
# Publishes Linux .deb builds to the S3-hosted apt repository at
# s3://$S3_BUCKET and (re)signs the repo metadata. Run from CI — see the
# publish-apt job in .github/workflows/release.yml, which downloads both
# arch's .deb artifacts, imports the GPG signing key, assumes the AWS role,
# and then calls this script.
#
# Pool history is intentionally never deleted (no `--delete` on the pool
# sync below) so older versions stay installable/pinnable; only the
# regenerated dists/ index is pruned of stale files.
set -euo pipefail

: "${S3_BUCKET:?set S3_BUCKET}"
: "${GPG_KEY_FPR:?set GPG_KEY_FPR}"
: "${DEB_DIR:?set DEB_DIR (directory containing the .deb files to publish)}"
# Resolve DEB_DIR once up-front so relative paths survive the temp-dir cd.
DEB_DIR="$(cd "$DEB_DIR" && pwd)"
CODENAME="${CODENAME:-stable}"
COMPONENT="${COMPONENT:-main}"
ARCHES="${ARCHES:-amd64 arm64}"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"

echo "==> Pulling existing repo state from s3://$S3_BUCKET"
mkdir -p pool dists
aws s3 sync "s3://$S3_BUCKET/pool" pool --no-progress
aws s3 sync "s3://$S3_BUCKET/dists" dists --no-progress

echo "==> Adding new .deb(s) to pool/$COMPONENT"
mkdir -p "pool/$COMPONENT"
debs=("$DEB_DIR"/*.deb)
if [ "${#debs[@]}" -eq 0 ]; then
  echo "::error::No .deb files found in DEB_DIR=$DEB_DIR"
  exit 1
fi
for deb in "${debs[@]}"; do
  # Re-derive the canonical Debian filename from the .deb's own control
  # data rather than trusting the incoming filename — dpkg-scanpackages's
  # --arch filter below matches on the `_<arch>.deb` filename suffix, and
  # the release workflow's GitHub-release copies are renamed to
  # `Flick-<arch>.deb`, which wouldn't match.
  pkg="$(dpkg-deb -f "$deb" Package)"
  ver="$(dpkg-deb -f "$deb" Version)"
  arch="$(dpkg-deb -f "$deb" Architecture)"
  cp "$deb" "pool/$COMPONENT/${pkg}_${ver}_${arch}.deb"
done

for ARCH in $ARCHES; do
  echo "==> Indexing $ARCH"
  mkdir -p "dists/$CODENAME/$COMPONENT/binary-$ARCH"
  dpkg-scanpackages --arch "$ARCH" pool /dev/null \
    > "dists/$CODENAME/$COMPONENT/binary-$ARCH/Packages"
  gzip -9c "dists/$CODENAME/$COMPONENT/binary-$ARCH/Packages" \
    > "dists/$CODENAME/$COMPONENT/binary-$ARCH/Packages.gz"
done

echo "==> Writing Release"
cat > apt-ftparchive.conf <<EOF
APT::FTPArchive::Release::Origin "Flick";
APT::FTPArchive::Release::Label "Flick";
APT::FTPArchive::Release::Suite "$CODENAME";
APT::FTPArchive::Release::Codename "$CODENAME";
APT::FTPArchive::Release::Architectures "$ARCHES";
APT::FTPArchive::Release::Components "$COMPONENT";
APT::FTPArchive::Release::Description "Flick apt repository";
EOF
apt-ftparchive -c apt-ftparchive.conf release "dists/$CODENAME" > "dists/$CODENAME/Release"

echo "==> Signing Release (detached Release.gpg + inline InRelease)"
rm -f "dists/$CODENAME/Release.gpg" "dists/$CODENAME/InRelease"
gpg --batch --yes --pinentry-mode loopback --passphrase-fd 0 \
  --default-key "$GPG_KEY_FPR" --detach-sign --armor \
  -o "dists/$CODENAME/Release.gpg" "dists/$CODENAME/Release" <<< "${GPG_PASSPHRASE:-}"
gpg --batch --yes --pinentry-mode loopback --passphrase-fd 0 \
  --default-key "$GPG_KEY_FPR" --clearsign \
  -o "dists/$CODENAME/InRelease" "dists/$CODENAME/Release" <<< "${GPG_PASSPHRASE:-}"

echo "==> Exporting public key (served at /pubkey.gpg for users to import)"
gpg --armor --export "$GPG_KEY_FPR" > pubkey.gpg

echo "==> Publishing to s3://$S3_BUCKET"
aws s3 sync pool "s3://$S3_BUCKET/pool" \
  --content-type "application/vnd.debian.binary-package" --no-progress

# Two passes so each file gets the right Content-Type. --exclude/--include
# rules are evaluated in order with last-match-wins, so the *.gz pass must
# exclude everything first and then re-include *.gz, not the other order.
aws s3 sync dists "s3://$S3_BUCKET/dists" --delete --no-progress \
  --content-type "text/plain; charset=utf-8" --exclude "*.gz"
aws s3 sync dists "s3://$S3_BUCKET/dists" --delete --no-progress \
  --content-type "application/gzip" --exclude "*" --include "*.gz"

aws s3 cp pubkey.gpg "s3://$S3_BUCKET/pubkey.gpg" \
  --content-type "application/pgp-keys"

echo "==> Done"
