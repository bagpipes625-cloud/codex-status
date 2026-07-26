# Security policy

Please report security-sensitive findings through GitHub's private security advisory feature instead of a public issue. Include the affected version, a minimal reproduction, and the expected impact.

CodexStatus deliberately does not read authentication files or expose a network listener. Reports involving unexpected token access, process execution, startup persistence, or data disclosure are especially useful.

The automatic updater accepts only the repository's public stable GitHub Release asset with the expected versioned filename and HTTPS URL. The downloaded executable must match the SHA-256 digest returned in GitHub's release metadata before atomic replacement. Releases are not yet code-signed, so repository or release-account compromise remains outside the protection offered by this checksum.
