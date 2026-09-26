#!/bin/zsh
set -eu

# Only adjust Crawler's private key. The password stays in this local process.
keychain="$HOME/Library/Keychains/login.keychain-db"
print 'Allow Apple codesign to use the existing Crawler Local Development key.'
print 'Enter your login keychain password here, not in chat. Input is hidden.'
read -rs 'crawler_keychain_password?Keychain password: '
print
trap 'unset crawler_keychain_password' EXIT
/usr/bin/security set-key-partition-list \
  -S 'apple-tool:,apple:' -s -t private -l 'Crawler Local Development' \
  -k "$crawler_keychain_password" "$keychain" >/dev/null
unset crawler_keychain_password
print 'Crawler signing-key authorization updated. You can close this window.'
