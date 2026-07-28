# Verifying a release archive

Each release archive (`shamir-server-<tag>-<target>.tar.gz` on Linux/macOS,
`shamir-server-<tag>-<target>.zip` on Windows) ships with a matching
`<archive>.sha256` checksum file in coreutils `sha256sum` format
(`<hex digest>  <archive-filename>`). Verify the archive before you run the
binary it contains.

## Linux / macOS

Place the archive and its `.sha256` side by side, then:

    sha256sum -c shamir-server-<tag>-<target>.tar.gz.sha256

`sha256sum` is part of coreutils. On macOS it is available via
`brew install coreutils` (as `gsha256sum`), or use `shasum -a 256` manually.

## Windows (PowerShell)

`sha256sum` ships with Git for Windows (Git Bash), so the `sha256sum -c`
command above works there too. The native PowerShell equivalent is:

    $expected = (Get-Content shamir-server-<tag>-<target>.zip.sha256).Split(' ')[0]
    $actual   = (Get-FileHash shamir-server-<tag>-<target>.zip -Algorithm SHA256).Hash.ToLower()
    $actual -eq $expected    # -> True

## Signatures (optional)

Every archive is also signed with keyless sigstore/cosign via GitHub OIDC. The
GitHub Release carries matching `.sig`, `.pem`, and `.bundle` files for offline
signature verification:

    cosign verify-blob --bundle <archive>.bundle --certificate <archive>.pem \
        --signature <archive>.sig <archive>
