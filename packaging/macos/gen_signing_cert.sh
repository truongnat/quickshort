#!/bin/sh
# One-time setup: create a self-signed code-signing identity "quickshort".
#
# Why not ad-hoc (-): TCC (Screen Recording / Accessibility) keys grants to
# the code's designated requirement. Ad-hoc signatures change every build, so
# every update would re-prompt for permissions. A stable self-signed cert
# keeps the identity constant across releases — updates install silently.
# It does NOT satisfy Gatekeeper (that needs a $99/yr Developer ID), which is
# why the brew cask strips quarantine and DMG users approve once in Settings.
#
# Output: identity "quickshort" in the login keychain, usable via
#   codesign -s "quickshort" ...
# For CI: export QS_SIGN_P12 (base64 p12) + QS_SIGN_P12_PASSWORD secrets.
set -e
NAME="quickshort"
# keep the p12 + password under the app config dir — losing them means the
# cert can never be reproduced and every re-generated cert creates a new
# TCC identity (permissions must be re-granted)
DIR="${QS_SIGN_DIR:-$HOME/.config/quickshort/signing}"
mkdir -p "$DIR"

cat > "$DIR/openssl.cnf" <<EOF
[ req ]
distinguished_name = dn
x509_extensions = ext
prompt = no
[ dn ]
CN = $NAME
[ ext ]
keyUsage = digitalSignature
extendedKeyUsage = codeSigning
basicConstraints = critical,CA:false
EOF

openssl req -x509 -newkey rsa:2048 -keyout "$DIR/key.pem" -out "$DIR/cert.pem" \
  -days 3650 -nodes -config "$DIR/openssl.cnf" 2>/dev/null

# p12 password comes from the first arg or is generated; print it for secrets
PASS="${1:-$(openssl rand -hex 12)}"
# -legacy: macOS `security import` only accepts the old PBE algorithms
openssl pkcs12 -export -legacy -out "$DIR/$NAME.p12" -inkey "$DIR/key.pem" \
  -in "$DIR/cert.pem" -password "pass:$PASS"

# -A: allow every app to use the key — otherwise the first `codesign` pops a
# SecurityAgent dialog that blocks headless builds
security import "$DIR/$NAME.p12" -k ~/Library/Keychains/login.keychain-db \
  -P "$PASS" -A

echo "imported identity '$NAME' into login keychain"
echo "p12 + password saved to: $DIR  (back it up — CI secrets QS_SIGN_P12 /"
echo "                              QS_SIGN_P12_PASSWORD come from these files)"
