# Optional Windows code signing

This guide is for maintainers who choose to configure Windows Authenticode signing for release builds.

## Free open-source route: SignPath Foundation

Apply through [SignPath Foundation](https://signpath.org/apply), following its eligibility requirements and [terms](https://signpath.org/terms.html). Configure MFA and declare the actual author, reviewer and approver roles as required by the provider.

After acceptance:

1. Connect the GitHub repository to SignPath, install its required GitHub integration, and configure the project/trusted build policy for the GitHub-hosted workflow.
2. Create an artifact configuration selecting the EXE inside the uploaded `windows-build` artifact, and a **production** signing policy. Declare the actual authors, reviewers and signing approvers in the README.
3. Set repository secret `SIGNPATH_API_TOKEN` and repository variables `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`, `SIGNPATH_SIGNING_POLICY_SLUG`, `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG`.
4. Run **Windows build and release** manually for the desired tag (for example, `v0.1.6`). Approve its signing request in SignPath. The action waits up to one hour, downloads the signed EXE, validates Authenticode, and publishes/replaces the draft release.

The action uses uploaded GitHub artifact IDs and GitHub-hosted builds for provenance. See [SignPath's GitHub integration](https://docs.signpath.io/trusted-build-systems/github). Windows signature verification must report `Valid` for this signing route.

## Existing trusted certificate or signing service

Use the CA's supported hardware-token or managed signing tool. The code-signing certificate must chain to a Windows-trusted public CA and support Authenticode; a website HTTPS certificate does not qualify. Never put private keys or signing secrets in this repository.

With an accessible trusted certificate in the Windows user store and the Windows SDK's SignTool:

```powershell
signtool sign /sha1 YOUR_CERTIFICATE_THUMBPRINT /fd SHA256 /tr YOUR_CA_RFC3161_TIMESTAMP_URL /td SHA256 wardogs-earplugs.exe
signtool verify /pa /all /v wardogs-earplugs.exe
Get-AuthenticodeSignature .\wardogs-earplugs.exe
```

After a `Valid` result, package the EXE with signature validation:

```powershell
./scripts/package.ps1 -Executable ./wardogs-earplugs.exe -Tag v0.1.6 -RequireSignature
```

Publish the generated EXE, ZIP and `SHA256SUMS.txt`. The packaging step records the Authenticode requirement in the release manifest for this route. Hashes must cover the final signed file, and the file must not be modified after signing.

Provider enrollment and identity verification are completed through the maintainer's account. Keep private keys and signing secrets outside the repository.
