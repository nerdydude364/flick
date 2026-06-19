#!/usr/bin/env bash
# One-time helper: generates a dedicated GPG key for signing the apt
# repository's Release/InRelease files (see scripts/publish-apt.sh and the
# publish-apt job in .github/workflows/release.yml). Run this locally —
# never in CI — then copy the printed secret values into the repo's
# GitHub Actions secrets and delete the output directory.
#
# RSA4096 sign-only, no expiry, no passphrase: this key only ever signs
# release metadata from an unattended CI job, so there's no human present
# to type a passphrase, and RSA verifies with the gpgv shipped on older
# apt/Debian releases more reliably than ed25519 does.
set -euo pipefail
cd "$(dirname "$0")/.."

NAME="${1:-Flick Apt Repository}"
EMAIL="${2:-apt@flick.free}"
OUT="tmp/apt-signing-key"

if [ -e "$OUT" ]; then
  echo "$OUT already exists — remove it first if you mean to regenerate." >&2
  exit 1
fi
mkdir -p "$OUT"
mkdir -m 700 -p "$OUT/gnupg-home"

echo "==> Generating RSA4096 signing key for: $NAME <$EMAIL>"
gpg --homedir "$OUT/gnupg-home" --batch --passphrase '' --quick-generate-key \
  "$NAME <$EMAIL>" rsa4096 sign 0

FPR="$(gpg --homedir "$OUT/gnupg-home" --list-secret-keys --with-colons | awk -F: '/^fpr/ {print $10; exit}')"

gpg --homedir "$OUT/gnupg-home" --armor --export-secret-keys "$FPR" > "$OUT/private.asc"
gpg --homedir "$OUT/gnupg-home" --armor --export "$FPR" > "$OUT/public.asc"
rm -rf "$OUT/gnupg-home"

cat <<EOF

==> Done. Fingerprint: $FPR

Wrote (gitignored, under tmp/):
  $OUT/private.asc  -> goes in the GitHub Actions secret APT_GPG_PRIVATE_KEY
  $OUT/public.asc   -> what end users import; publish-apt.sh re-exports and
                       uploads this to s3://<bucket>/pubkey.gpg on every
                       release, so you don't need to upload it by hand.

Next steps:
  1. In GitHub: repo Settings -> Secrets and variables -> Actions, add:
       APT_GPG_PRIVATE_KEY = contents of $OUT/private.asc
       APT_GPG_PASSPHRASE  = (leave unset — this key has no passphrase)
  2. Delete $OUT once the secret is saved: rm -rf $OUT
EOF
