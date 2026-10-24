# Release signing

Windows release artifacts use the SignPath Foundation open-source signing
program. The signer is **SignPath Foundation**, not the project maintainer:
the Foundation holds the certificate and vouches for the verified build.

The pipeline is committed but **onboarding is not complete**. The `v0.1.1`
release is unsigned. Do not push a `v*` tag until the setup below is finished;
the release workflow fails before building if its signing configuration is
missing.

## What is signed

Two separate SignPath requests are made for every release:

1. `screencapture.exe` is signed first.
2. The NSIS installer is built around that signed executable, then
   `screencapture-setup.exe` is signed.

This ordering matters. Signing only the finished installer would leave the
installed `screencapture.exe` unsigned. The bundled FFmpeg binaries are
upstream components and are intentionally not signed; SignPath permits an
installer to include unsigned upstream OSS components.

Each request needs manual approval. Expect two approval prompts per release.
The signing workflow waits up to one hour for each request.

## One-time SignPath onboarding

1. Apply to the [SignPath Foundation](https://signpath.org/). The project is
   public, Apache-2.0, already released, and maintains the
   [code signing policy](../README.md#code-signing-policy) required by the
   program. The project has one maintainer, so that person must use MFA for
   GitHub and SignPath.
2. In SignPath, create or verify the project for this repository. Add
   GitHub.com as its trusted build system, link the project to this
   repository, and install the SignPath GitHub App if audit-log evaluation is
   used.
3. Create these artifact configurations and paste the matching XML from
   this repository:
   - `screencapture-app` from
     [`signpath/app-artifact-configuration.xml`](../signpath/app-artifact-configuration.xml)
   - `screencapture-installer` from
     [`signpath/installer-artifact-configuration.xml`](../signpath/installer-artifact-configuration.xml)
4. Create the `release-signing` signing policy with manual approval enabled
   and both artifact configurations available. Add the maintainer as an
   approver.
5. Create an API token for a user with submitter permission for the project
   and policy. Store it as a GitHub repository secret:

   ```powershell
   gh secret set SIGNPATH_API_TOKEN
   ```

6. Add the non-secret repository variables. Replace the organization id with
   the value from SignPath:

   ```powershell
   gh variable set SIGNPATH_ORGANIZATION_ID --body "<SignPath organization id>"
   gh variable set SIGNPATH_PROJECT_SLUG --body "screencapture"
   gh variable set SIGNPATH_SIGNING_POLICY_SLUG --body "release-signing"
   gh variable set SIGNPATH_APP_ARTIFACT_CONFIGURATION_SLUG --body "screencapture-app"
   gh variable set SIGNPATH_INSTALLER_ARTIFACT_CONFIGURATION_SLUG --body "screencapture-installer"
   ```

7. Confirm the repository home page still carries the required
   [Code signing policy](../README.md#code-signing-policy), including the
   SignPath attribution, team roles, and privacy statement.

## Cutting a signed release

1. Bump `Cargo.toml`, `Cargo.lock`, `assets/icon.rc`, and the NSIS version
   together, then merge the version change to `main`.
2. Create and push an annotated tag whose version matches `Cargo.toml`:

   ```powershell
   git tag -a v0.1.2 -m "screencapture v0.1.2"
   git push origin v0.1.2
   ```

3. The `Release` workflow checks that the tag is on `main` and matches the
   package version, runs all gates, fetches the pinned FFmpeg engine, builds
   the installer, and submits the executable and installer to SignPath.
4. Approve both signing requests in SignPath. The workflow downloads the
   signed files, verifies their Authenticode status and timestamps, writes
   `SHA256SUMS.txt`, and publishes the GitHub release.

The workflow only runs on GitHub-hosted Windows runners, as required by the
SignPath OSS trusted-build policy. It never checks out or builds untrusted
pull-request code.

## Verify a downloaded release

On Windows, with the files downloaded beside this checkout:

```powershell
pwsh ./scripts/verify-signature.ps1 -Path @(
    "screencapture.exe",
    "screencapture-setup.exe"
)
```

The script fails unless Windows reports `Valid` and the signature carries a
timestamp. The equivalent manual checks are:

```powershell
Get-AuthenticodeSignature -LiteralPath .\screencapture-setup.exe |
    Format-List Status, StatusMessage, SignerCertificate, TimeStamperCertificate

signtool verify /pa /all /v .\screencapture-setup.exe
Get-FileHash -Algorithm SHA256 .\screencapture-setup.exe
```

The published `SHA256SUMS.txt` covers both release executables.

## Common failures

- **Missing SignPath configuration** — add the secret and variables above;
  the workflow intentionally fails before building.
- **Tag does not match `Cargo.toml`** — bump the version and use `v<version>`.
- **Tag is not on `main`** — create release tags only from merged `main`, not
  from a branch or an unmerged commit.
- **One-hour signing timeout** — approve both requests while the workflow is
  waiting. If the run ends, follow SignPath's re-run policy and inspect the
  requests before retrying.
- **Signature is not valid after download** — do not publish the file; check
  that SignPath returned both artifacts and that the local file matches
  `SHA256SUMS.txt`.
- **`windres` is missing** — the workflow installs MinGW for the resource
  compiler and passes its path as `WINDRES`. Locally, use an MSYS2 shell with
  `mingw-w64-binutils`, or set `WINDRES` to the resource compiler to use.
